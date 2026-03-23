# ATP Gap E — Full Command Domain Handlers

> **Spec reference:** §6 Command Domains (all 11 domains)
> **Priority:** High
> **Depends on:** ATP Gap A, Gap B, Gap C, Gap D (dispatcher stub)

---

## Problem

The current `ATPTranslator` handles 9 commands across only 3 partial domains. The spec
defines 11 domains with ~60 operations total. This gap implements all missing handlers by
translating each ATP command to the correct IPC call and returning a spec-compliant reply.

### Coverage matrix

| Domain  | Spec ops | Currently implemented | Missing |
|---------|----------|----------------------|---------|
| `auth`  | 5        | 0                    | All 5   |
| `proc`  | 8        | 4 (spawn/kill/list/stat) | pause/resume/wait/setcap |
| `signal`| 4        | 0                    | All 4   |
| `fs`    | 6        | 3 (read/write/list)  | watch/unwatch/stat |
| `snap`  | 4        | 0                    | All 4   |
| `cron`  | 5        | 0                    | All 5   |
| `users` | 6        | 0                    | All 6   |
| `crews` | 7        | 0                    | All 7   |
| `cap`   | 4        | 0 (inspect/grant/revoke/policy) | All 4 |
| `sys`   | 7        | 2 (info/reboot)      | status/reload/shutdown/restart/logs/install/uninstall/update |
| `pipe`  | 3        | 0                    | All 3   |

---

## Architecture

Each domain gets its own handler module under `crates/avix-core/src/gateway/handlers/`.

```
crates/avix-core/src/gateway/handlers/
├── mod.rs
├── auth.rs
├── proc.rs
├── signal.rs
├── fs.rs
├── snap.rs
├── cron.rs
├── users.rs
├── crews.rs
├── cap.rs
├── sys.rs
└── pipe.rs
```

Each handler has the same signature:

```rust
pub async fn handle(cmd: ValidatedCmd, ipc: &IpcRouter) -> AtpReply
```

It pattern-matches `cmd.cmd.op`, calls the appropriate IPC method, and returns an
`AtpReply`. Unknown ops return `AtpReply::err(id, AtpError::new(Eparse, "unknown op"))`.

---

## What to Build — Domain by Domain

### `auth` domain

| Op         | IPC method         | Notes |
|------------|--------------------|-------|
| `refresh`  | (no IPC)           | Call `AuthService::refresh_token` directly |
| `whoami`   | (no IPC)           | Return claims from `ValidatedCmd::caller_*` |
| `logout`   | (no IPC)           | `ATPTokenStore::revoke(session_id)` + `AuthService::revoke_session` |
| `sessions` | `kernel/auth/sessions` | Admin only — already enforced by ACL |
| `kick`     | `kernel/auth/kick` | Admin only |

### `proc` domain — missing ops

| Op       | IPC method            | Body fields |
|----------|-----------------------|-------------|
| `pause`  | `kernel/proc/pause`   | `{ pid }` |
| `resume` | `kernel/proc/resume`  | `{ pid }` |
| `wait`   | `kernel/proc/wait`    | `{ pid, timeout_ms? }` |
| `setcap` | `kernel/proc/setcap`  | `{ pid, tools: { additional, denied } }` |

`spawn` body from spec: `{ "agent", "task", "crew", "tools": { "additional", "denied" } }` —
current impl uses `name`/`goal`, update to match spec.

### `signal` domain

| Op            | IPC method              | Body fields |
|---------------|-------------------------|-------------|
| `send`        | `kernel/signal/send`    | `{ signal, target, payload? }` |
| `subscribe`   | `kernel/signal/subscribe` | `{ pid }` |
| `unsubscribe` | `kernel/signal/unsubscribe` | `{ pid }` |
| `list`        | `kernel/signal/list`    | `{}` |

Valid signal names (validate before forwarding): `SIGSTART`, `SIGPAUSE`, `SIGRESUME`,
`SIGKILL`, `SIGSTOP`, `SIGSAVE`, `SIGPIPE`, `SIGESCALATE`.

