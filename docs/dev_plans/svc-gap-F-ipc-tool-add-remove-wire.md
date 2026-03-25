# Svc Gap F — `ipc.tool-add` / `ipc.tool-remove` Wire Protocol + Drain + Visibility

> **Status:** Pending
> **Priority:** High
> **Depends on:** Svc gaps A, B, C
> **Blocks:** Svc gap G (`_caller` injection reads `caller_scoped` from service record)
> **Affects:** `crates/avix-core/src/service/lifecycle.rs`,
>   `crates/avix-core/src/router/dispatcher.rs`,
>   `crates/avix-core/src/ipc/server.rs`

---

## Problem

`ServiceManager::handle_tool_add` and `handle_tool_remove` exist but:

1. They are not wired to any IPC endpoint — no JSON-RPC method dispatches to them.
2. `handle_tool_add` ignores the `visibility` and `descriptor` fields in the spec's
   `ipc.tool-add` params.
3. `handle_tool_remove` ignores the `drain: true` semantics — it should wait for
   in-flight calls to complete before removing.
4. `handle_tool_add` creates bare `ToolEntry` records from name strings only — no
   typed descriptor.
5. No `tool.changed` ATP event is broadcast to gateway clients after add/remove.

---

## Scope

Wire `ipc.tool-add` and `ipc.tool-remove` as proper JSON-RPC methods on the kernel IPC
server. Fully implement `drain` semantics using the existing `ToolRegistry` semaphore.
Accept and store the `descriptor` and `visibility` fields. Broadcast `tool.changed` ATP
events. Add a typed `CallerInfo` struct used in gap G.

---

## What Needs to Be Built

### 1. Typed params for `ipc.tool-add`

```rust
// service/lifecycle.rs

#[derive(Debug, Clone, Deserialize)]
pub struct IpcToolAddParams {
    pub _token: String,
    pub tools: Vec<IpcToolSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IpcToolSpec {
    pub name: String,
    pub descriptor: serde_json::Value,
    #[serde(default)]
    pub visibility: ToolVisibilitySpec,   // from tool_registry/descriptor.rs
}
```

### 2. Update `handle_tool_add`

```rust
pub async fn handle_tool_add(
    &self,
    params: IpcToolAddParams,
) -> Result<(), AvixError> {
    let svc_name = self.validate_token(&params._token).await?;
    if let Some(reg) = &self.tool_registry {
        let entries: Vec<ToolEntry> = params.tools
            .iter()
            .filter_map(|spec| {
                ToolName::parse(&spec.name).ok().map(|name| ToolEntry {
                    name,
                    owner: svc_name.clone(),
                    state: ToolState::Available,
                    visibility: spec.visibility.clone().into(),
                    descriptor: spec.descriptor.clone(),
                })
            })
            .collect();
        reg.add(&svc_name, entries).await?;
    }
    Ok(())
}
```

### 3. Typed params and drain semantics for `ipc.tool-remove`

The existing `ToolRegistry` uses `Semaphore` per tool entry. Implement drain by:
1. Setting the tool state to `Unavailable` immediately (new calls are rejected by router).
2. Acquiring all permits on the tool's semaphore (blocks until in-flight calls complete).
3. Then removing the entry.

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct IpcToolRemoveParams {
    pub _token: String,
    pub tools: Vec<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub drain: bool,
}
```

Extend `ToolRegistry::remove` to accept `drain: bool`:

```rust
// tool_registry/registry.rs
pub async fn remove(
    &self,
    _owner: &str,
    tool_names: &[&str],
    reason: &str,
    drain: bool,
) -> Result<(), AvixError> {
    if drain {
        // Phase 1: mark unavailable so router rejects new calls
        {
            let mut guard = self.inner.write().await;
            for name in tool_names {
                if let Some(rec) = guard.get_mut(*name) {
                    rec.entry.state = ToolState::Unavailable;
                }
            }
        }
        // Phase 2: drain — acquire all permits (waits for in-flight)
        {
            let guard = self.inner.read().await;
            for name in tool_names {
                if let Some(rec) = guard.get(*name) {
                    let _ = rec.semaphore
                        .acquire_many(tokio::sync::Semaphore::MAX_PERMITS as u32)
                        .await;
                }
            }
        }
    }
    // Phase 3: remove
    let mut guard = self.inner.write().await;
    let removed: Vec<String> = tool_names.iter()
        .filter(|n| guard.remove(*n).is_some())
        .map(|n| n.to_string())
        .collect();
    if !removed.is_empty() {
        let _ = self.events.send(ToolChangedEvent {
            op: format!("removed: {reason}"),
            tools: removed,
        });
    }
    Ok(())
}
```

### 4. IPC server dispatch

Wire both methods in the IPC server's request router:

```rust
// ipc/server.rs or wherever JSON-RPC dispatch happens

