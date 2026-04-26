# Dev Plan: HIL ATP Event Fix + JSONL Recording

## Task Summary

Fix 4 gaps in the HIL (Human-in-the-Loop) flow:

1. `hil.request` ATP event body shape wrong — client never receives usable data
2. SIGRESUME payload field names wrong — server never consumes approval token
3. `hil.resolved` ATP event body shape wrong — client never clears HIL modal
4. HIL request/response not recorded in invocation JSONL — state lost on reboot

`pid` is mandatory in all HIL APIs (request and resolved events, JSONL entries) because
multiple agent pids can share one ATP session.

## Architecture Refs

- `docs/architecture/` — ATP event kinds, signal protocol
- `crates/avix-core/src/kernel/hil.rs` — `HilRequest` struct
- `crates/avix-core/src/kernel/hil_manager.rs` — `open`, `resolve`, `push_resolved`, `timeout_hil`
- `crates/avix-core/src/gateway/handlers/signal.rs` — SIGRESUME gateway parser
- `crates/avix-client-core/src/atp/types.rs` — `HilRequestBody`, `HilResolvedBody`
- `crates/avix-client-core/src/commands.rs` — `resolve_hil`
- `crates/avix-core/src/executor/runtime_executor/dispatch_manager.rs` — HIL trigger site
- `crates/avix-core/src/invocation/conversation.rs` — `Role`, `ConversationEntry`

## Gap Analysis

### Gap 1 — `hil.request` event body shape

Server (`hil_manager::open`) serialises the full `HilRequest` struct. That struct has
`reason: Option<String>`, `expires_at: DateTime<Utc>`, `agent_name: String` — but **no**
`session_id`, `prompt`, or `timeout_secs`. Client `HilRequestBody` expects exactly those
three missing fields → event body never deserialises → TUI HIL modal never fires.

`HilRequest` also has no `atp_session_id` field at all; the server cannot include it.

### Gap 2 — SIGRESUME payload field names (client → server)

`resolve_hil` in `commands.rs` sends:
```json
{ "hil_id": "...", "approval_token": "...", "approved": true, "note": null }
```
Gateway `signal.rs` reads `payload["hilId"]`, `payload["approvalToken"]`,
`payload["decision"]` ("approved"/"denied" string). Field names never match → token
never consumed → agent never unblocked.

### Gap 3 — `hil.resolved` event body shape

`push_resolved` emits camelCase `{ "hilId": ..., "outcome": ..., "resolvedBy": ...,
"resolvedAt": ..., "note": ... }`. Client `HilResolvedBody` has field `hil_id`
(no rename annotation) and `pid` (server never sends it). `hil_id` never deserialises
→ TUI cannot clear the HIL modal.

### Gap 4 — HIL not in invocation JSONL

Neither HIL request nor response is written to `conversation_history`. After reboot or
session resume the UI cannot re-render the approval/denial in the conversation view.

## Wire Contracts (after fix)

### `hil.request` event body
```json
{
  "hil_id": "uuid",
  "pid": 12345,
  "session_id": "atp-session-uuid",
  "approval_token": "uuid",
  "hil_type": "capability_upgrade",
  "tool": "fs/write",
  "reason": "Agent needs write access to complete task",
  "prompt": "Agent requests capability: fs/write. Reason: Agent needs write access to complete task",
  "timeout_secs": 600,
  "urgency": "normal"
}
```

### `hil.resolved` event body
```json
{
  "hil_id": "uuid",
  "pid": 12345,
  "session_id": "atp-session-uuid",
  "outcome": "approved",
  "resolved_by": "alice",
  "resolved_at": "2026-04-25T10:00:00Z"
}
```

### SIGRESUME payload (client → server, inside `signal.send` body)
```json
{
  "approvalToken": "uuid",
  "hilId": "uuid",
  "decision": "approved"
}
```
Note: `note` field stays optional alongside these.

### JSONL entries (Role variants)

```json
{ "role": "hil_request", "content": "{\"hilId\":\"...\",\"pid\":12345,\"sessionId\":\"...\",\"hilType\":\"capability_upgrade\",\"tool\":\"fs/write\",\"reason\":\"...\",\"approvalToken\":\"...\"}" }
{ "role": "hil_response", "content": "{\"hilId\":\"...\",\"pid\":12345,\"outcome\":\"approved\",\"resolvedBy\":\"alice\",\"resolvedAt\":\"...\"}" }
```

`content` is a JSON-serialised string so existing `ConversationEntry` parsers see a
valid entry without schema breakage. UI detects `role == "hil_request"` to render the
approval card.

