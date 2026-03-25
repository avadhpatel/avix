# Svc Gap G — `_caller` Injection and `caller_scoped` Enforcement

> **Status:** Pending
> **Priority:** Medium
> **Depends on:** Svc gaps A (ServiceUnit with `caller_scoped` field), F (service records)
> **Blocks:** nothing (leaf for multi-user service security)
> **Affects:** `crates/avix-core/src/router/dispatcher.rs`,
>   `crates/avix-core/src/router/mod.rs`,
>   `crates/avix-core/src/service/lifecycle.rs`

---

## Problem

The router's `inject_caller` function is referenced in `router/dispatcher.rs` but the
implementation is incomplete: it doesn't read the `caller_scoped` flag from the service
record, and the `CallerInfo` struct is not typed. The architecture doc (`07-services.md
§ Multi-User Security`) requires the router to inject `_caller: { pid, user, token }`
into every tool call params when `caller_scoped: true`.

---

## Scope

Define `CallerInfo`, read `caller_scoped` from the service record (populated from
`ServiceUnit`), and enforce injection in the router dispatcher. Add `caller_scoped` to
`ServiceRecord` in `ServiceManager`. Test the injection path without a real socket.

---

## What Needs to Be Built

### 1. `router/caller.rs` — `CallerInfo`

```rust
use serde::{Deserialize, Serialize};
use crate::types::Pid;

/// Injected into every tool call when `caller_scoped: true` in service.unit.
/// Available to the service as `params._caller`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CallerInfo {
    pub pid: u64,
    pub user: String,
    pub token: String,    // the caller's CapabilityToken string (for audit)
}

impl CallerInfo {
    pub fn inject_into(&self, params: &mut serde_json::Value) {
        if let serde_json::Value::Object(map) = params {
            map.insert(
                "_caller".into(),
                serde_json::to_value(self).unwrap_or_default(),
            );
        }
    }
}
```

### 2. Add `caller_scoped` to `ServiceRecord`

```rust
// service/lifecycle.rs
struct ServiceRecord {
    token: ServiceToken,
    endpoint: Option<String>,
    caller_scoped: bool,    // ← new field, populated from ServiceUnit
}
```

Extend `spawn_and_get_token` to accept (or later: read from ServiceUnit) the
`caller_scoped` flag. Or add a `set_caller_scoped(name, value)` method called after
the unit is loaded.

Better: add a `ServiceSpawnRequest::from_unit(unit: &ServiceUnit)` constructor:

```rust
impl ServiceSpawnRequest {
    pub fn from_unit(unit: &ServiceUnit) -> Self {
        Self {
            name: unit.name.clone(),
            binary: unit.service.binary.clone(),
            caller_scoped: unit.capabilities.caller_scoped,
            max_concurrent: unit.service.max_concurrent,
        }
    }
}

pub struct ServiceSpawnRequest {
    pub name: String,
    pub binary: String,
    pub caller_scoped: bool,
    pub max_concurrent: u32,
}
```

### 3. `ServiceManager::is_caller_scoped`

```rust
pub async fn is_caller_scoped(&self, service_name: &str) -> bool {
    self.services.read().await
        .get(service_name)
        .map(|r| r.caller_scoped)
        .unwrap_or(false)
}
```

### 4. Router dispatcher — enforce injection

In `RouterDispatcher::dispatch`, after resolving the tool's owning service and before
forwarding to the service IPC endpoint:

```rust
// router/dispatcher.rs (in the dispatch method)

// Inject _caller if the service requires it.
let svc_name = tool_entry.owner.clone();
if self.service_manager.is_caller_scoped(&svc_name).await {
    let caller = build_caller_info(ctx, process_table)?;
    caller.inject_into(&mut request.params);
}
```

```rust
fn build_caller_info(
    ctx: &crate::syscall::SyscallContext,
    process_table: &crate::process::ProcessTable,
) -> Result<CallerInfo, AvixError> {
    let proc = process_table.get(ctx.caller_pid)?;
    Ok(CallerInfo {
        pid: ctx.caller_pid as u64,
        user: proc.owner.clone(),
        token: ctx.token.token_str.clone(),
    })
}
```

### 5. `router/mod.rs` exports

```rust
pub mod caller;
pub use caller::CallerInfo;
```

---

## Tests

```rust
// router/caller.rs #[cfg(test)]

#[test]
fn inject_into_adds_caller_field() {
    let caller = CallerInfo {
        pid: 42,
        user: "alice".into(),
        token: "tok-abc".into(),
    };
    let mut params = serde_json::json!({ "repo": "org/repo" });
    caller.inject_into(&mut params);
    assert_eq!(params["_caller"]["pid"], 42);
    assert_eq!(params["_caller"]["user"], "alice");
    assert_eq!(params["repo"], "org/repo");    // original field preserved
}

#[test]
fn inject_into_is_noop_for_non_object() {
    let caller = CallerInfo { pid: 1, user: "x".into(), token: "y".into() };
    let mut params = serde_json::json!([1, 2, 3]);  // array, not object
    caller.inject_into(&mut params);
    // should not panic and array unchanged
    assert!(params.is_array());
}

#[test]
fn caller_info_serialises_correctly() {
    let c = CallerInfo { pid: 5, user: "bob".into(), token: "t".into() };
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(v["pid"], 5);
    assert_eq!(v["user"], "bob");
}

// service/lifecycle.rs #[cfg(test)]

#[tokio::test]
async fn caller_scoped_flag_stored_from_spawn_request() {
    let mgr = ServiceManager::new_for_test();
    let req = ServiceSpawnRequest {
        name: "multi-svc".into(),
        binary: "/bin/m".into(),
        caller_scoped: true,
        max_concurrent: 20,
    };
    mgr.spawn_and_get_token(req).await.unwrap();
    assert!(mgr.is_caller_scoped("multi-svc").await);
}

#[tokio::test]
async fn caller_scoped_defaults_false() {
    let mgr = ServiceManager::new_for_test();
    let req = ServiceSpawnRequest {
        name: "plain-svc".into(),
        binary: "/bin/p".into(),
        caller_scoped: false,
        max_concurrent: 20,
    };
    mgr.spawn_and_get_token(req).await.unwrap();
    assert!(!mgr.is_caller_scoped("plain-svc").await);
}

#[tokio::test]
async fn spawn_request_from_unit_copies_caller_scoped() {
    use crate::service::unit::{ServiceUnit, CapabilitiesSection};
    let mut unit = make_test_unit("cs-svc");
    unit.capabilities.caller_scoped = true;
    let req = ServiceSpawnRequest::from_unit(&unit);
    assert!(req.caller_scoped);
    assert_eq!(req.max_concurrent, unit.service.max_concurrent);
}
```

---

## Success Criteria

- [ ] `CallerInfo::inject_into` adds `_caller` field to JSON params object
- [ ] `inject_into` is a no-op on non-object values
- [ ] `caller_scoped: true` in `ServiceSpawnRequest` is stored and readable via `is_caller_scoped`
- [ ] `ServiceSpawnRequest::from_unit` copies `caller_scoped` and `max_concurrent` from unit
- [ ] Router dispatcher calls `inject_into` when `is_caller_scoped` is true
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
