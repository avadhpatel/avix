# Dev Plan: Cat1 fs/* Tool Handlers (items 0b + 0c)

**Status:** COMPLETE
**Priority:** P1 — Cat1 0b+0c from TODO.md

---

## Problem Summary

Agents cannot read or write files. The VFS is operational internally but `fs/read`,
`fs/write`, `fs/list`, `fs/exists`, and `fs/delete` are not registered in the tool
registry, so `refresh_tool_list` never includes them and the LLM cannot call them.

Two changes are needed (implement together):
- **0c**: `KernelIpcServer` has no `fs/*` handlers despite being the right home (has
  `proc_handler` which has `VfsRouter` — actually VfsRouter is on `Runtime`, not
  `ProcHandler`). Add `vfs: Arc<VfsRouter>` to `KernelIpcServer` and add `fs/*` match
  arms to `dispatch_request`.
- **0b**: Register `fs/*` tools in the tool registry with `endpoint: "kernel"` so
  `dispatch_cat1_tool` routes them to `kernel.sock`.

---

## Architecture References

- `docs/architecture/09-runtime-executor-tools.md` § "Category 1 Direct Service Tools"
- `crates/avix-core/src/executor/ipc_dispatch.rs` — `dispatch_cat1_tool` resolves
  `endpoint: "kernel"` → `AVIX_KERNEL_SOCK` env var or `runtime_dir/kernel.sock`

---

## How Cat1 → kernel.sock routing works

`dispatch_cat1_tool` extracts `ipc.endpoint` from the tool descriptor and builds:
```
env_key = "AVIX_KERNEL_SOCK"          // for endpoint "kernel"
socket  = runtime_dir/kernel.sock     // fallback
```
So registering `fs/*` tools with `endpoint: "kernel"` automatically routes them to the
running `KernelIpcServer`. No additional routing is needed.

---

## Files to Change

| # | File | Change |
|---|------|--------|
| 1 | `crates/avix-core/src/kernel/ipc_server.rs` | Add `vfs` field + `fs/*` dispatch handlers |
| 2 | `crates/avix-core/src/bootstrap/mod.rs` | Pass VFS to `KernelIpcServer::new`; register `fs/*` tools |

---

## Implementation Order

### Step 1 — `kernel/ipc_server.rs`: add VfsRouter + fs/* handlers

**Struct change:**
```rust
// BEFORE
pub struct KernelIpcServer {
    sock_path: PathBuf,
    proc_handler: Arc<ProcHandler>,
    avix_root: PathBuf,
}

// AFTER
pub struct KernelIpcServer {
    sock_path: PathBuf,
    proc_handler: Arc<ProcHandler>,
    avix_root: PathBuf,
    vfs: Arc<VfsRouter>,
}
```

**`new()` change:**
```rust
// BEFORE
pub fn new(sock_path: PathBuf, proc_handler: Arc<ProcHandler>, avix_root: PathBuf) -> Self {
    Self { sock_path, proc_handler, avix_root }
}

// AFTER
pub fn new(
    sock_path: PathBuf,
    proc_handler: Arc<ProcHandler>,
    avix_root: PathBuf,
    vfs: Arc<VfsRouter>,
) -> Self {
    Self { sock_path, proc_handler, avix_root, vfs }
}
```

**`start()` — thread vfs through closures:**
```rust
// Add vfs to the captured variables
let proc_handler = Arc::clone(&self.proc_handler);
let avix_root = self.avix_root;
let vfs = Arc::clone(&self.vfs);        // NEW
tokio::spawn(async move {
    if let Err(e) = server
        .serve(move |msg| {
            let ph = Arc::clone(&proc_handler);
            let root = avix_root.clone();
            let vfs = Arc::clone(&vfs);    // NEW
            async move { handle_message(msg, ph, root, vfs).await }
        })
        .await
    { ... }
});
```

**`handle_message()` — add vfs parameter and thread to dispatch:**
```rust
async fn handle_message(
    msg: IpcMessage,
    proc_handler: Arc<ProcHandler>,
    avix_root: PathBuf,
    vfs: Arc<VfsRouter>,                                    // NEW
) -> Option<JsonRpcResponse> {
    match msg {
        IpcMessage::Request(req) => {
            debug!(method = %req.method, id = %req.id, "kernel IPC request");
            let resp = dispatch_request(
                &req.id, &req.method, req.params,
                proc_handler, avix_root,
                vfs,                                         // NEW
            ).await;
            Some(resp)
        }
        IpcMessage::Notification(notif) => {
            debug!(method = %notif.method, "kernel IPC notification (ignored)");
            None
        }
    }
}
```