## Files to Change (implementation order)

### Step 1 — `crates/avix-core/src/kernel/hil.rs`

Add `atp_session_id: String` field to `HilRequest` (no serde rename needed — snake_case
matches Rust field names). This is the ATP connection session ID, not the agent's
internal `session_id`.

```rust
pub struct HilRequest {
    // existing fields ...
    pub atp_session_id: String,   // ← new
}
```

All existing `HilRequest` construction sites (only `dispatch_manager.rs`) must be
updated in Step 3.

**Tests**: update sample_request() in hil.rs tests to include `atp_session_id`.

---

### Step 2 — `crates/avix-core/src/kernel/hil_manager.rs`

**`open()`** — replace `serde_json::to_value(&req)` with an explicit JSON body:

```rust
let now = Utc::now();
let timeout_secs = (req.expires_at - now).num_seconds().max(0) as u32;
let prompt = match (&req.tool, &req.reason) {
    (Some(t), Some(r)) => format!("Agent requests capability: {t}. Reason: {r}"),
    (Some(t), None)    => format!("Agent requests capability: {t}"),
    (None,    Some(r)) => r.clone(),
    (None,    None)    => "Agent requires human approval".to_string(),
};
let event_body = serde_json::json!({
    "hil_id":         req.hil_id,
    "pid":            req.pid.as_u64(),
    "session_id":     req.atp_session_id,
    "approval_token": req.approval_token,
    "hil_type":       req.hil_type,
    "tool":           req.tool,
    "reason":         req.reason,
    "prompt":         prompt,
    "timeout_secs":   timeout_secs,
    "urgency":        req.urgency,
});
```

**`resolve()`** — before `self.pending.write().await.remove(hil_id)`, extract `pid` and
`atp_session_id` from the pending entry:

```rust
let (session_owner, pid, atp_session_id) = {
    let guard = self.pending.read().await;
    if let Some(req) = guard.get(hil_id) {
        // update VFS state ...
        (req.agent_name.clone(), req.pid, req.atp_session_id.clone())
    } else {
        (String::new(), Pid::from_u64(0), String::new())
    }
};
self.pending.write().await.remove(hil_id);
self.push_resolved(hil_id, decision, resolved_by, &session_owner, pid, &atp_session_id, payload).await;
```

**`timeout_hil()`** — extract `atp_session_id` from the pending entry before removal:

```rust
let (session_owner, atp_session_id) = {
    let mut guard = self.pending.write().await;
    if let Some(req) = guard.remove(hil_id) {
        // update VFS state ...
        (req.agent_name.clone(), req.atp_session_id.clone())
    } else {
        return;
    }
};
// pid already a param
self.push_resolved(hil_id, "timeout", "kernel", &session_owner, pid, &atp_session_id, &json!({})).await;
```

**`push_resolved()`** — update signature to include `pid: Pid` and `session_id: &str`;
emit proper body:

```rust
async fn push_resolved(
    &self,
    hil_id: &str,
    outcome: &str,
    resolved_by: &str,
    session_owner: &str,
    pid: Pid,
    session_id: &str,
    payload: &serde_json::Value,
) {
    let event = AtpEvent::new(
        AtpEventKind::HilResolved,
        session_owner,
        serde_json::json!({
            "hil_id":      hil_id,
            "pid":         pid.as_u64(),
            "session_id":  session_id,
            "outcome":     outcome,
            "resolved_by": resolved_by,
            "resolved_at": Utc::now(),
        }),
    );
    self.event_bus.publish(event, Some(session_owner.to_string()), Role::User);
}
```

**Tests**: update existing HilManager tests in hil_manager.rs — add `atp_session_id`
to `HilRequest` construction and assert new event body fields.

---

### Step 3 — `crates/avix-core/src/executor/runtime_executor/dispatch_manager.rs`

**Set `atp_session_id`** in `hil_req` construction (the `cap/request-tool` handler):

```rust
let hil_req = crate::kernel::hil::HilRequest {
    // existing fields ...
    atp_session_id: self.atp_session_id.clone(),
};
```

**Record HIL request** in `conversation_history` immediately after `hil_mgr.open()` succeeds:

```rust
use crate::invocation::conversation::{ConversationEntry, Role};
let hil_entry = ConversationEntry::from_role_content(
    Role::HilRequest,
    serde_json::to_string(&serde_json::json!({
        "hilId":         hil_id,
        "pid":           self.pid.as_u64(),
        "sessionId":     self.atp_session_id,
        "hilType":       "capability_upgrade",
        "tool":          tool_name,
        "reason":        reason,
        "approvalToken": hil_req.approval_token,  // capture before move
    })).unwrap_or_default(),
);
self.memory.conversation_history.push(hil_entry);
```

