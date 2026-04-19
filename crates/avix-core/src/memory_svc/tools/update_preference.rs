use chrono::Utc;
use serde_json::{json, Value};

use crate::error::AvixError;
use crate::memory_svc::{
    UserPreferenceModel, UserPreferenceModelMetadata, UserPreferenceModelSpec,
};

use super::super::service::{CallerContext, MemoryService};
use super::super::store;

use tracing::instrument;

#[instrument]
pub async fn handle(
    svc: &MemoryService,
    params: Value,
    caller: &CallerContext,
) -> Result<Value, AvixError> {
    let path = UserPreferenceModel::vfs_path(&caller.owner, &caller.agent_name);
    let mut model = match store::read_preference_model(svc.vfs(), &path).await {
        Ok(m) => m,
        Err(_) => UserPreferenceModel::new(
            UserPreferenceModelMetadata {
                agent_name: caller.agent_name.clone(),
                owner: caller.owner.clone(),
                updated_at: Utc::now(),
            },
            UserPreferenceModelSpec {
                summary: String::new(),
                structured: Default::default(),
                corrections: vec![],
            },
        ),
    };

    if let Some(s) = params["summary"].as_str() {
        model.spec.summary = s.into();
    }

    // Merge structured fields if provided
    if let Some(obj) = params["structured"].as_object() {
        if let Some(v) = obj.get("outputFormat").and_then(|v| v.as_str()) {
            model.spec.structured.output_format = Some(v.into());
        }
        if let Some(v) = obj.get("preferredLength").and_then(|v| v.as_str()) {
            model.spec.structured.preferred_length = Some(v.into());
        }
        if let Some(v) = obj.get("tonePreference").and_then(|v| v.as_str()) {
            model.spec.structured.tone_preference = Some(v.into());
        }
        if let Some(v) = obj.get("timezone").and_then(|v| v.as_str()) {
            model.spec.structured.timezone = Some(v.into());
        }
        if let Some(v) = obj.get("primaryLanguage").and_then(|v| v.as_str()) {
            model.spec.structured.primary_language = Some(v.into());
        }
        if let Some(v) = obj.get("proactiveUpdates").and_then(|v| v.as_bool()) {
            model.spec.structured.proactive_updates = Some(v);
        }
    }

    model.metadata.updated_at = Utc::now();
    store::write_preference_model(svc.vfs(), &path, &model).await?;

    Ok(json!({ "updated": true }))
}
