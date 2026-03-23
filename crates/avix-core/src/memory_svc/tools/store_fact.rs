use chrono::Utc;
use serde_json::{json, Value};

use crate::error::AvixError;
use crate::memfs::VfsPath;
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
    let key: String = params["key"]
        .as_str()
        .ok_or_else(|| AvixError::ConfigParse("missing key".into()))?
        .into();
    let summary: String = params["summary"]
        .as_str()
        .ok_or_else(|| AvixError::ConfigParse("missing summary".into()))?
        .into();

    let confidence: Option<crate::memory_svc::MemoryConfidence> = params["confidence"]
        .as_str()
        .and_then(|s| serde_json::from_value(Value::String(s.to_string())).ok());

    let pinned = params["pinned"].as_bool().unwrap_or(false);

    let scope = params["scope"].as_str().unwrap_or("own");
    if scope != "own" {
        return Err(AvixError::ConfigParse(
            "crew scope not yet implemented".into(),
        ));
    }

    let path = MemoryRecord::vfs_path_semantic(&caller.owner, &caller.agent_name, &key);
    let replaced = svc
        .vfs()
        .exists(
            &VfsPath::parse(&path)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?,
        )
        .await;

    let now = Utc::now();
    let id = new_memory_id();
    let meta = MemoryRecordMetadata {
        id: id.clone(),
        record_type: MemoryRecordType::Semantic,
        agent_name: caller.agent_name.clone(),
        agent_pid: caller.pid,
        owner: caller.owner.clone(),
        created_at: now,
        updated_at: now,
        session_id: caller.session_id.clone(),
        tags: vec![],
        pinned,
    };
    let spec = MemoryRecordSpec {
        content: summary,
        outcome: None,
        related_goal: None,
        tools_used: vec![],
        key: Some(key.clone()),
        confidence,
        ttl_days: None,
        index: MemoryRecordIndex::default(),
    };
    let record = MemoryRecord::new(meta, spec);
    store::write_record(svc.vfs(), &path, &record).await?;

    Ok(json!({ "id": id, "key": key, "stored": true, "replaced": replaced }))
}
