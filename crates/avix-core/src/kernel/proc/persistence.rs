use std::path::PathBuf;

use tracing::instrument;

use crate::error::AvixError;

use super::types::{AgentRecord, AgentsYaml};

#[instrument(skip(path))]
pub async fn load_agents_yaml(path: &PathBuf) -> Result<AgentsYaml, AvixError> {
    if !path.exists() {
        return Ok(AgentsYaml { agents: Vec::new() });
    }
    let yaml = std::fs::read_to_string(path).map_err(|e| AvixError::Io(e.to_string()))?;
    serde_yaml::from_str(&yaml).map_err(|e| AvixError::ConfigParse(e.to_string()))
}

#[instrument(skip(path, agents))]
pub async fn save_agents_yaml(path: &PathBuf, agents: &AgentsYaml) -> Result<(), AvixError> {
    let yaml = serde_yaml::to_string(agents).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, &yaml).map_err(|e| AvixError::Io(e.to_string()))?;
    std::fs::rename(&tmp_path, path).map_err(|e| AvixError::Io(e.to_string()))?;
    Ok(())
}

#[instrument(skip(path))]
pub async fn persist_agent_record(
    path: &PathBuf,
    pid: u64,
    name: &str,
    goal: &str,
    session_id: &str,
) -> Result<(), AvixError> {
    let mut agents = load_agents_yaml(path).await.unwrap_or_default();

    let record = AgentRecord {
        pid,
        name: name.to_string(),
        goal: goal.to_string(),
        session_id: session_id.to_string(),
        spawned_at: chrono::Utc::now(),
    };

    if let Some(existing) = agents.agents.iter_mut().find(|a| a.pid == pid) {
        *existing = record;
    } else {
        agents.agents.push(record);
    }

    save_agents_yaml(path, &agents).await
}

pub async fn remove_agent_record(path: &PathBuf, pid: u64) -> Result<(), AvixError> {
    let mut agents = load_agents_yaml(path).await.unwrap_or_default();
    agents.agents.retain(|a| a.pid != pid);
    save_agents_yaml(path, &agents).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn load_agents_yaml_returns_empty_when_not_exists() {
        let path = PathBuf::from("/nonexistent/path.yaml");
        let result = load_agents_yaml(&path).await.unwrap();
        assert!(result.agents.is_empty());
    }

    #[tokio::test]
    async fn save_and_load_agents_yaml() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("agents.yaml");
        
        let agents = AgentsYaml {
            agents: vec![AgentRecord {
                pid: 42,
                name: "test".to_string(),
                goal: "test goal".to_string(),
                session_id: "sess-1".to_string(),
                spawned_at: chrono::Utc::now(),
            }],
        };
        
        save_agents_yaml(&path, &agents).await.unwrap();
        
        let loaded = load_agents_yaml(&path).await.unwrap();
        assert_eq!(loaded.agents.len(), 1);
        assert_eq!(loaded.agents[0].pid, 42);
    }

    #[tokio::test]
    async fn remove_agent_record_removes_entry() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("agents.yaml");
        
        let agents = AgentsYaml {
            agents: vec![AgentRecord {
                pid: 1,
                name: "test".to_string(),
                goal: "goal".to_string(),
                session_id: "sess-1".to_string(),
                spawned_at: chrono::Utc::now(),
            }],
        };
        save_agents_yaml(&path, &agents).await.unwrap();
        
        remove_agent_record(&path, 1).await.unwrap();
        
        let loaded = load_agents_yaml(&path).await.unwrap();
        assert!(loaded.agents.is_empty());
    }
}