Return `EPARSE` for unknown signal names.

#### `SIGPIPE` payload — `SigPipePayload`

`SIGPIPE` is the mechanism for sending a message (and optional attachments) into a
running agent's LLM context. Its `payload` field uses a typed struct so the wire format
is stable now and attachment processing can be added later without breaking changes.

File: `crates/avix-core/src/signal/pipe_payload.rs`

```rust
use serde::{Deserialize, Serialize};

/// The typed payload for a SIGPIPE signal (§6.3).
/// `text` is injected into the agent's LLM context immediately.
/// `attachments` is optional and currently parsed + validated but not yet
/// injected — RuntimeExecutor ignores it until multimodal support lands.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SigPipePayload {
    /// Plain-text instruction or message injected into the agent's context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// Optional file/data attachments. Ignored by RuntimeExecutor until
    /// multimodal injection is implemented (see future atp-gap-H).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<PipeAttachment>>,
}

/// A single attachment carried inside a SIGPIPE payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PipeAttachment {
    /// Binary or text data embedded directly in the message, base64-encoded.
    Inline {
        /// MIME type, e.g. "image/png", "text/plain", "application/pdf".
        content_type: String,
        /// Must be "base64". Reserved for future encodings.
        encoding: InlineEncoding,
        /// The encoded data string.
        data: String,
        /// Optional human-readable label shown in UIs.
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// A reference to a file already present in the VFS.
    /// RuntimeExecutor reads the file at injection time.
    VfsRef {
        /// Absolute VFS path, e.g. "/users/alice/report.pdf".
        path: String,
        /// MIME type hint for the LLM multimodal adapter.
        content_type: String,
        /// Optional human-readable label.
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InlineEncoding {
    Base64,
}

impl SigPipePayload {
    /// Validate the payload before forwarding to the kernel.
    /// Returns Err with a human-readable message if invalid.
    pub fn validate(&self) -> Result<(), String> {
        // At least one of text or attachments must be present
        if self.text.is_none() && self.attachments.as_ref().map_or(true, Vec::is_empty) {
            return Err("SIGPIPE payload must have 'text' or at least one attachment".into());
        }
        if let Some(attachments) = &self.attachments {
            for (i, att) in attachments.iter().enumerate() {
                match att {
                    PipeAttachment::Inline { data, encoding, .. } => {
                        if *encoding == InlineEncoding::Base64 {
                            use base64::Engine;
                            base64::engine::general_purpose::STANDARD
                                .decode(data)
                                .map_err(|e| format!("attachment[{i}] invalid base64: {e}"))?;
                        }
                    }
                    PipeAttachment::VfsRef { path, .. } => {
                        if !path.starts_with('/') {
                            return Err(format!(
                                "attachment[{i}] vfs_ref path must be absolute, got '{path}'"
                            ));
                        }
                        if path.starts_with("/secrets/") {
                            return Err(format!(
                                "attachment[{i}] vfs_ref path '/secrets/' is forbidden"
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
```

**Handler integration** — in `handlers/signal.rs`, when `signal == "SIGPIPE"`:

```rust
"SIGPIPE" => {
    // Parse and validate the typed payload
    let pipe_payload: SigPipePayload = serde_json::from_value(
        body.get("payload").cloned().unwrap_or(serde_json::json!({}))
    ).map_err(|e| AtpError::new(AtpErrorCode::Eparse, format!("invalid SIGPIPE payload: {e}")))?;

    pipe_payload.validate()
        .map_err(|e| AtpError::new(AtpErrorCode::Eparse, e))?;

    // Forward to kernel — attachments are carried through but RuntimeExecutor
    // currently only acts on payload.text; attachments are preserved for future use
    ipc.call("kernel/signal/send", json!({
        "signal": "SIGPIPE",
        "target": target_pid,
        "payload": serde_json::to_value(&pipe_payload)?,
    })).await
}
```

### `fs` domain — missing ops

