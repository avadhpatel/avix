use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::gateway::atp::error::{AtpError, AtpErrorCode};

/// Per-connection replay guard that tracks seen command IDs.
/// A fresh instance is created for each WebSocket connection.
#[derive(Default, Clone)]
pub struct ReplayGuard {
    seen: Arc<Mutex<HashSet<String>>>,
}

impl ReplayGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a command ID. Returns `Err(EPARSE)` if already seen (replay attack).
    pub async fn check_and_register(&self, id: &str) -> Result<(), AtpError> {
        let mut guard = self.seen.lock().await;
        if guard.contains(id) {
            return Err(AtpError::new(
                AtpErrorCode::Eparse,
                format!("duplicate command id '{id}'"),
            ));
        }
        guard.insert(id.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn first_id_accepted() {
        let guard = ReplayGuard::new();
        assert!(guard.check_and_register("c-001").await.is_ok());
    }

    #[tokio::test]
    async fn duplicate_id_rejected() {
        let guard = ReplayGuard::new();
        guard.check_and_register("c-001").await.unwrap();
        let err = guard.check_and_register("c-001").await.unwrap_err();
        assert_eq!(err.code, AtpErrorCode::Eparse);
    }

    #[tokio::test]
    async fn different_ids_both_accepted() {
        let guard = ReplayGuard::new();
        assert!(guard.check_and_register("c-001").await.is_ok());
        assert!(guard.check_and_register("c-002").await.is_ok());
    }

    #[tokio::test]
    async fn many_ids_all_accepted() {
        let guard = ReplayGuard::new();
        for i in 0..100 {
            assert!(guard.check_and_register(&format!("c-{i:04}")).await.is_ok());
        }
    }

    #[tokio::test]
    async fn replay_error_has_eparse_code() {
        let guard = ReplayGuard::new();
        guard.check_and_register("dup").await.unwrap();
        let err = guard.check_and_register("dup").await.unwrap_err();
        assert_eq!(err.code, AtpErrorCode::Eparse);
        assert!(err.message.contains("dup"));
    }

    #[tokio::test]
    async fn clone_shares_state() {
        let guard = ReplayGuard::new();
        let cloned = guard.clone();
        guard.check_and_register("shared-id").await.unwrap();
        // The clone should see the same state
        let err = cloned.check_and_register("shared-id").await.unwrap_err();
        assert_eq!(err.code, AtpErrorCode::Eparse);
    }
}
