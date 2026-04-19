use serde_json::{json, Value};

use crate::error::AvixError;
use crate::memory_svc::MemoryRecord;

use super::super::service::{CallerContext, MemoryService};
use super::super::store;

use tracing::instrument;

#[instrument]
pub async fn handle(
    svc: &MemoryService,
    params: Value,
    caller: &CallerContext,
) -> Result<Value, AvixError> {
    let key = params["key"]
        .as_str()
        .ok_or_else(|| AvixError::ConfigParse("missing key".into()))?;

    let path = MemoryRecord::vfs_path_semantic(&caller.owner, &caller.agent_name, key);
    match store::read_record(svc.vfs(), &path).await {
        Ok(record) => Ok(json!({
            "found": true,
            "record": {
                "id": record.metadata.id,
                "key": record.spec.key,
                "summary": record.spec.content,
                "confidence": record.spec.confidence,
                "updatedAt": record.metadata.updated_at,
                "pinned": record.metadata.pinned,
            }
        })),
        Err(_) => Ok(json!({ "found": false })),
    }
}