Note: capture `hil_req.approval_token` before `hil_mgr.open(hil_req)` consumes it, or
clone it first.

**Record HIL response** after `upgrader.request_tool()` returns:

```rust
// Ok branch
let response_entry = ConversationEntry::from_role_content(
    Role::HilResponse,
    serde_json::to_string(&serde_json::json!({
        "hilId":      hil_id,
        "pid":        self.pid.as_u64(),
        "outcome":    "approved",
        "resolvedAt": chrono::Utc::now(),
    })).unwrap_or_default(),
);
self.memory.conversation_history.push(response_entry);

// Err branch
let response_entry = ConversationEntry::from_role_content(
    Role::HilResponse,
    serde_json::to_string(&serde_json::json!({
        "hilId":      hil_id,
        "pid":        self.pid.as_u64(),
        "outcome":    "denied",
        "resolvedAt": chrono::Utc::now(),
    })).unwrap_or_default(),
);
self.memory.conversation_history.push(response_entry);
```

**Tests**: add a test that triggers `cap/request-tool` with a wired HilManager and
asserts `conversation_history` contains both `hil_request` and `hil_response` entries.

---

### Step 4 — `crates/avix-core/src/invocation/conversation.rs`

Add two new variants to `Role`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    Tool,
    System,
    HilRequest,   // ← new: serialises as "hil_request"
    HilResponse,  // ← new: serialises as "hil_response"
}
```

No other changes needed — `ConversationEntry` is generic over `role` + `content`.

**Tests**: add round-trip tests for `Role::HilRequest` and `Role::HilResponse`.

---

### Step 5 — `crates/avix-client-core/src/atp/types.rs`

Replace `HilRequestBody`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HilRequestBody {
    pub hil_id: String,
    pub pid: u64,
    pub session_id: String,
    pub approval_token: String,
    pub hil_type: String,
    pub tool: Option<String>,
    pub reason: Option<String>,
    pub prompt: String,
    pub timeout_secs: u32,
    pub urgency: String,
}
```

Replace `HilResolvedBody`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HilResolvedBody {
    pub hil_id: String,
    pub pid: u64,
    pub session_id: String,
    pub outcome: HilOutcome,
    pub resolved_by: String,
    pub resolved_at: chrono::DateTime<chrono::Utc>,
}
```

Note: server now sends `hil_id` (snake_case) and `pid` (always present) — no serde
renames needed on client structs if server uses snake_case output (confirmed above in
Step 2).

Also add `chrono` to `HilResolvedBody` — check `avix-client-core/Cargo.toml` for
existing chrono dep; add if missing.

**Tests**: add serde round-trip tests for both structs.

---

### Step 6 — `crates/avix-client-core/src/commands.rs`

Fix `resolve_hil` to send camelCase fields and `decision` string:

```rust
pub async fn resolve_hil(
    dispatcher: &Dispatcher,
    pid: u64,
    hil_id: &str,
    approval_token: &str,
    approved: bool,
    note: Option<&str>,
) -> Result<(), ClientError> {
    let mut payload = serde_json::json!({
        "hilId":         hil_id,
        "approvalToken": approval_token,
        "decision":      if approved { "approved" } else { "denied" },
    });
    if let Some(n) = note {
        payload["note"] = serde_json::Value::String(n.to_string());
    }
    send_signal(dispatcher, pid, "SIGRESUME", Some(payload)).await
}
```

**Tests**: update `resolve_hil_sends_sigresume` test to assert camelCase fields and
`decision` key.

---

## Testing Strategy

Run only targeted tests after each file:

```bash
# Step 1
cargo test --package avix-core hil::tests

# Step 2
cargo test --package avix-core hil_manager::tests

# Step 3
cargo test --package avix-core dispatch_manager

# Step 4
cargo test --package avix-core conversation::tests

# Step 5
cargo test --package avix-client-core atp::types

# Step 6
cargo test --package avix-client-core commands::tests
```

After all steps: `cargo check --package avix-core --package avix-client-core`

## Success Criteria

- `hil.request` event: client parses body, TUI modal renders with pid/tool/reason/prompt
- `hil.resolved` event: client parses body including pid, TUI clears modal
- SIGRESUME: server consumes approval token, agent unblocks
- Invocation JSONL: contains `hil_request` + `hil_response` entries with pid
- Double-resolve: still returns EUSED
- Timeout path: records `outcome: "denied"` in JSONL, emits resolved event with pid
