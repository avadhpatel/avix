use chrono::Utc;
use serde_json::{json, Value};

use crate::error::AvixError;
use crate::memory_svc::{
    new_memory_id, MemoryRecord, MemoryRecordIndex, MemoryRecordMetadata, MemoryRecordSpec,
    MemoryRecordType,
};

use super::super::service::{CallerContext, MemoryService};
use super::super::store;

pub async fn handle(
    svc: &MemoryService,
    params: Value,
    caller: &CallerContext,
) -> Result<Value, AvixError> {
    let summary: String = params["summary"]
        .as_str()
        .ok_or_else(|| AvixError::ConfigParse("missing summary".into()))?
        .into();

    let outcome: Option<crate::memory_svc::MemoryOutcome> = params["outcome"]
        .as_str()
        .and_then(|s| serde_json::from_value(Value::String(s.to_string())).ok());

    let related_goal = params["relatedGoal"].as_str().map(String::from);

    let tags: Vec<String> = params["tags"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let pinned = params["pinned"].as_bool().unwrap_or(false);

    let scope = params["scope"].as_str().unwrap_or("own");
    if scope != "own" {
        // Crew scope handled in memory-gap-F
        return Err(AvixError::ConfigParse(
            "crew scope not yet implemented".into(),
        ));
    }

    let now = Utc::now();
    let id = new_memory_id();
    let meta = MemoryRecordMetadata {
        id: id.clone(),
        record_type: MemoryRecordType::Episodic,
        agent_name: caller.agent_name.clone(),
        agent_pid: caller.pid,
        owner: caller.owner.clone(),
        created_at: now,
        updated_at: now,
        session_id: caller.session_id.clone(),
        tags,
        pinned,
    };
    let spec = MemoryRecordSpec {
        content: summary,
        outcome,
        related_goal,
        tools_used: vec![],
        key: None,
        confidence: None,
        ttl_days: None,
        index: MemoryRecordIndex::default(),
    };
    let record = MemoryRecord::new(meta, spec);
    let path =
        MemoryRecord::vfs_path_episodic(&caller.owner, &caller.agent_name, &now, &id);
    store::write_record(svc.vfs(), &path, &record).await?;

    Ok(json!({ "id": id, "stored": true, "indexed": false }))
}
