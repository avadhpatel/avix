pub mod capability;
pub mod concurrency;
pub mod dispatcher;
pub mod mangle;
pub mod registry;

pub use capability::{check_capability, ALWAYS_PRESENT};
pub use concurrency::{CallerScopedLimiter, ConcurrencyGuard, ConcurrencyLimiter};
pub use dispatcher::RouterDispatcher;
pub use mangle::{mangle, unmangle, validate_tool_name};
pub use registry::ServiceRegistry;

use crate::types::Pid;

pub fn inject_caller(params: &mut serde_json::Value, pid: Pid, user: &str) {
    if let Some(obj) = params.as_object_mut() {
        obj.insert(
            "_caller".to_string(),
            serde_json::json!({
                "pid": pid.as_u32(),
                "user": user,
            }),
        );
    }
}
