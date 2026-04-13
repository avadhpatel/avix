# Session Gap B — Session Delete

**Status**: Pending  
**Created**: 2026-04-12  
**Workaround**: Stop kernel, delete `<root>/data/sessions.redb`

---

## Summary

`SessionStore::delete()` exists at the store layer (`crates/avix-core/src/session/store.rs:84`)
but is not wired up to `PersistentSessionStore`, the IPC server, the ATP gateway, or the CLI.
This means there is no supported way to delete a bad/stale session record short of nuking the
entire `sessions.redb` file.

---

## Files to Change

| # | File | Change |
|---|------|--------|
| 1 | `crates/avix-core/src/kernel/proc/session.rs` | Add `delete(&self, session_id: &str)` to `PersistentSessionStore` |
| 2 | `crates/avix-core/src/kernel/ipc_server.rs` | Register + handle `kernel/proc/session/delete` |
| 3 | `crates/avix-core/src/gateway/handlers/session.rs` | Forward `proc/session/delete` ATP command via `ipc_forward` |
| 4 | `crates/avix-cli/src/commands/session.rs` | Add `avix session delete <id>` subcommand |
| 5 | `crates/avix-core/tests/session_delete.rs` (NEW) | Integration tests |

---

## Step-by-Step Implementation

### Step 1 — `PersistentSessionStore::delete()`

In `crates/avix-core/src/kernel/proc/session.rs`, add a method that delegates to
`SessionStore::delete()`:

```rust
pub async fn delete(&self, session_id: &str) -> Result<(), AvixError> {
    self.store.delete(session_id).await
}
```

`SessionStore::delete()` already exists and removes the redb entry. No new logic needed.

**Compile check**: `cargo check --package avix-core`

---

### Step 2 — IPC handler `kernel/proc/session/delete`

In `crates/avix-core/src/kernel/ipc_server.rs`, add a handler alongside the existing
`kernel/proc/session/*` handlers (around line 390–430):

```rust
"kernel/proc/session/delete" => {
    let session_id = params["session_id"]
        .as_str()
        .ok_or_else(|| AvixError::InvalidParams("missing session_id".into()))?;
    self.sessions.delete(session_id).await?;
    Ok(json!({"deleted": session_id}))
}
```

**Compile check**: `cargo check --package avix-core`

---

### Step 3 — ATP gateway forwarding

In `crates/avix-core/src/gateway/handlers/session.rs` (or wherever `proc/session/*` ATP
commands are forwarded), add:

```rust
"proc/session/delete" => {
    ipc_forward("kernel/proc/session/delete", params, &self.kernel_sock).await
}
```

Follow the exact same pattern as `proc/session/pause` and `proc/session/resume`.

**Compile check**: `cargo check --package avix-core`

---

### Step 4 — CLI `avix session delete <id>`

In `crates/avix-cli/src/commands/session.rs`, add a `delete` subcommand:

```rust
SessionCommand::Delete { id } => {
    let result = client.send("proc/session/delete", json!({"session_id": id})).await?;
    println!("Deleted session {}", result["deleted"].as_str().unwrap_or(&id));
    Ok(())
}
```

Register it in the `SessionCommand` enum and match arm alongside `list`, `get`, `pause`,
`resume`.

**Compile check**: `cargo check --package avix-cli`

---

### Step 5 — Integration tests

New file `crates/avix-core/tests/session_delete.rs`:

```rust
// T-SB-01: delete existing session removes it from list
// T-SB-02: delete non-existent session returns error (or is a no-op — decide at impl time)
// T-SB-03: delete then get returns not_found
```

**Test run**: `cargo test --test session_delete`

---

## Architecture Spec Update

After all steps pass, update `docs/architecture/06-agents.md` (session management section)
to document `kernel/proc/session/delete` and `avix session delete <id>`.

---

## Notes

- `SessionStore::delete()` at `crates/avix-core/src/session/store.rs:84` already removes the
  redb entry atomically — no new store-level logic needed.
- Decide at Step 2 whether deleting a non-existent session is an error or a no-op (recommend
  no-op for idempotency — return `{"deleted": session_id}` regardless).
- The `PersistentSessionStore` wraps `SessionStore` — only a one-liner delegation is needed.
