use serde_json::{json, Value};

use crate::error::AvixError;

use super::super::schema::MemoryRecord;
use super::super::search;
use super::super::service::{CallerContext, MemoryService};
use super::super::sharing::load_grant;
use super::super::store;

pub async fn handle(
    svc: &MemoryService,
    params: Value,
    caller: &CallerContext,
) -> Result<Value, AvixError> {
    let query = params["query"]
        .as_str()
        .ok_or_else(|| AvixError::ConfigParse("missing query".into()))?;
    let default_limit = svc.default_retrieve_limit() as u64;
    let limit = params["limit"].as_u64().unwrap_or(default_limit).min(20) as usize;

    let types: Vec<String> = params["types"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| vec!["episodic".to_string(), "semantic".to_string()]);

    let scopes: Vec<String> = params["scopes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // ── Own records ───────────────────────────────────────────────────────────

    let mut own_candidates: Vec<MemoryRecord> = vec![];

    if types.contains(&"episodic".to_string()) {
        let dir = format!(
            "/users/{}/memory/{}/episodic",
            caller.owner, caller.agent_name
        );
        own_candidates.extend(store::list_records(svc.vfs(), &dir).await?);
    }
    if types.contains(&"semantic".to_string()) {
        let dir = format!(
            "/users/{}/memory/{}/semantic",
            caller.owner, caller.agent_name
        );
        own_candidates.extend(store::list_records(svc.vfs(), &dir).await?);
    }

    let total_own = own_candidates.len();
    let ranked_own = search::bm25_rank(&own_candidates, query, limit);

    let mut records_json: Vec<Value> = ranked_own
        .iter()
        .map(|r| record_to_json(r, "own"))
        .collect();

    // ── Granted records ───────────────────────────────────────────────────────

    if scopes.contains(&"grants".to_string()) {
        let grant_dir = format!("/proc/services/memory/agents/{}/grants", caller.agent_name);
        let grant_entries = svc
            .vfs()
            .list(
                &crate::memfs::VfsPath::parse(&grant_dir)
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?,
            )
            .await
            .unwrap_or_default();

        for filename in grant_entries.iter().filter(|e| e.ends_with(".yaml")) {
            let grant_path = format!("{}/{}", grant_dir, filename);
            let grant = match load_grant(svc, &grant_path).await {
                Ok(g) => g,
                Err(_) => continue,
            };

            for record_id in &grant.spec.records {
                let granted_record = super::super::service::find_record_by_id(
                    svc.vfs(),
                    &grant.spec.grantor.owner,
                    &grant.spec.grantor.agent_name,
                    record_id,
                )
                .await;

                if let Some(record_path) = granted_record {
                    if let Ok(record) = store::read_record(svc.vfs(), &record_path).await {
                        let scope_tag = format!("grant:{}", grant.metadata.id);
                        records_json.push(record_to_json(&record, &scope_tag));
                    }
                }
            }
        }
    }

    let returned = records_json.len();
    Ok(json!({
        "records": records_json,
        "totalCandidates": total_own,
        "returned": returned,
    }))
}

fn record_to_json(r: &MemoryRecord, scope: &str) -> Value {
    let summary_len = r.spec.content.len().min(200);
    json!({
        "id": r.metadata.id,
        "type": r.metadata.record_type,
        "scope": scope,
        "summary": &r.spec.content[..summary_len],
        "tags": r.metadata.tags,
        "createdAt": r.metadata.created_at,
        "pinned": r.metadata.pinned,
    })
}
