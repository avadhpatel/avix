use serde::{Deserialize, Serialize};

use tracing::instrument;

/// Injected into tool call params when `caller_scoped: true` in service.yaml.
/// Available to the service as `params._caller`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CallerInfo {
    pub pid: u64,
    pub user: String,
    /// The caller's capability token string (for audit).
    pub token: String,
}

impl CallerInfo {
    /// Insert `_caller` into a JSON params object.
    /// No-op if `params` is not an object.
    #[instrument]
    pub fn inject_into(&self, params: &mut serde_json::Value) {
        if let serde_json::Value::Object(map) = params {
            map.insert(
                "_caller".into(),
                serde_json::to_value(self).unwrap_or_default(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(params["repo"], "org/repo");
    }

    #[test]
    fn inject_into_is_noop_for_non_object() {
        let caller = CallerInfo {
            pid: 1,
            user: "x".into(),
            token: "y".into(),
        };
        let mut params = serde_json::json!([1, 2, 3]);
        caller.inject_into(&mut params);
        assert!(params.is_array());
    }

    #[test]
    fn caller_info_serialises_correctly() {
        let c = CallerInfo {
            pid: 5,
            user: "bob".into(),
            token: "t".into(),
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["pid"], 5);
        assert_eq!(v["user"], "bob");
        assert_eq!(v["token"], "t");
    }

    #[test]
    fn inject_into_overwrites_existing_caller() {
        let caller = CallerInfo {
            pid: 99,
            user: "carol".into(),
            token: "tok-new".into(),
        };
        let mut params = serde_json::json!({ "_caller": "old" });
        caller.inject_into(&mut params);
        assert_eq!(params["_caller"]["pid"], 99);
    }
}
