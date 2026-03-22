# Filesystem Gap D — VFS Write Protection: `/proc/` and `/kernel/` Are Kernel-Only

> **Finding:** `MemFs::write()` accepts any path. The spec mandates that agents and users
> can never write into `/proc/` or `/kernel/`. These trees are kernel-owned; violations must
> return `EPERM`.
>
> **Architecture note:** `MemFs` is a kernel-internal primitive — only kernel code ever
> calls it directly. The right enforcement layer is therefore NOT `MemFs::write()` itself
> (that would break legitimate kernel writes), but the **syscall handler** (`kernel/fs/write`)
> which is the sole entry point through which agent code reaches the VFS. The syscall
> handler checks the requested path and rejects writes to protected trees before calling
> `MemFs::write`.
>
> Additionally, `MemFs` should offer an `assert_writable_by_agent` helper that the syscall
> handler calls, so the path logic is centralised in one place and can be independently tested.

---

## Protected trees (agent writes blocked)

| Tree | Rule |
|---|---|
| `/proc/` | Kernel-only writes. Agents may read (via `kernel/fs/read` with appropriate tool grant), never write. |
| `/kernel/` | Kernel-only. No read or write via syscall from agents. |
| `/secrets/` | Already enforced in `SecretsStore::vfs_read()` — this is a reminder not to add a VFS bypass. |
| `/etc/avix/` | Read-only via VFS for agents. Write requires root role and is not granted via the tool system. |
| `/bin/` | Read-only for agents (execute via spawn, not raw write). |

---

## Step 1 — Write Tests First

### 1a. Unit test on `MemFs` helper

Add to `crates/avix-core/tests/memfs.rs`:

```rust
// ── Finding D: path protection helper ────────────────────────────────────────

#[test]
fn agent_writable_path_allows_user_workspace() {
    use avix_core::memfs::VfsPath;
    assert!(VfsPath::parse("/users/alice/workspace/file.txt").unwrap().is_agent_writable());
}

#[test]
fn agent_writable_path_allows_services_workspace() {
    use avix_core::memfs::VfsPath;
    assert!(VfsPath::parse("/services/svc-pipeline/workspace/out.txt").unwrap().is_agent_writable());
}

#[test]
fn agent_writable_path_allows_crews_shared() {
    use avix_core::memfs::VfsPath;
    assert!(VfsPath::parse("/crews/researchers/shared/report.md").unwrap().is_agent_writable());
}

#[test]
fn agent_writable_blocks_proc() {
    use avix_core::memfs::VfsPath;
    assert!(!VfsPath::parse("/proc/57/status.yaml").unwrap().is_agent_writable());
}

#[test]
fn agent_writable_blocks_kernel() {
    use avix_core::memfs::VfsPath;
    assert!(!VfsPath::parse("/kernel/defaults/agent.yaml").unwrap().is_agent_writable());
}

#[test]
fn agent_writable_blocks_secrets() {
    use avix_core::memfs::VfsPath;
    assert!(!VfsPath::parse("/secrets/alice/openai-key.enc").unwrap().is_agent_writable());
}

#[test]
fn agent_writable_blocks_etc_avix() {
    use avix_core::memfs::VfsPath;
    assert!(!VfsPath::parse("/etc/avix/kernel.yaml").unwrap().is_agent_writable());
}

#[test]
fn agent_writable_blocks_bin() {
    use avix_core::memfs::VfsPath;
    assert!(!VfsPath::parse("/bin/researcher/manifest.yaml").unwrap().is_agent_writable());
}
```

### 1b. Integration test via syscall handler (`kernel/fs/write`)

Add to `crates/avix-core/tests/syscalls.rs` (or wherever `SyscallHandler` tests live):

