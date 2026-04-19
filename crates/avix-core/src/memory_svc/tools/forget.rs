use serde_json::{json, Value};

use crate::error::AvixError;

use super::super::service::{find_record_by_id, CallerContext, MemoryService};
use super::super::store;

use tracing::instrument;

#[instrument]
pub async fn handle(
    svc: &MemoryService,
    params: Value,
    caller: &CallerContext,
) -> Result<Value, AvixError> {
    let ids: Vec<String> = params["ids"]
        .as_array()
        .ok_or_else(|| AvixError::ConfigParse("missing ids array".into()))?
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();

    let mut deleted = vec![];
    let mut not_found = vec![];

    for id in &ids {
        if let Some(path) =
            find_record_by_id(svc.vfs(), &caller.owner, &caller.agent_name, id).await
        {
            store::delete_record(svc.vfs(), &path).await?;
            deleted.push(id.clone());
        } else {
            not_found.push(id.clone());
        }
    }

    Ok(json!({ "deleted": deleted, "notFound": not_found }))
}
