use std::collections::BTreeSet;

use tracing::{debug, instrument};

use crate::router::ALWAYS_PRESENT;
use crate::syscall::SyscallRegistry;
use crate::tool_registry::ToolRegistry;

/// Maps manifest `requestedCapabilities` strings to concrete tool names.
///
/// # Capability group format
///
/// `<namespace>:<filter>`
///
/// - `filter == "*"` → include all tools/syscalls in the namespace.
/// - `filter != "*"` → use `<namespace>/<filter>` as a prefix; include only
///   tools whose name starts with that prefix.
///
/// ## ToolRegistry matching
///
/// Prefix = `"<ns>/"` (wildcard) or `"<ns>/<filter>"` (specific).
/// Matches any tool whose name starts with the prefix.  This handles both flat
/// names (`fs/read`) and arbitrarily nested names (`workspace/project/list`).
///
/// ## SyscallRegistry matching
///
/// - `kernel:*` → all syscalls.
/// - `kernel:<filter>` → syscalls whose `domain == filter` (e.g. `kernel:proc`
///   → syscalls in domain `proc`).
/// - `<ns>:*` or `<ns>:<filter>` (ns ≠ `"kernel"`) → syscalls whose
///   `domain == ns`.  The filter is applied as a name prefix
///   `kernel/<ns>/<filter>` when not `*`.
///
/// ## Always-present tools
///
/// `cap/request-tool`, `cap/escalate`, `cap/list`, `job/watch` are always
/// appended regardless of what the manifest requests.
pub struct CapabilityResolver<'a> {
    tool_registry: &'a ToolRegistry,
    syscall_registry: &'a SyscallRegistry,
}

impl<'a> CapabilityResolver<'a> {
    pub fn new(tool_registry: &'a ToolRegistry, syscall_registry: &'a SyscallRegistry) -> Self {
        Self {
            tool_registry,
            syscall_registry,
        }
    }

