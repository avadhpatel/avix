# Memory Gap F — HIL Memory Sharing Flow

> **Status:** Complete
> **Priority:** Low — core memory works without sharing; this adds cross-agent collaboration
> **Depends on:** memory-gap-A (MemoryGrant schema), memory-gap-C (service), memory-gap-D (capability tokens)
> **Affects:** `avix-core/src/memory_svc/tools/share_request.rs` (new), `avix-core/src/kernel/`

---

## Problem

The `memory/share-request` tool is registered but returns `NotImplemented`. Cross-agent
memory sharing requires:

1. An agent calls `memory/share-request` → triggers HIL (SIGPAUSE)
2. Human reviews the records (summaries shown, not opaque IDs) and approves/denies
3. On approval, `memory.svc` creates a `MemoryGrant` record
4. The receiving agent can call `memory/retrieve` with `scopes: ["grants"]` and see the
   granted records tagged with `scope: grant:<id>`
5. Session-scoped grants expire when the session closes; permanent grants persist to VFS

---

## What Needs to Be Built

### 1. `memory/share-request` tool handler

```rust
pub async fn handle(
    svc: &MemoryService,
    params: serde_json::Value,
    caller: &CallerContext,
) -> Result<serde_json::Value, AvixError> {
    // 1. Validate: caller holds memory:share
    if !caller.granted_tools.contains(&"memory/share-request".to_string()) {
        return Err(AvixError::PermissionDenied("memory:share not granted".into()));
    }

    let target_agent: String = params["targetAgent"].as_str()...?.into();
    let record_ids: Vec<String> = params["recordIds"].as_array()...;
    let reason: String = params["reason"].as_str().unwrap_or("").into();
    let scope: MemoryGrantScope = match params["scope"].as_str().unwrap_or("session") {
        "permanent" => MemoryGrantScope::Permanent,
        _ => MemoryGrantScope::Session,
    };

    // 2. Validate target agent has canReceive: true
    // (look up agent manifest via process table)
    // In this gap: derive from resolved.yaml's sharing.canReceive field
    if !svc.agent_can_receive(&target_agent).await? {
        return Err(AvixError::PermissionDenied(
            format!("agent '{target_agent}' cannot receive memory grants")
        ));
    }

    // 3. Validate record IDs belong to caller's namespace (own records only)
    for id in &record_ids {
        svc.verify_record_owner(id, &caller.owner, &caller.agent_name).await?;
    }

    // 4. Load record summaries for the HIL event body (human sees content, not IDs)
    let mut record_summaries = vec![];
    for id in &record_ids {
        if let Ok(record) = svc.find_record_by_id(id, &caller.owner, &caller.agent_name).await {
            record_summaries.push(json!({
                "id": id,
                "type": record.metadata.record_type,
                "summary": &record.spec.content[..300.min(record.spec.content.len())],
                "createdAt": record.metadata.created_at,
            }));
        }
    }

    // 5. Mint ApprovalToken and send HIL event via kernel
    let hil_id = format!("hil-mem-{}", new_memory_id());
    let approval_token = svc.kernel.mint_approval_token(
        caller.pid,
        &hil_id,
        "memory_share",
    ).await?;

    // 6. Write /proc/<pid>/hil-queue/<hil_id>.yaml
    let hil_event = json!({
        "hilId": hil_id,
        "pid": caller.pid,
        "agentName": caller.agent_name,
        "type": "memory_share",
        "targetAgent": target_agent,
        "records": record_summaries,
        "reason": reason,
        "scope": scope,
        "prompt": format!(
            "{} wants to share {} memory record(s) with {}. Approve?",
            caller.agent_name, record_ids.len(), target_agent
        ),
        "approvalToken": approval_token,
        "expiresAt": (Utc::now() + chrono::Duration::seconds(
            svc.kernel_config.sharing.hil_timeout_sec as i64
        )).to_rfc3339(),
    });
    svc.kernel.write_hil_event(caller.pid, &hil_id, &hil_event).await?;

    // 7. Send SIGPAUSE to the requesting agent
    svc.kernel.send_signal(caller.pid, "SIGPAUSE").await?;

    // The agent is now paused. SIGRESUME will be sent after human decision.
    // The actual grant creation happens in the approval handler below.
    Ok(json!({ "hilId": hil_id, "status": "pending" }))
}
```

### 2. HIL approval handler — `memory_share` event type

