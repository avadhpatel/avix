use serde_json::{json, Value};

use crate::error::AvixError;
use crate::memory_svc::UserPreferenceModel;

use super::super::service::{CallerContext, MemoryService};
use super::super::store;

use tracing::instrument;

#[instrument]
pub async fn handle(
    svc: &MemoryService,
    _params: Value,
    caller: &CallerContext,
) -> Result<Value, AvixError> {
    let path = UserPreferenceModel::vfs_path(&caller.owner, &caller.agent_name);
    match store::read_preference_model(svc.vfs(), &path).await {
        Ok(model) => Ok(json!({ "found": true, "model": model })),
        Err(_) => Ok(json!({ "found": false })),
    }
}