| Op       | IPC method          | Body fields |
|----------|---------------------|-------------|
| `watch`  | `kernel/fs/watch`   | `{ path }` |
| `unwatch`| `kernel/fs/unwatch` | `{ path }` |
| `stat`   | `kernel/fs/stat`    | `{ path }` |

`watch` registers a VFS watcher — events flow via the ATP event bus as `fs.changed`.
The handler must also validate no `/secrets/` read (already guarded by VFS; but double-
check that `read` of `/secrets/` is rejected with `EPERM` at this layer too).

### `snap` domain

| Op        | IPC method           | Body fields |
|-----------|----------------------|-------------|
| `create`  | `kernel/snap/create` | `{ pid, label? }` |
| `list`    | `kernel/snap/list`   | `{ pid? }` |
| `restore` | `kernel/snap/restore`| `{ snapshot_id }` |
| `delete`  | `kernel/snap/delete` | `{ snapshot_id }` |

### `cron` domain

| Op       | IPC method            | Body fields |
|----------|-----------------------|-------------|
| `list`   | `kernel/cron/list`    | `{}` |
| `add`    | `kernel/cron/add`     | `{ schedule, agent, task, ... }` |
| `remove` | `kernel/cron/remove`  | `{ job_id }` |
| `pause`  | `kernel/cron/pause`   | `{ job_id }` |
| `resume` | `kernel/cron/resume`  | `{ job_id }` |

### `users` domain

| Op       | IPC method             | Body fields |
|----------|------------------------|-------------|
| `list`   | `kernel/users/list`    | `{}` |
| `get`    | `kernel/users/get`     | `{ username? }` — default to caller if user role |
| `create` | `kernel/users/create`  | `{ username, role, credential, ... }` |
| `update` | `kernel/users/update`  | `{ username, ... }` |
| `delete` | `kernel/users/delete`  | `{ username }` |
| `passwd` | `kernel/users/passwd`  | `{ current_password, new_password }` |

`get` self-scoping: if caller role is `user` and `body.username` is absent or matches
`caller_identity`, allow. If `body.username` is a different user → `EPERM`. Admin
bypasses this.

### `crews` domain

| Op       | IPC method              | Body fields |
|----------|-------------------------|-------------|
| `list`   | `kernel/crews/list`     | `{}` |
| `get`    | `kernel/crews/get`      | `{ crew_name }` |
| `create` | `kernel/crews/create`   | `{ name, description, ... }` |
| `update` | `kernel/crews/update`   | `{ crew_name, ... }` |
| `delete` | `kernel/crews/delete`   | `{ crew_name }` |
| `join`   | `kernel/crews/join`     | `{ crew_name, username }` |
| `leave`  | `kernel/crews/leave`    | `{ crew_name, username }` |

### `cap` domain

| Op           | IPC method              | Body fields |
|--------------|-------------------------|-------------|
| `inspect`    | `kernel/cap/inspect`    | `{ subject }` |
| `grant`      | `kernel/cap/grant`      | `{ subject, tools }` |
| `revoke`     | `kernel/cap/revoke`     | `{ subject, tools }` |
| `policy/get` | `kernel/cap/policy_get` | `{}` |
| `policy/set` | `kernel/cap/policy_set` | `{ policy }` |

### `sys` domain — missing ops

| Op          | IPC method              | Notes |
|-------------|-------------------------|-------|
| `status`    | `kernel/sys/status`     | operator+ |
| `reload`    | `kernel/sys/reload`     | admin, admin port |
| `shutdown`  | `kernel/sys/shutdown`   | admin, admin port |
| `restart`   | `kernel/sys/restart`    | admin, admin port |
| `logs`      | `kernel/sys/logs`       | operator+, streaming → job_id |
| `install`   | `kernel/sys/install`    | operator+, async → job_id |
| `uninstall` | `kernel/sys/uninstall`  | operator+ |
| `update`    | `kernel/sys/update`     | operator+ |

Streaming / async ops (`logs`, `install`) return a `job_id`; progress is pushed
via the `jobs.svc` and surfaced as ATP events (Gap F).

