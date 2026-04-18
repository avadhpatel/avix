# Dev Plan: Session Management — Client Commands & Ownership Enforcement

## Task Summary

Wire up the missing session management operations so that:
1. All session commands live under `avix client session` (already the case — preserved).
2. Every session op enforces **ownership**: users can only list/get/delete/pause/resume
   sessions they own. Non-owning access returns `EPERM`. Operators and admins bypass the
   check via an injected `is_privileged` flag (derived from `caller_role`).
3. `session-delete` is fully plumbed end-to-end (IPC → gateway → client-core → CLI).
4. The three existing ops (`session-get`, `session-pause`, `session-resume`) are hardened
   with the same ownership check — currently they have none.

---

## Architecture References

- `docs/architecture/04-atp.md` — ATP command domains, ACL pipeline, role model
- `docs/architecture/06-agents.md` — session lifecycle, `SessionRecord` schema
- `docs/architecture/12-avix-clients.md` — CLI subcommand listing, client-core helpers

---

## Current Gaps

| Gap | Description |
|-----|-------------|
| `session-delete` missing everywhere | No IPC handler, no ATP gateway op, no CLI command, no client-core helper |
| `session-get` no ownership check | Any user who knows a UUID can read another user's session |
| `session-pause` no ownership check | Same — can pause another user's session |
| `session-resume` no ownership check | Same — can resume another user's session |
| `caller_identity` not injected for get/pause/resume | Gateway injects it only for `session-list`; all other ops omit it |

---

## Ownership Enforcement Design

The `SessionRecord` carries `username: String`. After fetching a session from the store,
the IPC server checks:

```
if !caller_identity.is_empty() && !is_privileged && session.username != caller_identity {
    return EPERM
}
```

- `caller_identity` — injected by the gateway from `ValidatedCmd.caller_identity`.
  An empty string means the kernel itself is calling (bypasses check).
- `is_privileged` — set to `true` when `caller_role >= operator` (injected by gateway
  as `"privileged": true`). Operators and admins may act on any session.
- Operators/admins naturally get `is_privileged: true` since their ATP token carries
  `Role::Operator` or `Role::Admin`, which the gateway reads from `ValidatedCmd.caller_role`.

For `session-list` this check is not needed — `list_for_user(username)` already filters by
username and the gateway already injects `caller_identity` as the username when omitted.

---

## Files to Change

| # | File | Change |
|---|------|--------|
| 1 | `crates/avix-core/src/kernel/proc/mod.rs` | Add `delete_session(&self, id: &Uuid)` |
| 2 | `crates/avix-core/src/kernel/ipc_server.rs` | Add `kernel/proc/session/delete`; add ownership check to `get`, `pause`, `resume` |
| 3 | `crates/avix-core/src/gateway/handlers/proc.rs` | Add `"session-delete"` to session ops; inject `caller_identity` + `is_privileged` for all session ops except list |
| 4 | `crates/avix-client-core/src/commands.rs` | Add `delete_session(dispatcher, session_id)` |
| 5 | `crates/avix-cli/src/commands/client/session.rs` | Add `Delete { id }` variant + arm |

---

## Implementation Order

### Step 1 — `proc/mod.rs`: Add `delete_session()`

In `ProcHandler`, add:

```rust
pub async fn delete_session(&self, id: &Uuid) -> Result<(), AvixError> {
    let store = self.session_store.as_ref()
        .ok_or_else(|| AvixError::NotFound("session store not configured".into()))?;
    store.delete(id).await
}
```

`PersistentSessionStore::delete(&uuid)` already exists at
`crates/avix-core/src/session/persistence.rs:111` — no new store logic needed.

Compile: `cargo check --package avix-core`
Test: `cargo test --package avix-core -- kernel::proc`

---

### Step 2 — `ipc_server.rs`: `session/delete` handler + ownership checks

**2a — Add ownership helper** (inline near the session block, not a shared fn):

```rust
fn session_ownership_ok(
    session_username: &str,
    caller_identity: &str,
    is_privileged: bool,
) -> bool {
    caller_identity.is_empty()       // kernel-internal call — bypass
    || is_privileged                 // operator / admin — bypass
    || session_username == caller_identity
}
```

