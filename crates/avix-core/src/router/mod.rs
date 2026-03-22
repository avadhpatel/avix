pub mod concurrency;
pub mod registry;

pub use concurrency::{CallerScopedLimiter, ConcurrencyGuard, ConcurrencyLimiter};
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