### `pipe` domain

| Op      | IPC method           | Body fields |
|---------|----------------------|-------------|
| `open`  | `kernel/pipe/open`   | `{ from_pid, to_pid, name? }` |
| `close` | `kernel/pipe/close`  | `{ pipe_id }` |
| `list`  | `kernel/pipe/list`   | `{}` |

---

## IPC Error → AtpError Mapping

All handlers wrap IPC errors. Standard mapping:

| IPC error string | ATP code |
|-----------------|----------|
| `not found` / `ENOENT` | `ENOTFOUND` |
| `permission` / `EPERM` | `EPERM` |
| `conflict` / `ECONFLICT` | `ECONFLICT` |
| `unavailable` / `EUNAVAIL` | `EUNAVAIL` |
| anything else | `EINTERNAL` |

Helper:

```rust
fn ipc_err_to_atp(ipc_err: AvixError) -> AtpError {
    let msg = ipc_err.to_string();
    let code = if msg.contains("not found") || msg.contains("ENOENT") {
        AtpErrorCode::Enotfound
    } else if msg.contains("permission") || msg.contains("EPERM") {
        AtpErrorCode::Eperm
    } else if msg.contains("conflict") || msg.contains("ECONFLICT") {
        AtpErrorCode::Econflict
    } else if msg.contains("unavailable") || msg.contains("EUNAVAIL") {
        AtpErrorCode::Eunavail
    } else {
        AtpErrorCode::Einternal
    };
    AtpError::new(code, msg)
}
```

---

## Tests to Write

Each handler module gets unit tests using a mock `IpcRouter`. Key tests per domain:

### `auth` handlers

```rust
#[tokio::test]
async fn whoami_returns_identity_and_role();

#[tokio::test]
async fn logout_revokes_session();

#[tokio::test]
async fn refresh_returns_new_token();
```

### `proc` handlers

```rust
#[tokio::test]
async fn pause_translates_to_kernel_proc_pause();

#[tokio::test]
async fn resume_translates_to_kernel_proc_resume();

#[tokio::test]
async fn setcap_blocked_for_user_role();

#[tokio::test]
async fn setcap_allowed_for_operator_role();
```

### `signal` handlers

```rust
#[tokio::test]
async fn send_valid_signal_translates_correctly();

#[tokio::test]
async fn send_unknown_signal_returns_eparse();
```

### `SigPipePayload` unit tests

