# Day 27 — `exec.svc` and `mcp-bridge.svc`

> **Goal:** Implement `exec.svc` (runtime discovery and code execution: Python, Node.js, Bash) and `mcp-bridge.svc` (MCP server proxy — translates MCP tool calls into IPC-compatible format). Both services implement the IPC service startup contract.

---

## Pre-flight: Verify Day 26

```bash
cargo test --workspace
grep -r "ATPCommand"    crates/avix-core/src/
grep -r "ATPTranslator" crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Module Setup

Add to `src/lib.rs`:
```rust
pub mod exec_svc;
pub mod mcp_bridge;
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/exec_svc.rs`:

```rust
use avix_core::exec_svc::{ExecService, ExecRequest, ExecResult};

// ── Python detection ──────────────────────────────────────────────────────────

#[tokio::test]
async fn detect_python_runtime() {
    let svc = ExecService::new();
    let available = svc.detect_runtimes().await;
    // At least one runtime must be present in CI
    assert!(!available.is_empty());
}

// ── Python execution ──────────────────────────────────────────────────────────

#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn exec_python_print_hello() {
    let svc = ExecService::new();
    if !svc.has_runtime("python3").await { return; } // skip if not installed

    let result = svc.exec(ExecRequest {
        runtime: "python3".into(),
        code:    "print('hello')".into(),
        timeout_sec: 5,
        env:     Default::default(),
    }).await.unwrap();

    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.trim() == "hello");
}

#[tokio::test]
async fn exec_bad_syntax_returns_nonzero() {
    let svc = ExecService::new();
    if !svc.has_runtime("python3").await { return; }

    let result = svc.exec(ExecRequest {
        runtime: "python3".into(),
        code:    "this is not valid python !!".into(),
        timeout_sec: 5,
        env:     Default::default(),
    }).await.unwrap();

    assert_ne!(result.exit_code, 0);
    assert!(!result.stderr.is_empty());
}

#[tokio::test]
async fn exec_timeout_kills_runaway_process() {
    let svc = ExecService::new();
    if !svc.has_runtime("python3").await { return; }

    let result = svc.exec(ExecRequest {
        runtime:     "python3".into(),
        code:        "import time; time.sleep(60)".into(),
        timeout_sec: 1,
        env:         Default::default(),
    }).await;

    assert!(result.is_err() || result.unwrap().exit_code != 0);
}

// ── Unsupported runtime ───────────────────────────────────────────────────────

#[tokio::test]
async fn exec_unknown_runtime_returns_error() {
    let svc = ExecService::new();
    let result = svc.exec(ExecRequest {
        runtime: "cobol".into(), code: "HELLO".into(),
        timeout_sec: 5, env: Default::default(),
    }).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("runtime"));
}
```

Create `crates/avix-core/tests/mcp_bridge.rs`:

```rust
use avix_core::mcp_bridge::{McpBridge, McpToolDescriptor};

// ── Tool descriptor translation ───────────────────────────────────────────────

#[test]
fn mcp_tool_descriptor_translates_to_avix_format() {
    let bridge = McpBridge::new_for_test("github-svc");
    let mcp_descriptor = serde_json::json!({
        "name": "list-prs",
        "description": "List open pull requests",
        "inputSchema": {
            "type": "object",
            "properties": {
                "state": {"type": "string"}
            },
            "required": ["state"]
        }
    });

    let avix_descriptor = bridge.translate_descriptor(&mcp_descriptor).unwrap();
    assert_eq!(avix_descriptor["name"], "mcp/github/list-prs");
    assert!(avix_descriptor["description"].as_str().unwrap().contains("pull request"));
}

// ── Namespace injection ───────────────────────────────────────────────────────

#[test]
fn mcp_tool_name_gets_namespace_prefix() {
    let bridge = McpBridge::new_for_test("github-svc");
    let descriptor = serde_json::json!({"name": "create-issue", "description": ".", "inputSchema": {}});
    let translated = bridge.translate_descriptor(&descriptor).unwrap();
    assert_eq!(translated["name"], "mcp/github/create-issue");
}

#[test]
fn different_server_names_produce_different_namespaces() {
    let github  = McpBridge::new_for_test("github-svc");
    let slack   = McpBridge::new_for_test("slack-svc");

    let d = serde_json::json!({"name": "post", "description": ".", "inputSchema": {}});
    let g = github.translate_descriptor(&d).unwrap();
    let s = slack.translate_descriptor(&d).unwrap();

    assert_eq!(g["name"], "mcp/github/post");
    assert_eq!(s["name"], "mcp/slack/post");
}

// ── Tool call forwarding ──────────────────────────────────────────────────────

#[test]
fn mcp_call_strips_namespace_before_forwarding() {
    let bridge = McpBridge::new_for_test("github-svc");
    let avix_call = serde_json::json!({
        "name": "mcp/github/list-prs",
        "args": {"state": "open"}
    });
    let mcp_call = bridge.translate_outbound_call(&avix_call).unwrap();
    assert_eq!(mcp_call["method"], "list-prs");
    assert_eq!(mcp_call["params"]["state"], "open");
}

// ── Tool count ────────────────────────────────────────────────────────────────

#[test]
fn bridge_registers_all_tools_from_server() {
    let bridge = McpBridge::new_for_test("github-svc");
    let tools = vec![
        serde_json::json!({"name": "list-prs", "description": ".", "inputSchema": {}}),
        serde_json::json!({"name": "create-issue", "description": ".", "inputSchema": {}}),
        serde_json::json!({"name": "close-issue", "description": ".", "inputSchema": {}}),
    ];
    let translated: Vec<_> = tools.iter()
        .map(|t| bridge.translate_descriptor(t).unwrap())
        .collect();
    assert_eq!(translated.len(), 3);
    assert!(translated.iter().all(|t| t["name"].as_str().unwrap().starts_with("mcp/github/")));
}
```

---

## Step 3 — Implement

`ExecService.exec` uses `tokio::process::Command` with a `timeout`. Runtimes detected by `which python3`, `which node`, `which bash`. `McpBridge` stores the server name and uses it to prefix tool names as `mcp/<server-short-name>/<tool>`.

---

## Step 4 — Verify

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-27: exec.svc — runtime detection, code exec, timeout; mcp-bridge namespace"
```

## Success Criteria

- [ ] `detect_runtimes` finds at least one runtime in CI
- [ ] Python print executes correctly
- [ ] Bad syntax returns non-zero exit code
- [ ] Timeout kills runaway process
- [ ] Unknown runtime returns error
- [ ] MCP tool names get `mcp/<server>/` prefix
- [ ] Outbound call strips namespace before forwarding
- [ ] All bridge tests pass
- [ ] 20+ tests pass, 0 clippy warnings

---
---