```rust
// ── Finding D: fs/write enforcement ──────────────────────────────────────────

#[tokio::test]
async fn fs_write_to_proc_returns_eperm() {
    let handler = SyscallHandler::new_for_test();
    // Even admin token cannot write to /proc/ — it is kernel-only
    let result = handler.call(
        "kernel/fs/write",
        json!({"path": "/proc/57/status.yaml", "content": "tamper"}),
        &admin_token(), Pid::new(57), "alice",
    ).await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string().to_lowercase();
    assert!(msg.contains("eperm") || msg.contains("permission") || msg.contains("forbidden"),
        "write to /proc/ must return EPERM, got: {msg}");
}

#[tokio::test]
async fn fs_write_to_kernel_returns_eperm() {
    let handler = SyscallHandler::new_for_test();
    let result = handler.call(
        "kernel/fs/write",
        json!({"path": "/kernel/defaults/agent.yaml", "content": "tamper"}),
        &admin_token(), Pid::new(57), "alice",
    ).await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string().to_lowercase();
    assert!(msg.contains("eperm") || msg.contains("permission") || msg.contains("forbidden"),
        "write to /kernel/ must return EPERM, got: {msg}");
}

#[tokio::test]
async fn fs_write_to_user_workspace_succeeds() {
    let handler = SyscallHandler::new_for_test();
    let result = handler.call(
        "kernel/fs/write",
        json!({"path": "/users/alice/workspace/notes.txt", "content": "hello"}),
        &admin_token(), Pid::new(57), "alice",
    ).await;
    assert!(result.is_ok(), "write to /users/alice/workspace/ must succeed: {:?}", result);
}

#[tokio::test]
async fn fs_read_of_proc_status_succeeds_if_file_exists() {
    // Agents CAN read /proc/<pid>/status.yaml (read-only) — only writes are blocked
    let handler = SyscallHandler::new_for_test();
    // First spawn an agent to create the file
    let spawn_result = handler.call(
        "kernel/proc/spawn",
        json!({"agent": "test-reader", "goal": "g", "capabilities": []}),
        &admin_token(), Pid::new(0), "alice",
    ).await.unwrap();
    let pid = spawn_result["pid"].as_u64().unwrap();

    let read_result = handler.call(
        "kernel/fs/read",
        json!({"path": format!("/proc/{pid}/status.yaml")}),
        &admin_token(), Pid::new(57), "alice",
    ).await;
    assert!(read_result.is_ok(), "/proc/<pid>/status.yaml must be readable: {:?}", read_result);
}
```

---

## Step 2 — Implementation

### 2a. Add `is_agent_writable()` to `VfsPath`

In `src/memfs/path.rs`:

```rust
impl VfsPath {
    /// Returns `true` if an agent (non-kernel caller) may write to this path.
    /// The following trees are kernel-only and must never be written by agents:
    ///   /proc/        kernel-generated runtime state
    ///   /kernel/      compiled-in defaults and dynamic limits
    ///   /secrets/     kernel-managed encrypted store
    ///   /etc/avix/    system configuration (operator-only)
    ///   /bin/         system agents (operator-only)
    pub fn is_agent_writable(&self) -> bool {
        let s = self.as_str();
        !s.starts_with("/proc/")
            && !s.starts_with("/kernel/")
            && !s.starts_with("/secrets/")
            && !s.starts_with("/etc/avix/")
            && !s.starts_with("/bin/")
    }
}
```

### 2b. Enforce in the `kernel/fs/write` syscall handler

In `src/syscall/domain/fs_.rs`, at the start of the `write` handler:

```rust
"kernel/fs/write" => {
    let path_str = params["path"].as_str().ok_or(AvixError::ConfigParse("missing path".into()))?;
    let path = VfsPath::parse(path_str)?;

    if !path.is_agent_writable() {
        return Err(AvixError::CapabilityDenied(
            format!("EPERM: path {path_str} is kernel-owned and not writable by agents"),
        ));
    }
    // ... proceed with MemFs::write
}
```

**Important:** `MemFs::write()` itself is NOT modified. The raw `MemFs` remains
unrestricted for legitimate kernel code paths (bootstrap, agent spawn, pipe/open,
proc file updates). Enforcement lives exclusively in the syscall handler.

---

## Step 3 — Verify

```bash
cargo test --workspace
# All 8 VfsPath unit tests must pass
# All 4 syscall integration tests must pass
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

---

## Success Criteria

- [ ] `VfsPath::is_agent_writable()` returns `false` for `/proc/`, `/kernel/`, `/secrets/`, `/etc/avix/`, `/bin/`
- [ ] `VfsPath::is_agent_writable()` returns `true` for `/users/`, `/services/`, `/crews/`
- [ ] `kernel/fs/write` to `/proc/` returns EPERM (even with admin token)
- [ ] `kernel/fs/write` to `/kernel/` returns EPERM (even with admin token)
- [ ] `kernel/fs/write` to `/users/<u>/workspace/` succeeds with appropriate token
- [ ] `kernel/fs/read` of `/proc/<pid>/status.yaml` succeeds (reads are not blocked)
- [ ] `MemFs::write()` itself is unchanged — still accepts any path (kernel-internal)
- [ ] 12 new tests pass, 0 clippy warnings