File: `crates/avix-core/src/signal/pipe_payload.rs` (under `#[cfg(test)]`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_only_payload_is_valid() {
        let p = SigPipePayload { text: Some("hello".into()), attachments: None };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn empty_payload_is_invalid() {
        let p = SigPipePayload::default();
        assert!(p.validate().is_err());
    }

    #[test]
    fn valid_base64_inline_attachment_passes() {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"hello world");
        let p = SigPipePayload {
            text: None,
            attachments: Some(vec![PipeAttachment::Inline {
                content_type: "text/plain".into(),
                encoding: InlineEncoding::Base64,
                data,
                label: None,
            }]),
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn invalid_base64_inline_attachment_fails() {
        let p = SigPipePayload {
            text: None,
            attachments: Some(vec![PipeAttachment::Inline {
                content_type: "image/png".into(),
                encoding: InlineEncoding::Base64,
                data: "not!!valid%%base64".into(),
                label: None,
            }]),
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn vfs_ref_absolute_path_passes() {
        let p = SigPipePayload {
            text: Some("see attached".into()),
            attachments: Some(vec![PipeAttachment::VfsRef {
                path: "/users/alice/report.pdf".into(),
                content_type: "application/pdf".into(),
                label: None,
            }]),
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn vfs_ref_relative_path_fails() {
        let p = SigPipePayload {
            text: None,
            attachments: Some(vec![PipeAttachment::VfsRef {
                path: "users/alice/report.pdf".into(),  // missing leading /
                content_type: "application/pdf".into(),
                label: None,
            }]),
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn vfs_ref_secrets_path_is_forbidden() {
        let p = SigPipePayload {
            text: None,
            attachments: Some(vec![PipeAttachment::VfsRef {
                path: "/secrets/api_key".into(),
                content_type: "text/plain".into(),
                label: None,
            }]),
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn payload_round_trips_through_json() {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"img");
        let p = SigPipePayload {
            text: Some("look at this".into()),
            attachments: Some(vec![
                PipeAttachment::Inline {
                    content_type: "image/png".into(),
                    encoding: InlineEncoding::Base64,
                    data: data.clone(),
                    label: Some("screenshot".into()),
                },
                PipeAttachment::VfsRef {
                    path: "/users/alice/doc.pdf".into(),
                    content_type: "application/pdf".into(),
                    label: None,
                },
            ]),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: SigPipePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(back.text.as_deref(), Some("look at this"));
        assert_eq!(back.attachments.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn inline_attachment_type_tag_serializes_correctly() {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(b"x");
        let att = PipeAttachment::Inline {
            content_type: "image/png".into(),
            encoding: InlineEncoding::Base64,
            data,
            label: None,
        };
        let json = serde_json::to_string(&att).unwrap();
        assert!(json.contains("\"type\":\"inline\""));
    }

    #[test]
    fn vfs_ref_attachment_type_tag_serializes_correctly() {
        let att = PipeAttachment::VfsRef {
            path: "/users/alice/f.txt".into(),
            content_type: "text/plain".into(),
            label: None,
        };
        let json = serde_json::to_string(&att).unwrap();
        assert!(json.contains("\"type\":\"vfs_ref\""));
    }
}
```

### `fs` handlers

```rust
#[tokio::test]
async fn read_secrets_returns_eperm();  // double-check at handler level

#[tokio::test]
async fn stat_translates_to_kernel_fs_stat();

#[tokio::test]
async fn watch_translates_to_kernel_fs_watch();
```

### `snap` handlers

```rust
#[tokio::test]
async fn snap_create_translates_to_kernel_snap_create();

#[tokio::test]
async fn snap_delete_blocked_for_user_role();
```

### `users` handlers

```rust
#[tokio::test]
async fn get_own_user_allowed_for_user_role();

#[tokio::test]
async fn get_other_user_blocked_for_user_role();

#[tokio::test]
async fn create_user_blocked_for_operator_role();

#[tokio::test]
async fn create_user_allowed_for_admin_role();
```

### `cap` / `sys` / `pipe` handlers

```rust
#[tokio::test]
async fn cap_inspect_allowed_for_operator();

#[tokio::test]
async fn sys_status_returns_service_health();

#[tokio::test]
async fn pipe_open_translates_to_kernel_pipe_open();
```

---

## Success Criteria

- [ ] All 11 domain handler modules exist and compile
- [ ] Every spec op listed in §6 has a handler (even if stubbed with `EUNAVAIL` where kernel IPC not yet ready)
- [ ] `proc.spawn` body uses `agent`/`task`/`crew`/`tools` from spec (not `name`/`goal`)
- [ ] `signal.send` validates signal name; unknown signal → `EPARSE`
- [ ] `fs.read` / `fs.write` of `/secrets/` → `EPERM` at handler level
- [ ] `users.get` self-scoping enforced for `user` role
- [ ] IPC error → ATP error code mapping covers all 5 cases
- [ ] `SigPipePayload` struct exists in `signal/pipe_payload.rs` with `text` + `attachments`
- [ ] `PipeAttachment` enum has `Inline` (base64) and `VfsRef` variants with serde tag `type`
- [ ] `SigPipePayload::validate` rejects: empty payload, invalid base64, relative VFS paths, `/secrets/` VFS refs
- [ ] SIGPIPE handler parses `SigPipePayload`, calls `validate`, forwards full payload to kernel
- [ ] Payload round-trips through JSON without data loss
- [ ] RuntimeExecutor ignoring unknown `attachments` field does not break existing text-only SIGPIPE flows
- [ ] All above tests pass; `cargo clippy` zero warnings
