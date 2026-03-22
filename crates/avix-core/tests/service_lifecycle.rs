use avix_core::service::{IpcRegisterRequest, ServiceManager, ServiceSpawnRequest};

#[tokio::test]
async fn spawn_service_returns_token() {
    let mgr = ServiceManager::new_for_test();
    let token = mgr
        .spawn_and_get_token(ServiceSpawnRequest {
            name: "llm.svc".into(),
            binary: "/usr/bin/avix-llm".into(),
        })
        .await
        .unwrap();
    assert_eq!(token.service_name, "llm.svc");
    assert!(token.token_str.starts_with("svc-token-"));
}

#[tokio::test]
async fn service_token_has_unique_pid() {
    let mgr = ServiceManager::new_for_test();
    let t1 = mgr
        .spawn_and_get_token(ServiceSpawnRequest {
            name: "svc-a".into(),
            binary: "/bin/a".into(),
        })
        .await
        .unwrap();
    let t2 = mgr
        .spawn_and_get_token(ServiceSpawnRequest {
            name: "svc-b".into(),
            binary: "/bin/b".into(),
        })
        .await
        .unwrap();
    assert_ne!(t1.pid.as_u32(), t2.pid.as_u32());
}

#[tokio::test]
async fn ipc_register_with_valid_token() {
    let mgr = ServiceManager::new_for_test();
    let token = mgr
        .spawn_and_get_token(ServiceSpawnRequest {
            name: "router.svc".into(),
            binary: "/bin/router".into(),
        })
        .await
        .unwrap();

    let result = mgr
        .handle_ipc_register(IpcRegisterRequest {
            token: token.token_str.clone(),
            name: "router.svc".into(),
            endpoint: "/run/avix/router.sock".into(),
            tools: vec![],
        })
        .await
        .unwrap();
    assert!(result.registered);
    assert_eq!(result.pid.as_u32(), token.pid.as_u32());
}

#[tokio::test]
async fn ipc_register_with_invalid_token_fails() {
    let mgr = ServiceManager::new_for_test();
    let result = mgr
        .handle_ipc_register(IpcRegisterRequest {
            token: "bad-token".into(),
            name: "llm.svc".into(),
            endpoint: "/run/avix/llm.sock".into(),
            tools: vec![],
        })
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("invalid service token"));
}

#[tokio::test]
async fn ipc_register_name_mismatch_fails() {
    let mgr = ServiceManager::new_for_test();
    let token = mgr
        .spawn_and_get_token(ServiceSpawnRequest {
            name: "auth.svc".into(),
            binary: "/bin/auth".into(),
        })
        .await
        .unwrap();

    let result = mgr
        .handle_ipc_register(IpcRegisterRequest {
            token: token.token_str,
            name: "wrong-name".into(),
            endpoint: "/run/avix/wrong.sock".into(),
            tools: vec![],
        })
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn service_env_contains_socket_vars() {
    let mgr = ServiceManager::new_for_test();
    mgr.spawn_and_get_token(ServiceSpawnRequest {
        name: "my.svc".into(),
        binary: "/bin/my".into(),
    })
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
    let mgr = ServiceManager::new_for_test();
    let token = mgr
        .spawn_and_get_token(ServiceSpawnRequest {
            name: "my.svc".into(),
            binary: "/bin/my".into(),
        })
        .await
        .unwrap();

    let env = mgr.service_env("my.svc").await.unwrap();
    let svc_sock = &env["AVIX_SVC_SOCK"];
    assert!(svc_sock.contains("my.svc"));
    assert!(svc_sock.contains(&token.pid.as_u32().to_string()));
}

#[tokio::test]
async fn service_tool_add_registers_tools_in_registry() {
    let (mgr, registry) = ServiceManager::new_with_registry();
    let token = mgr
        .spawn_and_get_token(ServiceSpawnRequest {
            name: "fs.svc".into(),
            binary: "/bin/fs".into(),
        })
        .await
        .unwrap();

    mgr.handle_tool_add(
        token.token_str.clone(),
        vec!["fs/read".into(), "fs/write".into(), "fs/delete".into()],
    )
    .await
    .unwrap();

    assert_eq!(registry.tool_count().await, 3);
    assert!(registry.lookup("fs/read").await.is_ok());
}

#[tokio::test]
async fn service_tool_remove_removes_from_registry() {
    let (mgr, registry) = ServiceManager::new_with_registry();
    let token = mgr
        .spawn_and_get_token(ServiceSpawnRequest {
            name: "fs.svc".into(),
            binary: "/bin/fs".into(),
        })
        .await
        .unwrap();

    mgr.handle_tool_add(
        token.token_str.clone(),
        vec!["fs/read".into(), "fs/write".into()],
    )
    .await
    .unwrap();

    mgr.handle_tool_remove(
        token.token_str.clone(),
        vec!["fs/write".into()],
        "service degraded",
        false,
    )
    .await
    .unwrap();

    assert_eq!(registry.tool_count().await, 1);
    assert!(registry.lookup("fs/read").await.is_ok());
    assert!(registry.lookup("fs/write").await.is_err());
}

#[tokio::test]
async fn service_tool_add_invalid_token_fails() {
    let mgr = ServiceManager::new_for_test();
    let result = mgr
        .handle_tool_add("bad-token".into(), vec!["fs/read".into()])
        .await;
    assert!(result.is_err());
}
