use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::token::ServiceToken;
use crate::error::AvixError;
use crate::tool_registry::entry::ToolEntry;
use crate::tool_registry::ToolRegistry;
use crate::types::{
    tool::{ToolName, ToolState, ToolVisibility},
    Pid,
};

pub struct ServiceSpawnRequest {
    pub name: String,
    pub binary: String,
}

pub struct IpcRegisterRequest {
    pub token: String,
    pub name: String,
    pub endpoint: String,
    pub tools: Vec<String>,
}

#[derive(Debug)]
pub struct IpcRegisterResult {
    pub registered: bool,
    pub pid: Pid,
}

struct ServiceRecord {
    token: ServiceToken,
    endpoint: Option<String>,
}

pub struct ServiceManager {
    services: Arc<RwLock<HashMap<String, ServiceRecord>>>,
    token_to_svc: Arc<RwLock<HashMap<String, String>>>,
    pid_counter: Arc<std::sync::atomic::AtomicU32>,
    tool_registry: Option<Arc<ToolRegistry>>,
    runtime_dir: PathBuf,
}

impl ServiceManager {
    pub fn new_for_test(runtime_dir: PathBuf) -> Self {
        Self {
            services: Arc::new(RwLock::new(HashMap::new())),
            token_to_svc: Arc::new(RwLock::new(HashMap::new())),
            pid_counter: Arc::new(std::sync::atomic::AtomicU32::new(10)),
            tool_registry: None,
            runtime_dir,
        }
    }

    pub fn new_with_registry(runtime_dir: PathBuf) -> (Self, Arc<ToolRegistry>) {
        let reg = Arc::new(ToolRegistry::new());
        let mgr = Self {
            services: Arc::new(RwLock::new(HashMap::new())),
            token_to_svc: Arc::new(RwLock::new(HashMap::new())),
            pid_counter: Arc::new(std::sync::atomic::AtomicU32::new(10)),
            tool_registry: Some(Arc::clone(&reg)),
            runtime_dir,
        };
        (mgr, reg)
    }

    pub async fn spawn_and_get_token(
        &self,
        req: ServiceSpawnRequest,
    ) -> Result<ServiceToken, AvixError> {
        let pid = Pid::new(
            self.pid_counter
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        );
        let token_str = format!("svc-token-{}", Uuid::new_v4());
        let token = ServiceToken {
            token_str: token_str.clone(),
            service_name: req.name.clone(),
            pid,
        };
        let record = ServiceRecord {
            token: token.clone(),
            endpoint: None,
        };
        self.services.write().await.insert(req.name.clone(), record);
        self.token_to_svc.write().await.insert(token_str, req.name);
        Ok(token)
    }

    pub async fn handle_ipc_register(
        &self,
        req: IpcRegisterRequest,
    ) -> Result<IpcRegisterResult, AvixError> {
        let svc_name = self.validate_token(&req.token).await?;
        if svc_name != req.name {
            return Err(AvixError::CapabilityDenied("token/name mismatch".into()));
        }
        let mut guard = self.services.write().await;
        let record = guard
            .get_mut(&svc_name)
            .ok_or_else(|| AvixError::ConfigParse("service not found".into()))?;
        record.endpoint = Some(req.endpoint);
        let pid = record.token.pid;
        Ok(IpcRegisterResult {
            registered: true,
            pid,
        })
    }

    async fn validate_token(&self, token_str: &str) -> Result<String, AvixError> {
        self.token_to_svc
            .read()
            .await
            .get(token_str)
            .cloned()
            .ok_or_else(|| AvixError::CapabilityDenied("invalid service token".into()))
    }

    pub async fn service_env(&self, name: &str) -> Result<HashMap<String, String>, AvixError> {
        let guard = self.services.read().await;
        guard
            .get(name)
            .ok_or_else(|| AvixError::ConfigParse(format!("service not found: {name}")))?;

        let token = guard[name].token.token_str.clone();
        let pid = guard[name].token.pid.as_u32();

        let mut env = HashMap::new();
        #[cfg(unix)]
        {
            env.insert("AVIX_KERNEL_SOCK".into(), format!("{}/kernel.sock", self.runtime_dir.display()));
            env.insert("AVIX_ROUTER_SOCK".into(), format!("{}/router.sock", self.runtime_dir.display()));
            env.insert(
                "AVIX_SVC_SOCK".into(),
                format!("{}/services/{name}-{pid}.sock", self.runtime_dir.display()),
            );
        }
        #[cfg(windows)]
        {
            env.insert("AVIX_KERNEL_SOCK".into(), r"\\.\pipe\avix-kernel".into());
            env.insert("AVIX_ROUTER_SOCK".into(), r"\\.\pipe\avix-router".into());
            env.insert(
                "AVIX_SVC_SOCK".into(),
                format!(r"\\.\pipe\avix-svc-{name}-{pid}"),
            );
        }
        env.insert("AVIX_SVC_TOKEN".into(), token);
        Ok(env)
    }

    pub async fn handle_tool_add(
        &self,
        token_str: String,
        tool_names: Vec<String>,
    ) -> Result<(), AvixError> {
        let svc_name = self.validate_token(&token_str).await?;
        if let Some(reg) = &self.tool_registry {
            let entries: Vec<ToolEntry> = tool_names
                .iter()
                .filter_map(|n| {
                    ToolName::parse(n).ok().map(|name| ToolEntry {
                        name,
                        owner: svc_name.clone(),
                        state: ToolState::Available,
                        visibility: ToolVisibility::All,
                        descriptor: serde_json::json!({"name": n}),
                    })
                })
                .collect();
            reg.add(&svc_name, entries).await?;
        }
        Ok(())
    }

    pub async fn handle_tool_remove(
        &self,
        token_str: String,
        tool_names: Vec<String>,
        reason: &str,
        drain: bool,
    ) -> Result<(), AvixError> {
        let svc_name = self.validate_token(&token_str).await?;
        if let Some(reg) = &self.tool_registry {
            let refs: Vec<&str> = tool_names.iter().map(|s| s.as_str()).collect();
            reg.remove(&svc_name, &refs, reason, drain).await?;
        }
        Ok(())
    }
}