**2b — Update `kernel/proc/session/get`**:

```rust
"kernel/proc/session/get" => {
    let session_id = params["id"].as_str().unwrap_or("");
    let caller_identity = params["caller_identity"].as_str().unwrap_or("");
    let is_privileged = params["is_privileged"].as_bool().unwrap_or(false);
    let uuid = match uuid::Uuid::parse_str(session_id) { ... };
    match proc_handler.get_session(&uuid).await {
        Ok(Some(session)) => {
            if !session_ownership_ok(&session.username, caller_identity, is_privileged) {
                return JsonRpcResponse::err(id, -32001, "EPERM: session belongs to another user", None);
            }
            JsonRpcResponse::ok(id, json!(session))
        }
        Ok(None) => JsonRpcResponse::err(id, -32003, &format!("session {session_id} not found"), None),
        Err(e) => JsonRpcResponse::err(id, -32000, &e.to_string(), None),
    }
}
```

**2c — Same ownership check for `session/pause` and `session/resume`**: extract
`caller_identity` + `is_privileged` from params; check before dispatching to
`proc_handler.pause_agent` / `resume_session`.

**2d — Add `kernel/proc/session/delete`**:

```rust
"kernel/proc/session/delete" => {
    let session_id = params["session_id"].as_str().unwrap_or("");
    let caller_identity = params["caller_identity"].as_str().unwrap_or("");
    let is_privileged = params["is_privileged"].as_bool().unwrap_or(false);
    let uuid = match uuid::Uuid::parse_str(session_id) {
        Ok(u) => u,
        Err(_) => return JsonRpcResponse::err(id, -32002, "invalid session ID", None),
    };
    // Ownership check — fetch record first
    match proc_handler.get_session(&uuid).await {
        Ok(Some(session)) => {
            if !session_ownership_ok(&session.username, caller_identity, is_privileged) {
                return JsonRpcResponse::err(id, -32001, "EPERM: session belongs to another user", None);
            }
        }
        Ok(None) => {
            // Idempotent — deleting a non-existent session is a no-op
            return JsonRpcResponse::ok(id, json!({ "deleted": session_id }));
        }
        Err(e) => return JsonRpcResponse::err(id, -32000, &e.to_string(), None),
    }
    match proc_handler.delete_session(&uuid).await {
        Ok(()) => {
            tracing::info!(session_id, "deleted session");
            JsonRpcResponse::ok(id, json!({ "deleted": session_id }))
        }
        Err(e) => {
            tracing::warn!(error = %e, "kernel/proc/session/delete failed");
            JsonRpcResponse::err(id, -32000, &e.to_string(), None)
        }
    }
}
```

Compile: `cargo check --package avix-core`
Test: `cargo test --package avix-core -- kernel::ipc_server`

---

### Step 3 — `gateway/handlers/proc.rs`: inject caller context + add `session-delete`

Update the `"session-list" | "session-get" | "session-resume"` match arm. Instead of one
arm, split into two:

```rust
// session-list: inject username when not provided
"session-list" => {
    let ipc_method = "kernel/proc/session/list";
    tracing::info!(op, "forwarding session-list to kernel IPC");
    let mut body = cmd.cmd.body;
    if body["username"].as_str().unwrap_or("").is_empty() {
        body["username"] = serde_json::json!(cmd.caller_identity);
    }
    ipc_forward(&id, ipc_method, body, ctx.ipc.as_ref()).await
}
// session-get / session-pause / session-resume / session-delete:
// inject caller_identity + is_privileged for ownership check
"session-get" | "session-pause" | "session-resume" | "session-delete" => {
    let ipc_method = format!("kernel/proc/{}", op.replace('-', "/"));
    tracing::info!(op, ipc_method = %ipc_method, "forwarding session op to kernel IPC");
    let is_privileged = matches!(cmd.caller_role, crate::types::Role::Operator | crate::types::Role::Admin);
    let mut body = cmd.cmd.body;
    body["caller_identity"] = serde_json::json!(cmd.caller_identity);
    body["is_privileged"] = serde_json::json!(is_privileged);
    ipc_forward(&id, &ipc_method, body, ctx.ipc.as_ref()).await
}
```

