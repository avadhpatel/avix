use crate::types::Pid;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceToken {
    pub token_str: String,
    pub service_name: String,
    pub pid: Pid,
}
