use avix_core::service::{
    IpcRegisterRequest, IpcToolAddParams, IpcToolRemoveParams, IpcToolSpec, ServiceManager,
    ServiceSpawnRequest,
};
use avix_core::tool_registry::descriptor::ToolVisibilitySpec;

#[tokio::test]
async fn spawn_service_returns_token() {
    let mgr = ServiceManager::new_for_test(std::path::PathBuf::from("/run/avix"));
    let token = mgr
        .spawn_and_get_token(ServiceSpawnRequest::simple("llm.svc", "/usr/bin/avix-llm"))
        .await
        .unwrap();
    assert_eq!(token.service_name, "llm.svc");
    assert!(token.token_str.starts_with("svc-token-"));
}

#[tokio::test]
async fn service_token_has_unique_pid() {
    let mgr = ServiceManager::new_for_test(std::path::PathBuf::from("/run/avix"));
    let t1 = mgr
        .spawn_and_get_token(ServiceSpawnRequest::simple("svc-a", "/bin/a"))
        .await
        .unwrap();
    let t2 = mgr
        .spawn_and_get_token(ServiceSpawnRequest::simple("svc-b", "/bin/b"))
        .await
        .unwrap();
    assert_ne!(t1.pid.as_u64(), t2.pid.as_u64());
}

#[tokio::test]
async fn ipc_register_with_valid_token() {
    let tmp = tempfile::tempdir().unwrap();
    let mgr = ServiceManager::new_for_test(tmp.path().to_path_buf());
    let token = mgr
        .spawn_and_get_token(ServiceSpawnRequest::simple("router.svc", "/bin/router"))
        .await
        .unwrap();

    let result = mgr
        .handle_ipc_register(
            IpcRegisterRequest {
                token: token.token_str.clone(),
                name: "router.svc".into(),
                endpoint: "/run/avix/router.sock".into(),
                tools: vec![],
            },
            tmp.path(),
        )
        .await
        .unwrap();
    assert!(result.registered);
    assert_eq!(result.pid.as_u64(), token.pid.as_u64());
}

#[tokio::test]
async fn ipc_register_with_invalid_token_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let mgr = ServiceManager::new_for_test(tmp.path().to_path_buf());
    let result = mgr
        .handle_ipc_register(
            IpcRegisterRequest {
                token: "bad-token".into(),
                name: "llm.svc".into(),
                endpoint: "/run/avix/llm.sock".into(),
                tools: vec![],
            },
            tmp.path(),
        )
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("invalid service token"));
}

#[tokio::test]
async fn ipc_register_name_mismatch_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let mgr = ServiceManager::new_for_test(tmp.path().to_path_buf());
    let token = mgr
        .spawn_and_get_token(ServiceSpawnRequest::simple("auth.svc", "/bin/auth"))
        .await
        .unwrap();

    let result = mgr
        .handle_ipc_register(
            IpcRegisterRequest {
                token: token.token_str,
                name: "wrong-name".into(),
                endpoint: "/run/avix/wrong.sock".into(),
                tools: vec![],
            },
            tmp.path(),
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn service_env_contains_socket_vars() {
    let mgr = ServiceManager::new_for_test(std::path::PathBuf::from("/run/avix"));
    mgr.spawn_and_get_token(ServiceSpawnRequest::simple("my.svc", "/bin/my"))
        .await
        .unwrap();

    let env = mgr.service_env("my.svc").await.unwrap();
    assert!(env.contains_key("AVIX_KERNEL_SOCK"));
    assert!(env.contains_key("AVIX_ROUTER_SOCK"));
    assert!(env.contains_key("AVIX_SVC_SOCK"));
    assert!(env.contains_key("AVIX_SVC_TOKEN"));
}

#[tokio::test]
async fn service_env_sock_path_contains_name_and_pid() {
    let mgr = ServiceManager::new_for_test(std::path::PathBuf::from("/run/avix"));
    let token = mgr
        .spawn_and_get_token(ServiceSpawnRequest::simple("my.svc", "/bin/my"))
        .await
        .unwrap();

    let env = mgr.service_env("my.svc").await.unwrap();
    let svc_sock = &env["AVIX_SVC_SOCK"];
    assert!(svc_sock.contains("my.svc"));
    assert!(svc_sock.contains(&token.pid.as_u64().to_string()));
}

#[tokio::test]
async fn service_tool_add_registers_tools_in_registry() {
    let (mgr, registry) = ServiceManager::new_with_registry(std::path::PathBuf::from("/tmp"));
    let token = mgr
        .spawn_and_get_token(ServiceSpawnRequest::simple("fs.svc", "/bin/fs"))
        .await
        .unwrap();

    mgr.handle_tool_add(IpcToolAddParams {
        token: token.token_str.clone(),
        tools: vec![
            IpcToolSpec {
                name: "fs/read".into(),
                descriptor: serde_json::json!({}),
                visibility: ToolVisibilitySpec::All,
            },
            IpcToolSpec {
                name: "fs/write".into(),
                descriptor: serde_json::json!({}),
                visibility: ToolVisibilitySpec::All,
            },
            IpcToolSpec {
                name: "fs/delete".into(),
                descriptor: serde_json::json!({}),
                visibility: ToolVisibilitySpec::All,
            },
        ],
    })
    .await
    .unwrap();

    assert_eq!(registry.tool_count().await, 3);
    assert!(registry.lookup("fs/read").await.is_ok());
}

#[tokio::test]
async fn service_tool_remove_removes_from_registry() {
    let (mgr, registry) = ServiceManager::new_with_registry(std::path::PathBuf::from("/tmp"));
    let token = mgr
        .spawn_and_get_token(ServiceSpawnRequest::simple("fs.svc", "/bin/fs"))
        .await
        .unwrap();

    mgr.handle_tool_add(IpcToolAddParams {
        token: token.token_str.clone(),
        tools: vec![
            IpcToolSpec {
                name: "fs/read".into(),
                descriptor: serde_json::json!({}),
                visibility: ToolVisibilitySpec::All,
            },
            IpcToolSpec {
                name: "fs/write".into(),
                descriptor: serde_json::json!({}),
                visibility: ToolVisibilitySpec::All,
            },
        ],
    })
    .await
    .unwrap();

    mgr.handle_tool_remove(IpcToolRemoveParams {
        token: token.token_str.clone(),
        tools: vec!["fs/write".into()],
        reason: "service degraded".into(),
        drain: false,
    })
    .await
    .unwrap();

    assert_eq!(registry.tool_count().await, 1);
    assert!(registry.lookup("fs/read").await.is_ok());
    assert!(registry.lookup("fs/write").await.is_err());
}

#[tokio::test]
async fn service_tool_add_invalid_token_fails() {
    let mgr = ServiceManager::new_for_test(std::path::PathBuf::from("/run/avix"));
    let result = mgr
        .handle_tool_add(IpcToolAddParams {
            token: "bad-token".into(),
            tools: vec![IpcToolSpec {
                name: "fs/read".into(),
                descriptor: serde_json::json!({}),
                visibility: ToolVisibilitySpec::All,
            }],
        })
        .await;
    assert!(result.is_err());
}
