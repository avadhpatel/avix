//! Structured trace output for debugging ATP traffic, agent turns, and notifications.
//!
//! Enabled by passing `--trace atp,agent,notifications` (or `--trace all`) to
//! `avix start`, `avix tui`, or `avix-web`. Each enabled category writes one
//! JSONL file to the logs directory:
//!
//!   `<log_dir>/trace-atp.jsonl`
//!   `<log_dir>/trace-agent.jsonl`
//!   `<log_dir>/trace-notifications.jsonl`
//!
//! All trace calls are no-ops when the corresponding flag is disabled.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

/// Which categories of trace output are enabled.
#[derive(Debug, Clone, Default)]
pub struct TraceFlags {
    /// Trace all inbound/outbound ATP frames through the gateway.
    pub atp: bool,
    /// Trace agent spawn, LLM calls, tool calls, and exits.
    pub agent: bool,
    /// Trace notifications (HIL requests, agent exits, sys alerts).
    pub notifications: bool,
}

impl TraceFlags {
    /// Parse from a comma-separated string.
    /// Recognised tokens: `atp`, `agent`, `notifications` (or `notif`), `all`.
    pub fn from_csv(s: &str) -> Self {
        let mut flags = Self::default();
        for part in s.split(',') {
            match part.trim() {
                "atp" => flags.atp = true,
                "agent" => flags.agent = true,
                "notifications" | "notif" => flags.notifications = true,
                "all" => {
                    flags.atp = true;
                    flags.agent = true;
                    flags.notifications = true;
                }
                other if !other.is_empty() => {
                    tracing::warn!(token = other, "unknown --trace token (ignored)");
                }
                _ => {}
            }
        }
        flags
    }

    pub fn is_any_enabled(&self) -> bool {
        self.atp || self.agent || self.notifications
    }
}

/// Async, non-blocking tracer that writes JSONL lines to per-category files.
///
/// When `flags.is_any_enabled()` is false this struct is a true no-op — no files
/// are created and every trace call returns immediately.
pub struct Tracer {
    pub flags: TraceFlags,
    log_dir: PathBuf,
}

impl Tracer {
    /// Create a tracer that writes to `log_dir`. The directory is created
    /// eagerly if any flag is enabled.
    pub fn new(flags: TraceFlags, log_dir: PathBuf) -> Arc<Self> {
        if flags.is_any_enabled() {
            let _ = std::fs::create_dir_all(&log_dir);
            tracing::info!(
                log_dir = %log_dir.display(),
                atp = flags.atp,
                agent = flags.agent,
                notifications = flags.notifications,
                "trace enabled"
            );
        }
        Arc::new(Self { flags, log_dir })
    }

    /// A tracer with all flags disabled. Every call is a no-op.
    pub fn noop() -> Arc<Self> {
        Arc::new(Self {
            flags: TraceFlags::default(),
            log_dir: PathBuf::new(),
        })
    }

    // ── internal helpers ──────────────────────────────────────────────────────

