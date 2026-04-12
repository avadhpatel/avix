use serde::{Deserialize, Serialize};

use crate::service::ServiceSummary;
use crate::tool_registry::ToolSummary;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolListResponse {
    pub total: usize,
    pub available: usize,
    pub unavailable: usize,
    pub tools: Vec<ToolSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceListResponse {
    pub total: usize,
    pub running: usize,
    pub starting: usize,
    pub services: Vec<ServiceSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub pid: u64,
    pub name: String,
    pub goal: String,
    pub session_id: String,
    pub spawned_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentsYaml {
    pub agents: Vec<AgentRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveAgent {
    pub pid: u64,
    pub name: String,
    pub status: String,
    pub goal: String,
}