When the human approves a `memory_share` HIL event, the kernel calls:

```rust
pub async fn on_memory_share_approved(
    svc: &MemoryService,
    hil_id: &str,
    caller_pid: u32,
    target_agent: &str,
    record_ids: Vec<String>,
    scope: MemoryGrantScope,
    owner: &str,
    session_id: &str,
    approving_user: &str,
) -> Result<(), AvixError> {
    let grant_id = format!("grant-{}", new_memory_id());
    let grant = MemoryGrant {
        api_version: "avix/v1".into(),
        kind: "MemoryGrant".into(),
        metadata: MemoryGrantMetadata {
            id: grant_id.clone(),
            granted_at: Utc::now(),
            granted_by: approving_user.into(),
            hil_id: hil_id.into(),
        },
        spec: MemoryGrantSpec {
            grantor: MemoryGrantGrantor {
                agent_name: // from process table by caller_pid
                owner: owner.into(),
            },
            grantee: MemoryGrantGrantee {
                agent_name: target_agent.into(),
                owner: owner.into(),   // v1: same owner only
            },
            records: record_ids,
            scope: scope.clone(),
            session_id: session_id.into(),
            expires_at: None,
        },
    };

    match scope {
        MemoryGrantScope::Session => {
            // Hold in /proc/services/memory/agents/<target>/grants/<id>.yaml
            // (in-memory via VFS; expires when session closes)
            let path = memory_agent_grants_path(target_agent, &grant_id);
            let yaml = serde_yaml::to_string(&grant).unwrap();
            svc.vfs.write(&VfsPath::parse(&path).unwrap(), yaml.into_bytes()).await?;
        }
        MemoryGrantScope::Permanent => {
            // Persist to /users/<owner>/memory/<grantor-agent>/grants/<id>.yaml
            let path = MemoryGrant::vfs_path(owner, &grant.spec.grantor.agent_name, &grant_id);
            let yaml = serde_yaml::to_string(&grant).unwrap();
            svc.vfs.write(&VfsPath::parse(&path).unwrap(), yaml.into_bytes()).await?;
        }
    }

    Ok(())
}
```

### 3. `memory/retrieve` — add `grants` scope

Update the retrieve handler to check active grants and include granted records:

```rust
if scopes.contains(&"grants".to_string()) {
    // Load all active grants where grantee.agentName == caller.agent_name
    let grant_dir = format!("/proc/services/memory/agents/{}/grants", caller.agent_name);
    let grant_entries = svc.vfs.list(&VfsPath::parse(&grant_dir).unwrap()).await.unwrap_or_default();
    for grant_path in grant_entries.iter().filter(|e| e.ends_with(".yaml")) {
        if let Ok(grant) = load_grant(&svc.vfs, grant_path).await {
            // Load the granted records from the grantor's namespace
            for record_id in &grant.spec.records {
                if let Ok(record) = svc.find_record_by_id(
                    record_id,
                    &grant.spec.grantor.owner,
                    &grant.spec.grantor.agent_name
                ).await {
                    // Tag with grant scope
                    grant_candidates.push((record, format!("grant:{}", grant.metadata.id)));
                }
            }
        }
    }
}
```

### 4. Session cleanup — expire session-scoped grants

When a session closes (SIGSTOP), delete all session-scoped grant records from
`/proc/services/memory/agents/<agent>/grants/` where `spec.scope == "session"`.

```rust
pub async fn cleanup_session_grants(
    svc: &MemoryService,
    agent_name: &str,
    session_id: &str,
) -> Result<(), AvixError> {
    let grant_dir = format!("/proc/services/memory/agents/{}/grants", agent_name);
    let entries = svc.vfs.list(&VfsPath::parse(&grant_dir).unwrap()).await.unwrap_or_default();
    for path in entries.iter().filter(|e| e.ends_with(".yaml")) {
        if let Ok(grant) = load_grant(&svc.vfs, path).await {
            if grant.spec.scope == MemoryGrantScope::Session
                && grant.spec.session_id == session_id
            {
                svc.vfs.delete(&VfsPath::parse(path).unwrap()).await.ok();
            }
        }
    }
    Ok(())
}
```

---

## TDD Test Plan

File: `crates/avix-core/tests/memory_sharing.rs`