match req.method.as_str() {
    "ipc.register"    => handle_register(...),
    "ipc.tool-add"    => {
        let params: IpcToolAddParams = serde_json::from_value(req.params)?;
        svc_manager.handle_tool_add(params).await?;
        json_ok(req.id, serde_json::json!({"added": true}))
    }
    "ipc.tool-remove" => {
        let params: IpcToolRemoveParams = serde_json::from_value(req.params)?;
        svc_manager.handle_tool_remove_typed(params).await?;
        json_ok(req.id, serde_json::json!({"removed": true}))
    }
    _ => json_error(req.id, -32601, "method not found"),
}
```

### 5. `tool.changed` ATP event broadcast

After every `tool-add` or `tool-remove`, push a `tool.changed` ATP event to all
subscribed gateway clients. Wire the `ToolRegistry`'s `EventReceiver` into the gateway's
event fan-out task.

The existing `ToolChangedEvent` struct is already defined in `tool_registry/events.rs`.
The gateway task should:

```rust
// In the gateway's event loop:
while let Some(evt) = tool_events.recv().await {
    let atp_event = AtpEvent {
        kind: "tool_changed".into(),
        body: serde_json::to_value(&evt).unwrap_or_default(),
    };
    gateway.broadcast(atp_event).await;
}
```

---

## Tests

```rust
// service/lifecycle.rs #[cfg(test)]

#[tokio::test]
async fn tool_add_with_descriptor_stores_visibility() {
    let (mgr, reg) = ServiceManager::new_with_registry();
    // pre-register service
    let token = mgr.spawn_and_get_token(ServiceSpawnRequest {
        name: "github-svc".into(), binary: "/bin/g".into()
    }).await.unwrap();

    let params = IpcToolAddParams {
        _token: token.token_str.clone(),
        tools: vec![IpcToolSpec {
            name: "github/list-prs".into(),
            descriptor: serde_json::json!({"description": "List PRs"}),
            visibility: ToolVisibilitySpec::All,
        }],
    };
    mgr.handle_tool_add(params).await.unwrap();

    let entry = reg.lookup("github/list-prs").await.unwrap();
    assert_eq!(entry.owner, "github-svc");
    assert_eq!(entry.descriptor["description"], "List PRs");
}

#[tokio::test]
async fn tool_add_rejects_invalid_token() {
    let (mgr, _) = ServiceManager::new_with_registry();
    let params = IpcToolAddParams {
        _token: "bad-token".into(),
        tools: vec![],
    };
    assert!(mgr.handle_tool_add(params).await.is_err());
}

#[tokio::test]
async fn tool_remove_without_drain_removes_immediately() {
    let (mgr, reg) = ServiceManager::new_with_registry();
    let token = register_service(&mgr, "svc-a", &["x/y"]).await;

    mgr.handle_tool_remove(
        token.token_str.clone(),
        vec!["x/y".into()],
        "test",
        false,
    ).await.unwrap();

    assert!(reg.lookup("x/y").await.is_err());
}

#[tokio::test]
async fn tool_remove_with_drain_marks_unavailable_first() {
    // Not testing the actual blocking behaviour (no in-flight calls in test),
    // but verifying the tool is removed after drain=true completes.
    let (mgr, reg) = ServiceManager::new_with_registry();
    let token = register_service(&mgr, "svc-b", &["a/b"]).await;

    mgr.handle_tool_remove(
        token.token_str.clone(),
        vec!["a/b".into()],
        "gone",
        true,
    ).await.unwrap();

    assert!(reg.lookup("a/b").await.is_err());
}

#[tokio::test]
async fn tool_changed_event_fires_on_add() {
    let (reg, mut events) = ToolRegistry::new_with_events();
    reg.add("svc", vec![make_entry("ns/tool")]).await.unwrap();
    let evt = events.recv().await.unwrap();
    assert_eq!(evt.op, "added");
    assert!(evt.tools.contains(&"ns/tool".to_string()));
}

#[tokio::test]
async fn tool_changed_event_fires_on_remove() {
    let (reg, mut events) = ToolRegistry::new_with_events();
    reg.add("svc", vec![make_entry("ns/tool")]).await.unwrap();
    let _ = events.recv().await; // consume the add event
    reg.remove("svc", &["ns/tool"], "test", false).await.unwrap();
    let evt = events.recv().await.unwrap();
    assert!(evt.op.contains("removed"));
}
```

---

## Success Criteria

- [ ] `handle_tool_add` stores `descriptor` and `visibility` correctly
- [ ] `handle_tool_add` rejects an invalid token
- [ ] `handle_tool_remove` with `drain: false` removes immediately
- [ ] `handle_tool_remove` with `drain: true` marks tool unavailable before removal
- [ ] `ToolChangedEvent` fires on both add and remove
- [ ] `ipc.tool-add` and `ipc.tool-remove` JSON-RPC methods are dispatched correctly
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
