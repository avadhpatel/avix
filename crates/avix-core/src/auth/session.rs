use crate::types::Role;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub session_id: String,
    pub identity_name: String,
    pub role: Role,
    pub created_at: Instant,
    pub ttl: Duration,
}

impl SessionEntry {
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.ttl
    }
}