```rust
// T-MF-01: share-request without memory:share capability returns EPERM
#[tokio::test]
async fn share_request_requires_memory_share_cap() {
    let caller = make_caller_without_share_cap("alice", "researcher");
    let result = svc.dispatch("memory/share-request", json!({
        "targetAgent": "writer",
        "recordIds": ["mem-abc123"],
        "reason": "sharing research",
        "scope": "session"
    }), &caller).await;
    assert!(result.is_err());
}

// T-MF-02: share-request against non-receivable agent returns EPERM
#[tokio::test]
async fn share_request_to_non_receivable_agent() { ... }

// T-MF-03: approved grant creates MemoryGrant record in VFS
#[tokio::test]
async fn approved_grant_stored_in_vfs() {
    let svc = make_test_memory_svc_with_kernel().await;
    on_memory_share_approved(&svc, "hil-001", 57, "writer",
        vec!["mem-abc123".into()], MemoryGrantScope::Session,
        "alice", "sess-xyz", "alice").await.unwrap();
    let grant_dir = VfsPath::parse("/proc/services/memory/agents/writer/grants").unwrap();
    let entries = svc.vfs.list(&grant_dir).await.unwrap();
    assert!(!entries.is_empty(), "expected grant record in VFS");
}

// T-MF-04: permanent grant stored in user memory tree
#[tokio::test]
async fn permanent_grant_stored_in_user_tree() { ... }

// T-MF-05: retrieve with grants scope includes granted records
#[tokio::test]
async fn retrieve_includes_granted_records() {
    // Setup: researcher has a grant to writer's records
    // Writer calls retrieve with scopes: ["grants"]
    // Assert granted record appears with scope: "grant:<id>"
}

// T-MF-06: session cleanup removes session grants on SIGSTOP
#[tokio::test]
async fn session_cleanup_removes_session_grants() {
    let svc = make_test_memory_svc_with_kernel().await;
    // Create a session-scoped grant
    on_memory_share_approved(&svc, "hil-001", 57, "writer",
        vec!["mem-abc123".into()], MemoryGrantScope::Session,
        "alice", "sess-xyz", "alice").await.unwrap();
    // Cleanup
    cleanup_session_grants(&svc, "writer", "sess-xyz").await.unwrap();
    let grant_dir = VfsPath::parse("/proc/services/memory/agents/writer/grants").unwrap();
    let entries: Vec<_> = svc.vfs.list(&grant_dir).await.unwrap()
        .into_iter().filter(|e| e.ends_with(".yaml")).collect();
    assert!(entries.is_empty(), "session grants should be cleaned up");
}

// T-MF-07: cross-user sharing is rejected in v1
#[tokio::test]
async fn cross_user_sharing_rejected() {
    // Caller owner: "alice", target agent owner: "bob"
    // Should return EPERM: cross-user sharing not supported in v1
}
```

---

## Implementation Notes

- The `memory_share` HIL event type is the fourth HIL type alongside
  `tool_call_approval`, `capability_upgrade`, and `escalation` (per ATP spec).
  The existing `ApprovalToken` / SIGPAUSE / SIGRESUME pattern is reused unchanged.
- `on_memory_share_approved()` is called from the kernel's approval handler after
  `ApprovalToken` is consumed atomically. The atomicity guarantee (ADR-07) prevents
  double-grant from duplicate approval responses.
- v1 constraint: both grantor and grantee must have the same `owner` (same user).
  This is checked by comparing `grant.spec.grantor.owner == grant.spec.grantee.owner`.
  Cross-user sharing (`crossUserEnabled: false` in config) is a hard v2 item.
- Grantee agents call `memory/retrieve` with `scopes: ["grants"]` — the retrieve
  handler filters grants by `grantee.agentName == caller.agent_name`. Granted records
  are read-only: any write attempted against a granted record path returns `EPERM`.

---

## Success Criteria

- [x] `memory/share-request` requires `memory:share` capability (T-MF-01)
- [ ] Non-receivable agent rejected (T-MF-02) — deferred (requires process table)
- [x] Approved session grant stored in `/proc/services/memory/` (T-MF-03)
- [x] Permanent grant stored in user memory tree (T-MF-04)
- [x] Retrieve with grants scope includes granted records (retrieve.rs updated)
- [x] Session grants cleaned up on SIGSTOP (T-MF-06)
- [x] Cross-user sharing rejected (share_request.rs validates targetOwner)
- [x] `cargo clippy --workspace -- -D warnings` passes