    fn ts() -> String {
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    /// Append one JSON line to `<log_dir>/<filename>` in a background task.
    fn emit(&self, filename: &'static str, event: Value) {
        let path = self.log_dir.join(filename);
        tokio::spawn(async move {
            let line = format!("{}\n", event);
            if let Ok(mut f) = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await
            {
                let _ = f.write_all(line.as_bytes()).await;
            }
        });
    }

    // ── ATP tracing ───────────────────────────────────────────────────────────

    /// Trace a raw inbound ATP frame received from a WebSocket client.
    pub fn atp_inbound(&self, session_id: &str, identity: &str, raw_frame: &str) {
        if !self.flags.atp {
            return;
        }
        // Parse to Value so the JSONL is structured rather than escaped.
        let frame = serde_json::from_str::<Value>(raw_frame).unwrap_or_else(|_| json!(raw_frame));
        self.emit(
            "trace-atp.jsonl",
            json!({
                "ts": Self::ts(),
                "kind": "atp.inbound",
                "session": session_id,
                "identity": identity,
                "frame": frame,
            }),
        );
    }

    /// Trace a raw outbound ATP reply or event sent back to a WebSocket client.
    pub fn atp_outbound(&self, session_id: &str, raw_msg: &str) {
        if !self.flags.atp {
            return;
        }
        let msg = serde_json::from_str::<Value>(raw_msg).unwrap_or_else(|_| json!(raw_msg));
        self.emit(
            "trace-atp.jsonl",
            json!({
                "ts": Self::ts(),
                "kind": "atp.outbound",
                "session": session_id,
                "msg": msg,
            }),
        );
    }

    /// Trace an event pushed from the event bus to a client session.
    pub fn atp_event(&self, session_id: &str, event_kind: &str, raw_event: &str) {
        if !self.flags.atp {
            return;
        }
        let evt = serde_json::from_str::<Value>(raw_event).unwrap_or_else(|_| json!(raw_event));
        self.emit(
            "trace-atp.jsonl",
            json!({
                "ts": Self::ts(),
                "kind": "atp.event",
                "session": session_id,
                "event_kind": event_kind,
                "event": evt,
            }),
        );
    }

    // ── Agent tracing ─────────────────────────────────────────────────────────

    /// Trace agent spawn.
    pub fn agent_spawn(&self, pid: u64, name: &str, goal: &str, session_id: &str) {
        if !self.flags.agent {
            return;
        }
        self.emit(
            "trace-agent.jsonl",
            json!({
                "ts": Self::ts(),
                "kind": "agent.spawn",
                "pid": pid,
                "name": name,
                "goal": goal,
                "session": session_id,
            }),
        );
    }

    /// Trace an LLM call being sent (before the response arrives).
    pub fn agent_llm_call(
        &self,
        pid: u64,
        turn: u32,
        model: &str,
        message_count: usize,
        tool_count: usize,
    ) {
        if !self.flags.agent {
            return;
        }
        self.emit(
            "trace-agent.jsonl",
            json!({
                "ts": Self::ts(),
                "kind": "agent.llm_call",
                "pid": pid,
                "turn": turn,
                "model": model,
                "message_count": message_count,
                "tool_count": tool_count,
            }),
        );
    }

    /// Trace an LLM response received.
    pub fn agent_llm_response(
        &self,
        pid: u64,
        turn: u32,
        stop_reason: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        if !self.flags.agent {
            return;
        }
        self.emit(
            "trace-agent.jsonl",
            json!({
                "ts": Self::ts(),
                "kind": "agent.llm_response",
                "pid": pid,
                "turn": turn,
                "stop_reason": stop_reason,
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
            }),
        );
    }

    /// Trace a tool call dispatched by the executor.
    pub fn agent_tool_call(&self, pid: u64, call_id: &str, tool: &str, params: &Value) {
        if !self.flags.agent {
            return;
        }
        self.emit(
            "trace-agent.jsonl",
            json!({
                "ts": Self::ts(),
                "kind": "agent.tool_call",
                "pid": pid,
                "call_id": call_id,
                "tool": tool,
                "params": params,
            }),
        );
    }

    /// Trace a tool result returned to the executor.
    pub fn agent_tool_result(&self, pid: u64, call_id: &str, tool: &str, result: &Value) {
        if !self.flags.agent {
            return;
        }
        self.emit(
            "trace-agent.jsonl",
            json!({
                "ts": Self::ts(),
                "kind": "agent.tool_result",
                "pid": pid,
                "call_id": call_id,
                "tool": tool,
                "result": result,
            }),
        );
    }

    /// Trace agent exit.
    pub fn agent_exit(&self, pid: u64, status: &str, exit_reason: Option<&str>) {
        if !self.flags.agent {
            return;
        }
        self.emit(
            "trace-agent.jsonl",
            json!({
                "ts": Self::ts(),
                "kind": "agent.exit",
                "pid": pid,
                "status": status,
                "exit_reason": exit_reason,
            }),
        );
    }

    // ── Notification tracing ──────────────────────────────────────────────────

    /// Trace a notification being added to the store.
    pub fn notification_added(&self, kind: &str, id: &str, message: &str, agent_pid: Option<u64>) {
        if !self.flags.notifications {
            return;
        }
        self.emit(
            "trace-notifications.jsonl",
            json!({
                "ts": Self::ts(),
                "kind": "notification.added",
                "notif_kind": kind,
                "id": id,
                "message": message,
                "agent_pid": agent_pid,
            }),
        );
    }

    /// Trace a HIL request event.
    pub fn hil_request(&self, pid: u64, hil_id: &str, prompt: &str) {
        if !self.flags.notifications {
            return;
        }
        self.emit(
            "trace-notifications.jsonl",
            json!({
                "ts": Self::ts(),
                "kind": "notification.hil_request",
                "pid": pid,
                "hil_id": hil_id,
                "prompt": prompt,
            }),
        );
    }

    /// Trace a HIL being resolved (approved or denied).
    pub fn hil_resolved(&self, hil_id: &str, outcome: &str) {
        if !self.flags.notifications {
            return;
        }
        self.emit(
            "trace-notifications.jsonl",
            json!({
                "ts": Self::ts(),
                "kind": "notification.hil_resolved",
                "hil_id": hil_id,
                "outcome": outcome,
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::time::{sleep, Duration};

    #[test]
    fn parse_csv_individual_flags() {
        let f = TraceFlags::from_csv("atp,agent");
        assert!(f.atp);
        assert!(f.agent);
        assert!(!f.notifications);
    }

    #[test]
    fn parse_all_enables_all() {
        let f = TraceFlags::from_csv("all");
        assert!(f.atp && f.agent && f.notifications);
    }

    #[test]
    fn parse_notif_alias() {
        let f = TraceFlags::from_csv("notif");
        assert!(f.notifications);
        assert!(!f.atp);
    }

    #[test]
    fn noop_tracer_is_disabled() {
        let t = Tracer::noop();
        assert!(!t.flags.is_any_enabled());
    }

    #[tokio::test]
    async fn atp_inbound_writes_jsonl() {
        let dir = TempDir::new().unwrap();
        let flags = TraceFlags::from_csv("atp");
        let tracer = Tracer::new(flags, dir.path().to_path_buf());
        tracer.atp_inbound("sess-1", "alice", r#"{"type":"cmd","id":"r1"}"#);
        sleep(Duration::from_millis(50)).await; // let the spawned task flush
        let content = std::fs::read_to_string(dir.path().join("trace-atp.jsonl")).unwrap();
        assert!(content.contains("atp.inbound"));
        assert!(content.contains("alice"));
    }

    #[tokio::test]
    async fn agent_tool_call_writes_jsonl() {
        let dir = TempDir::new().unwrap();
        let tracer = Tracer::new(TraceFlags::from_csv("agent"), dir.path().to_path_buf());
        tracer.agent_tool_call(42, "call-1", "fs/read", &json!({"path": "/tmp/f"}));
        sleep(Duration::from_millis(50)).await;
        let content = std::fs::read_to_string(dir.path().join("trace-agent.jsonl")).unwrap();
        assert!(content.contains("agent.tool_call"));
        assert!(content.contains("fs/read"));
    }

    #[tokio::test]
    async fn noop_tracer_writes_nothing() {
        let dir = TempDir::new().unwrap();
        let tracer = Tracer::noop();
        tracer.atp_inbound("s", "u", "{}");
        tracer.agent_spawn(1, "bot", "goal", "sess");
        sleep(Duration::from_millis(50)).await;
        assert!(dir.path().join("trace-atp.jsonl").try_exists().unwrap() == false);
    }
}
