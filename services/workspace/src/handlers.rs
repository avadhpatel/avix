//! workspace.svc — tool handlers
//!
//! These handlers use IPC to call kernel syscalls for VFS operations.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::debug;

use crate::error::WorkspaceError;

pub fn extract_caller_user(params: &serde_json::Value) -> String {
    params
        .get("_caller")
        .and_then(|c| c.get("user"))
        .and_then(|u| u.as_str())
        .unwrap_or("anonymous")
        .to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    pub size: u64,
    pub created: Option<String>,
    pub modified: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ListParams {
    pub project: Option<String>,
    pub recursive: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListResponse {
    pub entries: Vec<FileEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReadParams {
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadResponse {
    pub content: String,
    pub metadata: FileMetadata,
}

#[derive(Debug, Clone, Serialize)]
pub struct InfoResponse {
    pub root: String,
    pub projects: Vec<String>,
    pub default_project: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct WriteParams {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WriteResponse {
    pub path: String,
    pub bytes_written: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct DeleteParams {
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeleteResponse {
    pub path: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateProjectParams {
    pub project_name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub template: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateProjectResponse {
    pub path: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct SnapshotParams {
    pub project: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotResponse {
    pub path: String,
    pub files: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct SearchParams {
    pub query: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub search_type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub path: String,
    pub line: Option<u32>,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct SetDefaultParams {
    pub project: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetDefaultResponse {
    pub project: String,
}

pub struct VfsClient {
    kernel_sock: PathBuf,
}

impl VfsClient {
    pub fn new(kernel_sock: PathBuf) -> Self {
        Self { kernel_sock }
    }

    pub async fn list(&self, path: &str) -> Result<Vec<String>, WorkspaceError> {
        let client = avix_core::ipc::IpcClient::new(self.kernel_sock.clone());
        let req = avix_core::ipc::message::JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "1".into(),
            method: "kernel/fs/list".into(),
            params: serde_json::json!({ "path": path }),
        };

        let resp = client
            .call(req)
            .await
            .map_err(|e| WorkspaceError::Ipc(e.to_string()))?;

        if let Some(err) = resp.error {
            return Err(WorkspaceError::Ipc(err.message));
        }

        let entries: Vec<String> = resp
            .result
            .and_then(|v: serde_json::Value| v.get("entries").cloned())
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();

        Ok(entries)
    }

    pub async fn read(&self, path: &str) -> Result<String, WorkspaceError> {
        let client = avix_core::ipc::IpcClient::new(self.kernel_sock.clone());
        let req = avix_core::ipc::message::JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "1".into(),
            method: "kernel/fs/read".into(),
            params: serde_json::json!({ "path": path }),
        };

        let resp = client
            .call(req)
            .await
            .map_err(|e| WorkspaceError::Ipc(e.to_string()))?;

        if let Some(err) = resp.error {
            return Err(WorkspaceError::Ipc(err.message));
        }

        let content: String = resp
            .result
            .and_then(|v: serde_json::Value| v.get("content").cloned())
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();

        Ok(content)
    }

    pub async fn write(&self, path: &str, content: &str) -> Result<usize, WorkspaceError> {
        let client = avix_core::ipc::IpcClient::new(self.kernel_sock.clone());
        let req = avix_core::ipc::message::JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "1".into(),
            method: "kernel/fs/write".into(),
            params: serde_json::json!({ "path": path, "content": content }),
        };

        let resp = client
            .call(req)
            .await
            .map_err(|e| WorkspaceError::Ipc(e.to_string()))?;

        if let Some(err) = resp.error {
            return Err(WorkspaceError::Ipc(err.message));
        }

        let bytes_written: usize = resp
            .result
            .and_then(|v: serde_json::Value| v.get("bytes_written").cloned())
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(content.len());

        Ok(bytes_written)
    }

    pub async fn delete(&self, path: &str) -> Result<bool, WorkspaceError> {
        let client = avix_core::ipc::IpcClient::new(self.kernel_sock.clone());
        let req = avix_core::ipc::message::JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "1".into(),
            method: "kernel/fs/delete".into(),
            params: serde_json::json!({ "path": path }),
        };

        let resp = client
            .call(req)
            .await
            .map_err(|e| WorkspaceError::Ipc(e.to_string()))?;

        if let Some(err) = resp.error {
            return Err(WorkspaceError::Ipc(err.message));
        }

        let deleted: bool = resp
            .result
            .and_then(|v: serde_json::Value| v.get("deleted").cloned())
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Ok(deleted)
    }
}

pub struct WorkspaceHandlers {
    vfs_client: Arc<VfsClient>,
    caller_user: RwLock<String>,
}

impl WorkspaceHandlers {
    pub fn new(kernel_sock: PathBuf) -> Self {
        let vfs_client = Arc::new(VfsClient::new(kernel_sock));
        Self {
            vfs_client,
            caller_user: RwLock::new("anonymous".to_string()),
        }
    }

    pub fn set_caller_from_params(&self, params: &serde_json::Value) {
        let user = extract_caller_user(params);
        let mut guard = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.caller_user.write())
        });
        *guard = user;
    }

    #[allow(dead_code)]
    pub fn set_caller_user(&self, user: String) {
        let mut guard = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.caller_user.write())
        });
        *guard = user;
    }

    async fn get_workspace_root(&self) -> String {
        let user = self.caller_user.read().await;
        format!("/users/{}/workspace", user)
    }

    pub async fn handle_list(&self, params: ListParams) -> Result<ListResponse, WorkspaceError> {
        let workspace_root = self.get_workspace_root().await;

        let target_path = params
            .project
            .map(|p| format!("{}/{}", workspace_root, p))
            .unwrap_or(workspace_root);

        debug!(path = %target_path, "workspace/list");

        let names = match self.vfs_client.list(&target_path).await {
            Ok(names) => names,
            Err(WorkspaceError::Ipc(_)) => vec![],
            Err(e) => return Err(e),
        };

        let entries: Vec<FileEntry> = names
            .into_iter()
            .map(|name| {
                let full_path = format!("{}/{}", target_path, name);
                FileEntry {
                    path: full_path.clone(),
                    name,
                    is_dir: false,
                    size: 0,
                    modified: None,
                }
            })
            .collect();

        Ok(ListResponse { entries })
    }

    pub async fn handle_read(&self, params: ReadParams) -> Result<ReadResponse, WorkspaceError> {
        let workspace_root = self.get_workspace_root().await;

        let target_path = if params.path.starts_with('/') {
            if !params.path.starts_with(&workspace_root) {
                return Err(WorkspaceError::PathOutsideWorkspace(params.path));
            }
            params.path.clone()
        } else {
            format!("{}/{}", workspace_root, params.path)
        };

        debug!(path = %target_path, "workspace/read");

        let content = self
            .vfs_client
            .read(&target_path)
            .await
            .map_err(|_| WorkspaceError::FileNotFound(target_path.clone()))?;

        Ok(ReadResponse {
            content,
            metadata: FileMetadata {
                size: 0,
                created: None,
                modified: None,
            },
        })
    }

    pub async fn handle_info(&self) -> Result<InfoResponse, WorkspaceError> {
        let workspace_root = self.get_workspace_root().await;

        debug!(root = %workspace_root, "workspace/info");

        let projects = match self.vfs_client.list(&workspace_root).await {
            Ok(names) => names,
            Err(WorkspaceError::Ipc(_)) => vec![],
            Err(e) => return Err(e),
        };

        Ok(InfoResponse {
            root: workspace_root,
            projects,
            default_project: None,
        })
    }

    pub async fn handle_write(&self, params: WriteParams) -> Result<WriteResponse, WorkspaceError> {
        let workspace_root = self.get_workspace_root().await;

        let target_path = if params.path.starts_with('/') {
            if !params.path.starts_with(&workspace_root) {
                return Err(WorkspaceError::PathOutsideWorkspace(params.path));
            }
            params.path.clone()
        } else {
            format!("{}/{}", workspace_root, params.path)
        };

        debug!(path = %target_path, "workspace/write");

        let _before_content = match self.vfs_client.read(&target_path).await {
            Ok(c) => Some(c),
            Err(WorkspaceError::FileNotFound(_)) => None,
            Err(e) => return Err(e),
        };

        let bytes_written = self.vfs_client.write(&target_path, &params.content).await?;

        debug!(bytes = bytes_written, "file written");

        Ok(WriteResponse {
            path: target_path,
            bytes_written,
        })
    }

    pub async fn handle_delete(
        &self,
        params: DeleteParams,
    ) -> Result<DeleteResponse, WorkspaceError> {
        let workspace_root = self.get_workspace_root().await;

        let target_path = if params.path.starts_with('/') {
            if !params.path.starts_with(&workspace_root) {
                return Err(WorkspaceError::PathOutsideWorkspace(params.path));
            }
            params.path.clone()
        } else {
            format!("{}/{}", workspace_root, params.path)
        };

        debug!(path = %target_path, "workspace/delete");

        let deleted = self.vfs_client.delete(&target_path).await?;

        Ok(DeleteResponse {
            path: target_path,
            deleted,
        })
    }

    pub async fn handle_create_project(
        &self,
        params: CreateProjectParams,
    ) -> Result<CreateProjectResponse, WorkspaceError> {
        let workspace_root = self.get_workspace_root().await;
        let project_path = format!("{}/{}", workspace_root, params.project_name);

        debug!(path = %project_path, "workspace/create-project");

        let mut files_created = vec![];

        match params.template.as_deref() {
            Some("python") => {
                let files = [
                    ("main.py", "#!/usr/bin/env python3\n\ndef main():\n    pass\n\nif __name__ == \"__main__\":\n    main()\n"),
                    ("requirements.txt", ""),
                    ("README.md", &format!("# {}\n\n{}", params.project_name, params.description.unwrap_or_default())),
                ];
                for (name, content) in files {
                    let file_path = format!("{}/{}", project_path, name);
                    self.vfs_client.write(&file_path, content).await?;
                    files_created.push(name.to_string());
                }
            }
            Some("web") => {
                let files = [
                    ("index.html", "<!DOCTYPE html>\n<html>\n<head>\n    <title>TODO</title>\n</head>\n<body>\n    <h1>TODO</h1>\n</body>\n</html>\n"),
                    ("style.css", "body { font-family: sans-serif; margin: 2rem; }\n"),
                    ("script.js", "// TODO\n"),
                ];
                for (name, content) in files {
                    let file_path = format!("{}/{}", project_path, name);
                    self.vfs_client.write(&file_path, content).await?;
                    files_created.push(name.to_string());
                }
            }
            _ => {}
        }

        Ok(CreateProjectResponse {
            path: project_path,
            files: files_created,
        })
    }

    pub async fn handle_snapshot(
        &self,
        params: SnapshotParams,
    ) -> Result<SnapshotResponse, WorkspaceError> {
        let workspace_root = self.get_workspace_root().await;
        let project_path = format!("{}/{}", workspace_root, params.project);

        let snapshot_name = params
            .name
            .unwrap_or_else(|| chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string());
        let snapshot_path = format!(
            "{}/.snapshots/{}/{}",
            workspace_root, params.project, snapshot_name
        );

        debug!(path = %snapshot_path, "workspace/snapshot");

        let files = self
            .vfs_client
            .list(&project_path)
            .await
            .unwrap_or_default();
        let mut file_count = 0;

        for file in files {
            let src = format!("{}/{}", project_path, file);
            if let Ok(content) = self.vfs_client.read(&src).await {
                let dst = format!("{}/{}", snapshot_path, file);
                let _ = self.vfs_client.write(&dst, &content).await;
                file_count += 1;
            }
        }

        Ok(SnapshotResponse {
            path: snapshot_path,
            files: file_count,
        })
    }

    pub async fn handle_search(
        &self,
        params: SearchParams,
    ) -> Result<SearchResponse, WorkspaceError> {
        let workspace_root = self.get_workspace_root().await;
        let project_path = params
            .project
            .map(|p| format!("{}/{}", workspace_root, p))
            .unwrap_or(workspace_root);

        debug!(query = %params.query, "workspace/search");

        let search_type = params.search_type.as_deref().unwrap_or("name");
        let mut results = vec![];

        match search_type {
            "name" => {
                let files = self
                    .vfs_client
                    .list(&project_path)
                    .await
                    .unwrap_or_default();
                for file in files {
                    if file.contains(&params.query) {
                        results.push(SearchResult {
                            path: format!("{}/{}", project_path, file),
                            line: None,
                            snippet: None,
                        });
                    }
                }
            }
            "content" => {
                let files = self
                    .vfs_client
                    .list(&project_path)
                    .await
                    .unwrap_or_default();
                for file in files {
                    let file_path = format!("{}/{}", project_path, file);
                    if let Ok(content) = self.vfs_client.read(&file_path).await {
                        for (i, line) in content.lines().enumerate() {
                            if line.contains(&params.query) {
                                results.push(SearchResult {
                                    path: file_path.clone(),
                                    line: Some(i as u32 + 1),
                                    snippet: Some(line.chars().take(100).collect()),
                                });
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        Ok(SearchResponse { results })
    }

    pub async fn handle_set_default(
        &self,
        params: SetDefaultParams,
    ) -> Result<SetDefaultResponse, WorkspaceError> {
        let workspace_root = self.get_workspace_root().await;
        let project_path = format!("{}/{}", workspace_root, params.project);

        debug!(project = %params.project, "workspace/set-default");

        let _ = self
            .vfs_client
            .write(&format!("{}/.default", workspace_root), &params.project)
            .await;

        Ok(SetDefaultResponse {
            project: project_path,
        })
    }
}
