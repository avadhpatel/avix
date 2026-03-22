use avix_core::syscall::{SyscallContext, SyscallError, SyscallHandler};
use avix_core::types::token::CapabilityToken;
use serde_json::json;

fn admin_ctx() -> SyscallContext {
    SyscallContext {
        caller_pid: 57,
        token: CapabilityToken::test_token(&[
            "kernel/fs/read",
            "kernel/fs/write",
            "kernel/proc/spawn",
        ]),
    }
}

// ── Finding D: fs/write enforcement ──────────────────────────────────────────

#[test]
fn fs_write_to_proc_returns_eperm() {
    let handler = SyscallHandler;
    let result = handler.dispatch(
        &admin_ctx(),
        "kernel/fs/write",
        json!({"path": "/proc/57/status.yaml", "content": "tamper"}),
    );
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string().to_lowercase();
    assert!(
        msg.contains("eperm") || msg.contains("permission") || msg.contains("forbidden"),
        "write to /proc/ must return EPERM, got: {msg}"
    );
}

#[test]
fn fs_write_to_kernel_returns_eperm() {
    let handler = SyscallHandler;
    let result = handler.dispatch(
        &admin_ctx(),
        "kernel/fs/write",
        json!({"path": "/kernel/defaults/agent.yaml", "content": "tamper"}),
    );
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string().to_lowercase();
    assert!(
        msg.contains("eperm") || msg.contains("permission") || msg.contains("forbidden"),
        "write to /kernel/ must return EPERM, got: {msg}"
    );
}

#[test]
fn fs_write_to_user_workspace_succeeds() {
    let handler = SyscallHandler;
    let result = handler.dispatch(
        &admin_ctx(),
        "kernel/fs/write",
        json!({"path": "/users/alice/workspace/notes.txt", "content": "hello"}),
    );
    assert!(
        result.is_ok(),
        "write to /users/alice/workspace/ must succeed: {:?}",
        result
    );
}

#[test]
fn fs_read_of_proc_path_is_not_blocked() {
    // Reads of /proc/ are allowed — only writes are blocked
    let handler = SyscallHandler;
    let result = handler.dispatch(
        &admin_ctx(),
        "kernel/fs/read",
        json!({"path": "/proc/57/status.yaml"}),
    );
    assert!(
        result.is_ok(),
        "/proc/<pid>/status.yaml must be readable by agents: {:?}",
        result
    );
}