**`dispatch_request()` — add vfs parameter + fs/* arms:**

Add `vfs: Arc<VfsRouter>` to the signature. Add the following match arms before the
final `other =>` catch-all:

```rust
"fs/read" => {
    let raw_path = params["path"].as_str().unwrap_or("");
    match VfsPath::parse(raw_path) {
        Err(e) => JsonRpcResponse::err(id, -32602, &e.to_string(), None),
        Ok(path) => match vfs.read(&path).await {
            Ok(bytes) => {
                match String::from_utf8(bytes.clone()) {
                    Ok(text) => JsonRpcResponse::ok(
                        id,
                        json!({ "content": text, "encoding": "utf-8" }),
                    ),
                    Err(_) => JsonRpcResponse::ok(
                        id,
                        json!({
                            "content": base64_encode(&bytes),
                            "encoding": "base64",
                        }),
                    ),
                }
            }
            Err(e) => {
                warn!(path = raw_path, error = %e, "fs/read failed");
                JsonRpcResponse::err(id, -32000, &e.to_string(), None)
            }
        },
    }
}

"fs/write" => {
    let raw_path = params["path"].as_str().unwrap_or("");
    let content = params["content"].as_str().unwrap_or("");
    match VfsPath::parse(raw_path) {
        Err(e) => JsonRpcResponse::err(id, -32602, &e.to_string(), None),
        Ok(path) => match vfs.write(&path, content.as_bytes().to_vec()).await {
            Ok(()) => {
                tracing::debug!(path = raw_path, "fs/write succeeded");
                JsonRpcResponse::ok(id, json!({ "ok": true }))
            }
            Err(e) => {
                warn!(path = raw_path, error = %e, "fs/write failed");
                JsonRpcResponse::err(id, -32000, &e.to_string(), None)
            }
        },
    }
}

"fs/list" => {
    let raw_path = params["path"].as_str().unwrap_or("");
    match VfsPath::parse(raw_path) {
        Err(e) => JsonRpcResponse::err(id, -32602, &e.to_string(), None),
        Ok(path) => match vfs.list(&path).await {
            Ok(entries) => JsonRpcResponse::ok(id, json!({ "entries": entries })),
            Err(e) => {
                warn!(path = raw_path, error = %e, "fs/list failed");
                JsonRpcResponse::err(id, -32000, &e.to_string(), None)
            }
        },
    }
}

"fs/exists" => {
    let raw_path = params["path"].as_str().unwrap_or("");
    match VfsPath::parse(raw_path) {
        Err(e) => JsonRpcResponse::err(id, -32602, &e.to_string(), None),
        Ok(path) => {
            let exists = vfs.exists(&path).await;
            JsonRpcResponse::ok(id, json!({ "exists": exists }))
        }
    }
}

"fs/delete" => {
    let raw_path = params["path"].as_str().unwrap_or("");
    match VfsPath::parse(raw_path) {
        Err(e) => JsonRpcResponse::err(id, -32602, &e.to_string(), None),
        Ok(path) => match vfs.delete(&path).await {
            Ok(()) => {
                tracing::debug!(path = raw_path, "fs/delete succeeded");
                JsonRpcResponse::ok(id, json!({ "ok": true }))
            }
            Err(e) => {
                warn!(path = raw_path, error = %e, "fs/delete failed");
                JsonRpcResponse::err(id, -32000, &e.to_string(), None)
            }
        },
    }
}
```

**base64 helper** — add a private `base64_encode` function in the file:
```rust
/// Encode bytes as base64 (URL-safe, no padding).
fn base64_encode(data: &[u8]) -> String {
    use std::fmt::Write;
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    ...
}
```

Actually, use the `base64` crate if already in Cargo.toml, else implement inline or
use `STANDARD` from the `base64` crate. Check Cargo.toml first.

**Imports to add:**
```rust
use crate::memfs::{VfsPath, VfsRouter};
```

**Compile check:** `cargo check --package avix-core`
**Tests to add:** see Tests section below.

---

### Step 2 — `bootstrap/mod.rs`: pass VFS + register fs/* tools

**Update the `KernelIpcServer::new` call** in `phase2_kernel` (currently line 295–296):
```rust
// BEFORE
KernelIpcServer::new(self.kernel_sock.clone(), proc_handler, self.root.clone())

// AFTER
KernelIpcServer::new(
    self.kernel_sock.clone(),
    proc_handler,
    self.root.clone(),
    Arc::clone(&self.vfs),
)
```

`self.vfs` is an `Arc<VfsRouter>` available from `bootstrap_with_root` (created in
Phase 0 before `phase2_kernel` runs).

**Register fs/* tools in `phase3_services`** — add after exec.svc registration block:
```rust
// Register fs/* Cat1 tools. IPC binding: endpoint "kernel" resolves to
// AVIX_KERNEL_SOCK env var or runtime_dir/kernel.sock at dispatch time.
let fs_tool_defs: &[(&str, &str)] = &[
    ("fs/read",   "Read the contents of a file in the VFS"),
    ("fs/write",  "Write content to a file in the VFS"),
    ("fs/list",   "List entries in a VFS directory"),
    ("fs/exists", "Check whether a path exists in the VFS"),
    ("fs/delete", "Delete a file from the VFS"),
];
let mut fs_entries = Vec::new();
for (name, desc) in fs_tool_defs {
    if let Ok(tool_name) = ToolName::parse(name) {
        let descriptor = serde_json::json!({
            "name": name,
            "description": desc,
            "ipc": {
                "transport": "local-ipc",
                "endpoint": "kernel",
                "method": name,
            }
        });
        fs_entries.push(ToolEntry::new(
            tool_name,
            "kernel".to_string(),
            ToolState::Available,
            ToolVisibility::All,
            descriptor,
        ));
    }
}
if let Err(e) = tool_registry.add("kernel", fs_entries).await {
    tracing::warn!(error = %e, "failed to register fs/* tools");
} else {
    tracing::info!("registered {} fs/* tools in tool registry", fs_tool_defs.len());
}
```

**Compile check:** `cargo check --package avix-core`

---

## Tests

### Step 1 tests — in `kernel/ipc_server.rs` test module

Add to the existing `#[cfg(test)]` module:

**`fs_read_missing_file_returns_error`** — build a `VfsRouter`, call
`dispatch_request` with method `"fs/read"` and `params = {"path": "/no/such/file"}`,
assert the response has `error.code == -32000`.

**`fs_read_existing_file_returns_content`** — write a file into a VfsRouter via
`vfs.write()`, then dispatch `fs/read` for the same path, assert `result.content`
equals the written content.

**`fs_write_then_read_roundtrip`** — dispatch `fs/write` with a path and content,
then dispatch `fs/read` for the same path, assert content matches.

**`fs_list_dir_returns_entries`** — write two files under `/tmp/test/a` and
`/tmp/test/b` (use `ensure_dir` if needed), dispatch `fs/list` for `/tmp/test`,
assert the `entries` array contains both filenames.

**`fs_exists_true_and_false`** — write a file, assert `fs/exists` returns
`{"exists":true}` for it; assert `{"exists":false}` for a non-existent path.

**`fs_delete_removes_file`** — write a file, dispatch `fs/delete`, then dispatch
`fs/exists`, assert `{"exists":false}`.

**`fs_read_invalid_path_returns_parse_error`** — dispatch `fs/read` with
`params = {"path": "relative/path"}` (missing leading `/`), assert response has
`error.code == -32602`.

Test filter: `cargo test --package avix-core kernel::ipc_server`

### Step 2 tests — in `bootstrap/mod.rs` test module

Extend the existing `#[cfg(test)]` module with:

**`fs_tools_registered_with_kernel_ipc_binding`** — create a `ToolRegistry`, run the
same registration loop as `phase3_services` for `fs_tool_defs`, call
`tool_registry.lookup("fs/read")` and assert `ipc.endpoint == "kernel"`.

Test filter: `cargo test --package avix-core bootstrap`

---

## Expected Outcome After Both Steps

1. `tool_registry.lookup("fs/read")` → `Ok(entry)` with `ipc.endpoint == "kernel"`
2. LLM tool list includes all 5 `fs/*` tools
3. Agent calls `fs/write` with `{"path":"/tmp/hello.txt","content":"hi"}` → `{"ok":true}`
4. Agent calls `fs/read` with `{"path":"/tmp/hello.txt"}` → `{"content":"hi","encoding":"utf-8"}`
5. Agents can read their own `/proc/<pid>/status.yaml` and explore the VFS