Compile: `cargo check --package avix-core`
Test: `cargo test --package avix-core -- gateway::handlers::proc`

---

### Step 4 — `avix-client-core/src/commands.rs`: `delete_session()`

Add after `resume_session`:

```rust
/// Delete a session by ID. Idempotent — no error if already deleted.
pub async fn delete_session(
    dispatcher: &Dispatcher,
    session_id: &str,
) -> Result<(), ClientError> {
    dispatch(
        dispatcher,
        "proc",
        "session-delete",
        serde_json::json!({ "session_id": session_id }),
    )
    .await?;
    Ok(())
}
```

Also update `docs/architecture/12-avix-clients.md` `commands.rs` listing to add
`delete_session`.

Compile: `cargo check --package avix-client-core`
Test: `cargo test --package avix-client-core`

---

### Step 5 — `avix-cli/src/commands/client/session.rs`: `Delete` subcommand

Add variant to `SessionCmd`:

```rust
/// Delete a session record
Delete {
    /// Session ID
    session_id: String,
    /// Skip confirmation prompt
    #[arg(long)]
    force: bool,
},
```

Add arm in `run()`:

```rust
SessionCmd::Delete { session_id, force } => {
    if !force {
        // Prompt: "Delete session <id>? [y/N]"
        print!("Delete session {}? [y/N] ", session_id);
        std::io::stdout().flush().ok();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        if !input.trim().eq_ignore_ascii_case("y") {
            emit(json, |_: &()| "Aborted".to_string(), ());
            return Ok(());
        }
    }
    let dispatcher = connect_config(None, None).await?;
    let reply = dispatcher
        .call(&AtpCmd_::new(
            "proc",
            "session-delete",
            "",
            serde_json::json!({ "session_id": session_id }),
        ))
        .await?;
    if !reply.ok {
        anyhow::bail!(reply.message.unwrap_or_else(|| "delete session failed".into()));
    }
    emit(json, |_: &()| format!("Deleted session {}", session_id), ());
}
```

Compile: `cargo check --package avix-cli`
Test: `cargo test --package avix-cli`

---

## Testing Strategy

### Step 2 IPC tests (add to `ipc_server.rs` `#[cfg(test)]` block)

```rust
// T-SM-01: session/get with wrong caller returns EPERM
// T-SM-02: session/get with correct caller succeeds
// T-SM-03: session/get with is_privileged=true bypasses ownership check
// T-SM-04: session/delete removes session when caller owns it
// T-SM-05: session/delete returns EPERM when caller doesn't own session
// T-SM-06: session/delete is idempotent (non-existent session returns ok)
// T-SM-07: session/pause with wrong caller returns EPERM
// T-SM-08: session/resume with wrong caller returns EPERM
```

### Step 3 gateway tests (add to `proc.rs` `#[cfg(test)]`)

```rust
// T-SM-09: session-delete injects caller_identity and is_privileged=false for user role
// T-SM-10: session-get injects is_privileged=true for operator role
```

---

## Success Criteria

- [ ] `avix client session delete <id>` deletes the session and prints confirmation
- [ ] `avix client session delete <id>` fails with EPERM if another user owns the session
- [ ] `avix client session show <id>` fails with EPERM if another user owns the session
- [ ] `avix client session resume <id>` fails with EPERM if another user owns the session
- [ ] Operator/admin role bypasses ownership check (`is_privileged: true`)
- [ ] Deleting a non-existent session is a no-op (idempotent)
- [ ] `--force` flag on `delete` skips the confirmation prompt
- [ ] `cargo check --package avix-core avix-client-core avix-cli` clean
- [ ] All new tests pass

---

## What This Does NOT Change

- Session creation — sessions are created only by the kernel at agent spawn time (no ATP endpoint to create directly)
- `session-list` ownership — already enforced by `list_for_user(username)` + gateway username injection
- `SessionRecord` schema — no new fields needed
- ATP domain table (proc domain already covers session ops)
