/// Helper for service authors: start a job-style (async) tool call.
///
/// Allocates a job ID, transitions it to Running, then spawns a background task.
/// Returns the job ID immediately — the caller should return it to the IPC caller.
///
/// The `work` closure receives `(job_id, registry_ref)` and is responsible for
/// calling `registry.write().await.progress(...)`, `complete(...)`, or `fail(...)`.
use std::future::Future;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::jobs::registry::JobRegistry;
use crate::types::Pid;

pub async fn start_job<F, Fut>(
    tool: &str,
    owner_pid: Pid,
    registry: Arc<RwLock<JobRegistry>>,
    work: F,
) -> String
where
    F: FnOnce(String, Arc<RwLock<JobRegistry>>) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let job_id = {
        let mut reg = registry.write().await;
        let id = reg.create(tool, owner_pid);
        reg.start(&id).expect("job was just created; start must succeed");
        id
    };

    let id_clone = job_id.clone();
    let reg_clone = registry.clone();
    tokio::spawn(async move {
        work(id_clone, reg_clone).await;
    });

    job_id
}
