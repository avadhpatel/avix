use std::sync::Arc;

use crate::invocation::conversation::{ConversationEntry, Role};
use crate::memfs::VfsRouter;
use crate::memory_svc::{
    service::{CallerContext, MemoryService},
    UserPreferenceModel,
};

/// Manages episodic memory, per-session context, and conversation history for an agent.
pub struct MemoryManager {
    /// Attached memory service for dispatching memory tools (e.g. `memory/log-event`).
    pub memory_svc: Option<Arc<MemoryService>>,
    /// Pre-built memory context block injected into the system prompt at spawn.
    pub memory_context: Option<String>,
    /// Conversation history: structured entries for persistence and auto-log.
    pub conversation_history: Vec<ConversationEntry>,
}

impl MemoryManager {
    pub fn new() -> Self {
        Self {
            memory_svc: None,
            memory_context: None,
            conversation_history: Vec::new(),
        }
    }

    /// Append a conversation message to the history.
    pub fn push_conversation_message(&mut self, role: &str, content: &str) {
        let r = match role {
            "assistant" => Role::Assistant,
            "tool" => Role::Tool,
            "system" => Role::System,
            _ => Role::User,
        };
        self.conversation_history
            .push(ConversationEntry::from_role_content(r, content));
    }

    /// Build the memory context block and store it in `memory_context`.
    ///
    /// No-op when no VFS is attached.
    pub async fn init_memory_context(
        &mut self,
        vfs: Option<&Arc<VfsRouter>>,
        spawned_by: &str,
        agent_name: &str,
    ) {
        self.memory_context = self.build_memory_context_block(vfs, spawned_by, agent_name).await;
    }

    pub async fn build_memory_context_block(
        &self,
        vfs: Option<&Arc<VfsRouter>>,
        spawned_by: &str,
        agent_name: &str,
    ) -> Option<String> {
        use crate::memfs::VfsPath;
        use crate::memory_svc::store;
        let vfs = vfs?;
        let mut parts = vec![];

        // 1. User preferences
        let pref_path = UserPreferenceModel::vfs_path(spawned_by, agent_name);
        if let Ok(bytes) = vfs.read(&VfsPath::parse(&pref_path).ok()?).await {
            if let Ok(model) = UserPreferenceModel::from_yaml(&String::from_utf8_lossy(&bytes)) {
                if !model.spec.summary.is_empty() {
                    let mut pref_text = format!("User preferences:\n  {}", model.spec.summary);
                    if !model.spec.corrections.is_empty() {
                        pref_text.push_str("\n\n  Corrections to avoid repeating:");
                        for c in &model.spec.corrections {
                            pref_text.push_str(&format!(
                                "\n    • \"{}\" ({})",
                                c.correction,
                                c.at.format("%Y-%m-%d")
                            ));
                        }
                    }
                    parts.push(pref_text);
                }
            }
        }

        // 2. Recent episodic context (last 5 records)
        let episodic_dir = format!("/users/{spawned_by}/memory/{agent_name}/episodic");
        if let Ok(mut records) = store::list_records(vfs, &episodic_dir).await {
            records.sort_by(|a, b| b.metadata.created_at.cmp(&a.metadata.created_at));
            let recent: Vec<_> = records.into_iter().take(5).collect();
            if !recent.is_empty() {
                let mut hist = format!("Recent session history (last {}):", recent.len());
                for r in &recent {
                    let summary_len = r.spec.content.len().min(120);
                    hist.push_str(&format!(
                        "\n  • {} {}",
                        r.metadata.created_at.format("%Y-%m-%d"),
                        &r.spec.content[..summary_len]
                    ));
                }
                parts.push(hist);
            }
        }

        // 3. Pinned facts
        let semantic_dir = format!("/users/{spawned_by}/memory/{agent_name}/semantic");
        if let Ok(all_semantic) = store::list_records(vfs, &semantic_dir).await {
            let pinned: Vec<_> = all_semantic
                .into_iter()
                .filter(|r| r.metadata.pinned)
                .collect();
            if !pinned.is_empty() {
                let mut pin_text = "Pinned facts:".to_string();
                for r in &pinned {
                    let key = r.spec.key.as_deref().unwrap_or(&r.metadata.id);
                    let content_len = r.spec.content.len().min(120);
                    pin_text.push_str(&format!(
                        "\n  • {}: {}",
                        key,
                        &r.spec.content[..content_len]
                    ));
                }
                parts.push(pin_text);
            }
        }

        if parts.is_empty() {
            return None;
        }

        Some(format!(
            "[MEMORY CONTEXT — {agent_name} — injected by memory.svc]\n\n{}",
            parts.join("\n\n")
        ))
    }

    /// Write a session summary to episodic memory when SIGSTOP fires.
    pub async fn auto_log_session_end(
        &self,
        pid: u64,
        agent_name: &str,
        spawned_by: &str,
        session_id: &str,
        granted_tools: &[String],
    ) {
        let svc = match &self.memory_svc {
            Some(s) => Arc::clone(s),
            None => return,
        };
        if self.conversation_history.is_empty() {
            return;
        }

        let summary = self
            .conversation_history
            .iter()
            .map(|entry| {
                let preview_len = entry.content.len().min(200);
                format!("{:?}: {}", entry.role, &entry.content[..preview_len])
            })
            .collect::<Vec<_>>()
            .join("\n");

        let caller = CallerContext {
            pid,
            agent_name: agent_name.to_string(),
            owner: spawned_by.to_string(),
            session_id: session_id.to_string(),
            granted_tools: granted_tools.to_vec(),
        };
        let params = serde_json::json!({
            "summary": summary,
            "outcome": "success",
            "scope": "own"
        });
        if let Err(e) = svc.dispatch("memory/log-event", params, &caller).await {
            tracing::warn!(pid, err = ?e, "auto session log failed");
        }
    }
}

impl Default for MemoryManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_conversation_message() {
        use crate::invocation::conversation::Role;
        let mut mgr = MemoryManager::new();
        mgr.push_conversation_message("user", "hello");
        mgr.push_conversation_message("assistant", "hi");
        assert_eq!(mgr.conversation_history.len(), 2);
        assert_eq!(mgr.conversation_history[0].role, Role::User);
        assert_eq!(mgr.conversation_history[0].content, "hello");
        assert_eq!(mgr.conversation_history[1].role, Role::Assistant);
        assert_eq!(mgr.conversation_history[1].content, "hi");
    }

    #[tokio::test]
    async fn build_memory_context_no_vfs_returns_none() {
        let mgr = MemoryManager::new();
        let ctx = mgr.build_memory_context_block(None, "alice", "myagent").await;
        assert!(ctx.is_none());
    }

    #[tokio::test]
    async fn auto_log_session_end_no_svc_no_panic() {
        let mgr = MemoryManager::new();
        // No memory_svc — should not panic
        mgr.auto_log_session_end(1, "agent", "user", "sess", &[]).await;
    }

    #[tokio::test]
    async fn auto_log_session_end_empty_history_skips() {
        let mgr = MemoryManager::new();
        // conversation_history is empty — log should be skipped silently
        mgr.auto_log_session_end(1, "agent", "user", "sess", &[]).await;
        // No assertion needed — just checking no panic
    }
}
