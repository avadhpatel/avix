/// Tool call dispatch — routes a tool call from an agent to the owning service.
///
/// Flow: validate tool → check capability → acquire in-flight guard →
///       check concurrency → resolve endpoint → inject _caller → forward via IPC.
use super::{capability, inject_caller, ConcurrencyLimiter};
use crate::error::AvixError;
use crate::ipc::{message::JsonRpcRequest, message::JsonRpcResponse, IpcClient};
use crate::process::ProcessTable;
use crate::router::registry::ServiceRegistry;
use crate::tool_registry::{ToolRegistry, ToolState};
use crate::types::Pid;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

// Spec §10 error codes
const ENOTFOUND_METHOD: i32 = -32601;
const EPERM: i32 = -32002;
const EUNAVAIL: i32 = -32005;
const ETIMEOUT: i32 = -32007;
const EBUSY: i32 = -32008;

const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_CONCURRENT: usize = 20;

pub struct RouterDispatcher {
    service_registry: Arc<ServiceRegistry>,
    tool_registry: Arc<ToolRegistry>,
    process_table: Arc<ProcessTable>,
    concurrency: ConcurrencyLimiter,
    call_timeout: Duration,
}

impl RouterDispatcher {
    pub fn new(
        service_registry: Arc<ServiceRegistry>,
        tool_registry: Arc<ToolRegistry>,
        process_table: Arc<ProcessTable>,
    ) -> Self {
        Self {
            service_registry,
            tool_registry,
            process_table,
            concurrency: ConcurrencyLimiter::new(DEFAULT_MAX_CONCURRENT),
            call_timeout: DEFAULT_CALL_TIMEOUT,
        }
    }

    pub fn with_max_concurrent(self, n: usize) -> Self {
        Self {
            concurrency: ConcurrencyLimiter::new(n),
            ..self
        }
    }

    pub fn with_call_timeout(self, d: Duration) -> Self {
        Self {
            call_timeout: d,
            ..self
        }
    }

    /// Dispatch a tool call from `caller_pid` to the service that owns `request.method`.
    ///
    /// Returns a `JsonRpcResponse` — either the service's response or an error response.
    pub async fn dispatch(
        &self,
        mut request: JsonRpcRequest,
        caller_pid: Pid,
        caller_user: &str,
        _caller_token: &str,
    ) -> JsonRpcResponse {
        let id = request.id.clone();

        // 1. Look up tool in ToolRegistry.
        let tool_entry = match self.tool_registry.lookup(&request.method).await {
            Ok(e) => e,
            Err(_) => {
                return JsonRpcResponse::err(
                    &id,
                    ENOTFOUND_METHOD,
                    &format!("tool '{}' not found", request.method),
                    None,
                )
            }
        };

        // 2. Reject if tool state is Unavailable.
        if tool_entry.state == ToolState::Unavailable {
            return JsonRpcResponse::err(&id, EUNAVAIL, "tool is unavailable", None);
        }

        // 3. Check caller capability.
        if let Err(e) =
            capability::check_capability(&request.method, caller_pid, &self.process_table).await
        {
            return JsonRpcResponse::err(&id, EPERM, &e.to_string(), None);
        }

        // 4. Acquire tool call guard (enables drain on remove).
        let _call_guard = match self.tool_registry.acquire(&request.method).await {
            Ok(g) => g,
            Err(e) => return JsonRpcResponse::err(&id, EUNAVAIL, &e.to_string(), None),
        };

        // 5. Acquire global concurrency slot — non-blocking → EBUSY.
        let _slot = match self.concurrency.try_acquire() {
            Some(g) => g,
            None => return JsonRpcResponse::err(&id, EBUSY, "dispatcher at max capacity", None),
        };

        // 6. Resolve owning service name then endpoint path.
        let endpoint: PathBuf = match self
            .service_registry
            .service_for_tool(&request.method)
            .await
        {
            Some(svc) => match self.service_registry.lookup(&svc).await {
                Some(ep) => ep.into(),
                None => {
                    return JsonRpcResponse::err(
                        &id,
                        EUNAVAIL,
                        &format!("endpoint for service '{svc}' not registered"),
                        None,
                    )
                }
            },
            None => {
                return JsonRpcResponse::err(
                    &id,
                    ENOTFOUND_METHOD,
                    &format!("no service registered for tool '{}'", request.method),
                    None,
                )
            }
        };

        // 7. Inject _caller into params.
        inject_caller(&mut request.params, caller_pid, caller_user);

        // 8. Forward via IpcClient.
        let client = IpcClient::new(endpoint).with_timeout(self.call_timeout);
        match client.call(request).await {
            Ok(resp) => resp,
            Err(AvixError::IpcTimeout) => {
                JsonRpcResponse::err(&id, ETIMEOUT, "call to service timed out", None)
            }
            Err(e) => JsonRpcResponse::err(&id, EUNAVAIL, &e.to_string(), None),
        }
    }
}