    /// Resolve a list of capability group strings into concrete tool names.
    ///
    /// The result is deduplicated and sorted. `ALWAYS_PRESENT` tools are
    /// always included.
    #[instrument(skip(self))]
    pub async fn resolve(&self, capabilities: &[String]) -> Vec<String> {
        let mut granted: BTreeSet<String> = BTreeSet::new();

        let all_tools = self.tool_registry.get_all_entries().await;
        let all_syscalls = self.syscall_registry.list();

        for cap in capabilities {
            let (ns, filter) = cap.split_once(':').unwrap_or((cap.as_str(), "*"));

            // ── SyscallRegistry ───────────────────────────────────────────
            if ns == "kernel" {
                for syscall in all_syscalls {
                    if filter == "*" || syscall.domain == filter {
                        granted.insert(syscall.name.clone());
                    }
                }
            } else {
                // e.g. `proc:*` → domain "proc"; `proc:spawn` → domain "proc"
                // and name starts with "kernel/proc/spawn"
                for syscall in all_syscalls {
                    if syscall.domain == ns {
                        if filter == "*" {
                            granted.insert(syscall.name.clone());
                        } else {
                            let name_prefix = format!("kernel/{}/{}", ns, filter);
                            if syscall.name.starts_with(&name_prefix) {
                                granted.insert(syscall.name.clone());
                            }
                        }
                    }
                }
            }

            // ── ToolRegistry ──────────────────────────────────────────────
            let prefix = if filter == "*" {
                format!("{}/", ns)
            } else {
                format!("{}/{}", ns, filter)
            };
            let mut matched = 0usize;
            for tool in &all_tools {
                if tool.name.as_str().starts_with(&prefix) {
                    granted.insert(tool.name.as_str().to_string());
                    matched += 1;
                }
            }
            if matched == 0 {
                debug!(cap, "capability group matched no registered tools");
            }
        }

        // Always-present tools are unconditionally granted.
        for &tool in ALWAYS_PRESENT {
            granted.insert(tool.to_string());
        }

        granted.into_iter().collect()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::syscall::SyscallRegistry;
    use crate::tool_registry::{ToolRegistry, entry::ToolEntry};
    use crate::types::tool::{ToolName, ToolState, ToolVisibility};

    fn make_tool(name: &str) -> ToolEntry {
        ToolEntry::new(
            ToolName::parse(name).unwrap(),
            "test".to_string(),
            ToolState::Available,
            ToolVisibility::All,
            serde_json::Value::Null,
        )
    }

    async fn registry_with(names: &[&str]) -> Arc<ToolRegistry> {
        let reg = Arc::new(ToolRegistry::new());
        let entries = names.iter().map(|n| make_tool(n)).collect();
        reg.add("test", entries).await.unwrap();
        reg
    }

    async fn resolve(reg: &ToolRegistry, caps: &[&str]) -> Vec<String> {
        let syscall_reg = SyscallRegistry::new();
        let resolver = CapabilityResolver::new(reg, &syscall_reg);
        resolver
            .resolve(&caps.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            .await
    }

    // T-CR-01
    #[tokio::test]
    async fn resolve_fs_star_matches_fs_tools() {
        let reg = registry_with(&["fs/read", "fs/write", "llm/complete"]).await;
        let tools = resolve(&reg, &["fs:*"]).await;
        assert!(tools.contains(&"fs/read".to_string()));
        assert!(tools.contains(&"fs/write".to_string()));
        assert!(!tools.contains(&"llm/complete".to_string()));
    }

    // T-CR-02
    #[tokio::test]
    async fn resolve_kernel_star_matches_all_syscalls() {
        let reg = registry_with(&[]).await;
        let syscall_reg = SyscallRegistry::new();
        let all_syscall_names: Vec<String> =
            syscall_reg.list().iter().map(|s| s.name.clone()).collect();
        let resolver = CapabilityResolver::new(&reg, &syscall_reg);
        let tools = resolver.resolve(&["kernel:*".to_string()]).await;
        // Every registered syscall name must appear in the result
        for name in &all_syscall_names {
            assert!(tools.contains(name), "missing syscall {name}");
        }
    }

    // T-CR-03
    #[tokio::test]
    async fn resolve_kernel_proc_filter_matches_proc_domain() {
        let reg = registry_with(&[]).await;
        let syscall_reg = SyscallRegistry::new();
        let expected: Vec<String> = syscall_reg
            .list()
            .iter()
            .filter(|s| s.domain == "proc")
            .map(|s| s.name.clone())
            .collect();
        let resolver = CapabilityResolver::new(&reg, &syscall_reg);
        let tools = resolver.resolve(&["kernel:proc".to_string()]).await;
        assert!(!expected.is_empty());
        for name in &expected {
            assert!(tools.contains(name), "missing proc syscall {name}");
        }
        // No non-proc syscalls in result (ignoring ALWAYS_PRESENT)
        let ap: std::collections::HashSet<&str> = ALWAYS_PRESENT.iter().copied().collect();
        for tool in &tools {
            if !ap.contains(tool.as_str()) {
                let is_proc = syscall_reg
                    .list()
                    .iter()
                    .any(|s| s.name == *tool && s.domain == "proc");
                assert!(is_proc, "unexpected non-proc tool in result: {tool}");
            }
        }
    }

    // T-CR-04
    #[tokio::test]
    async fn resolve_proc_star_matches_proc_domain_syscalls() {
        let reg = registry_with(&[]).await;
        let syscall_reg = SyscallRegistry::new();
        let resolver = CapabilityResolver::new(&reg, &syscall_reg);
        let by_kernel = resolver.resolve(&["kernel:proc".to_string()]).await;
        let by_proc = resolver.resolve(&["proc:*".to_string()]).await;
        // Both forms yield the same set of tools
        assert_eq!(by_kernel, by_proc);
    }

    // T-CR-05
    #[tokio::test]
    async fn resolve_nested_tool_names() {
        let reg = registry_with(&[
            "workspace/project/list",
            "workspace/project/create",
            "workspace/create-project",
        ])
        .await;

        // workspace:project → prefix "workspace/project" — matches first two
        let project_tools = resolve(&reg, &["workspace:project"]).await;
        assert!(project_tools.contains(&"workspace/project/list".to_string()));
        assert!(project_tools.contains(&"workspace/project/create".to_string()));
        assert!(!project_tools.contains(&"workspace/create-project".to_string()));

        // workspace:* → prefix "workspace/" — matches all three
        let all_tools = resolve(&reg, &["workspace:*"]).await;
        assert!(all_tools.contains(&"workspace/project/list".to_string()));
        assert!(all_tools.contains(&"workspace/project/create".to_string()));
        assert!(all_tools.contains(&"workspace/create-project".to_string()));
    }

    // T-CR-06
    #[tokio::test]
    async fn resolve_empty_capabilities_returns_only_always_present() {
        let reg = registry_with(&["fs/read", "llm/complete"]).await;
        let tools = resolve(&reg, &[]).await;
        // Only ALWAYS_PRESENT — no service tools
        for ap in ALWAYS_PRESENT {
            assert!(tools.contains(&ap.to_string()));
        }
        assert!(!tools.contains(&"fs/read".to_string()));
        assert!(!tools.contains(&"llm/complete".to_string()));
        assert_eq!(tools.len(), ALWAYS_PRESENT.len());
    }

    // T-CR-07
    #[tokio::test]
    async fn resolve_deduplicates_overlapping_groups() {
        let reg = registry_with(&["fs/read", "fs/write"]).await;
        // Both groups expand to the same tools
        let tools = resolve(&reg, &["fs:*", "fs:read"]).await;
        let count = tools.iter().filter(|t| *t == "fs/read").count();
        assert_eq!(count, 1, "fs/read should appear exactly once");
    }

    // T-CR-08
    #[tokio::test]
    async fn always_present_tools_always_included() {
        let reg = registry_with(&[]).await;
        // No capabilities requested — ALWAYS_PRESENT still granted
        let tools = resolve(&reg, &[]).await;
        for ap in ALWAYS_PRESENT {
            assert!(
                tools.contains(&ap.to_string()),
                "ALWAYS_PRESENT tool {ap} missing"
            );
        }
    }
}
