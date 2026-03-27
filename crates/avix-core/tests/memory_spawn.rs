use avix_core::config::MemoryConfig;
use avix_core::executor::runtime_executor::MockToolRegistry;
use avix_core::executor::{RuntimeExecutor, SpawnParams};
use avix_core::memfs::{VfsPath, VfsRouter};
use avix_core::memory_svc::{
    schema::{UserPreferenceModel, UserPreferenceModelMetadata, UserPreferenceModelSpec},
    service::MemoryService,
    store,
};
use avix_core::types::{capability_map::CapabilityToolMap, token::CapabilityToken, Pid};
use chrono::Utc;
use std::sync::atomic::Ordering;
use std::sync::Arc;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn token_with_caps(caps: &[&str]) -> CapabilityToken {
    CapabilityToken::test_token(caps)
}

async fn make_executor_with_vfs(
    owner: &str,
    agent: &str,
    pid: u32,
    caps: &[&str],
) -> (RuntimeExecutor, Arc<VfsRouter>) {
    let registry = Arc::new(MockToolRegistry::new());
    let vfs = Arc::new(VfsRouter::new());
    let params = SpawnParams {
        pid: Pid::new(pid),
        agent_name: agent.to_string(),
        goal: "test goal".to_string(),
        spawned_by: owner.to_string(),
        session_id: "sess-test".to_string(),
        token: token_with_caps(caps),
        system_prompt: None,
        selected_model: "claude-sonnet-4".into(),
        denied_tools: vec![],
        context_limit: 0,
            runtime_dir: std::path::PathBuf::new(),
    };
    let executor = RuntimeExecutor::spawn_with_registry(params, registry)
        .await
        .unwrap()
        .with_vfs(Arc::clone(&vfs));
    (executor, vfs)
}

async fn make_executor_with_memory_svc(
    owner: &str,
    agent: &str,
    pid: u32,
    caps: &[&str],
) -> (RuntimeExecutor, Arc<VfsRouter>) {
    let (executor, vfs) = make_executor_with_vfs(owner, agent, pid, caps).await;
    let svc = Arc::new(MemoryService::new(
        Arc::clone(&vfs),
        Arc::new(MemoryConfig::default()),
    ));
    let executor = executor.with_memory_svc(svc);
    (executor, vfs)
}

fn make_pref_record(owner: &str, agent: &str, summary: &str) -> UserPreferenceModel {
    UserPreferenceModel::new(
        UserPreferenceModelMetadata {
            agent_name: agent.to_string(),
            owner: owner.to_string(),
            updated_at: Utc::now(),
        },
        UserPreferenceModelSpec {
            summary: summary.to_string(),
            structured: Default::default(),
            corrections: vec![],
        },
    )
}

// ── T-MD-01: memory:read maps to retrieve/get-fact/get-preferences ────────────

#[test]
fn memory_read_capability_maps_correctly() {
    let map = CapabilityToolMap::default();
    let tools = map.tools_for_capability("memory:read");
    assert!(tools.contains(&"memory/retrieve"));
    assert!(tools.contains(&"memory/get-fact"));
    assert!(tools.contains(&"memory/get-preferences"));
    assert!(
        !tools.contains(&"memory/log-event"),
        "log-event requires write"
    );
    assert!(
        !tools.contains(&"memory/store-fact"),
        "store-fact requires write"
    );
}

// ── T-MD-02: memory:write is a superset of memory:read ───────────────────────

#[test]
fn memory_write_includes_read_tools() {
    let map = CapabilityToolMap::default();
    let tools = map.tools_for_capability("memory:write");
    // Read tools included
    assert!(tools.contains(&"memory/retrieve"));
    assert!(tools.contains(&"memory/get-fact"));
    assert!(tools.contains(&"memory/get-preferences"));
    // Write tools included
    assert!(tools.contains(&"memory/log-event"));
    assert!(tools.contains(&"memory/store-fact"));
    assert!(tools.contains(&"memory/update-preference"));
    assert!(tools.contains(&"memory/forget"));
}

// ── T-MD-03: memory:write capability grants all memory tools ─────────────────

#[tokio::test]
async fn memory_write_tools_registered_at_spawn() {
    let map = CapabilityToolMap::default();
    let write_tools: Vec<&str> = map.tools_for_capability("memory:write").to_vec();
    // Build a token with all memory:write tools
    let (_, registry) = {
        let reg = Arc::new(MockToolRegistry::new());
        let params = SpawnParams {
            pid: Pid::new(200),
            agent_name: "researcher".to_string(),
            goal: "test".to_string(),
            spawned_by: "alice".to_string(),
            session_id: "s1".to_string(),
            token: token_with_caps(&write_tools),
            system_prompt: None,
            selected_model: "claude-sonnet-4".into(),
            denied_tools: vec![],
            context_limit: 0,
            runtime_dir: std::path::PathBuf::new(),
        };
        let executor = RuntimeExecutor::spawn_with_registry(params, Arc::clone(&reg))
            .await
            .unwrap();
        (executor, reg)
    };
    let registered = registry.tools_registered_by_pid(200).await;
    assert!(registered.contains("memory/retrieve"));
    assert!(registered.contains("memory/log-event"));
    assert!(registered.contains("memory/get-fact"));
    assert!(registered.contains("memory/store-fact"));
    assert!(registered.contains("memory/forget"));
}

// ── T-MD-04: resolved.yaml includes memory block ─────────────────────────────

#[tokio::test]
async fn resolved_yaml_includes_memory_block() {
    let (executor, vfs) = make_executor_with_vfs("alice", "researcher", 300, &[]).await;
    executor.init_proc_files().await;
    let path = VfsPath::parse(&format!("/proc/{}/resolved.yaml", 300)).unwrap();
    let bytes = vfs.read(&path).await.unwrap();
    let yaml = String::from_utf8(bytes).unwrap();
    assert!(
        yaml.contains("episodicEnabled"),
        "expected episodicEnabled in resolved.yaml, got:\n{yaml}"
    );
    assert!(yaml.contains("semanticEnabled"));
    assert!(yaml.contains("autoInjectAtSpawn"));
}

// ── T-MD-05: memory tree is created at spawn when memory is enabled ───────────

#[tokio::test]
async fn spawn_creates_memory_tree() {
    let write_tools: Vec<&str> = CapabilityToolMap::default()
        .tools_for_capability("memory:write")
        .to_vec();
    let (executor, vfs) = make_executor_with_vfs("alice", "researcher", 301, &write_tools).await;
    executor.init_memory_tree().await;
    assert!(
        vfs.exists(&VfsPath::parse("/users/alice/memory/researcher/episodic/.keep").unwrap())
            .await,
        "expected episodic dir at spawn"
    );
    assert!(
        vfs.exists(&VfsPath::parse("/users/alice/memory/researcher/semantic/.keep").unwrap())
            .await
    );
}

// ── T-MD-05b: no memory tree when no memory tools ────────────────────────────

#[tokio::test]
async fn no_memory_tree_without_memory_tools() {
    let (executor, vfs) = make_executor_with_vfs("alice", "researcher", 302, &[]).await;
    executor.init_memory_tree().await;
    // No memory tools → no init
    assert!(
        !vfs.exists(&VfsPath::parse("/users/alice/memory/researcher/episodic/.keep").unwrap())
            .await,
        "expected NO episodic dir when no memory tools"
    );
}

// ── T-MD-06: system prompt includes memory context block when prefs exist ──────

#[tokio::test]
async fn spawn_injects_memory_context_when_prefs_exist() {
    let write_tools: Vec<&str> = CapabilityToolMap::default()
        .tools_for_capability("memory:write")
        .to_vec();
    let (mut executor, vfs) =
        make_executor_with_vfs("alice", "researcher", 303, &write_tools).await;

    // Pre-populate preferences
    let pref = make_pref_record("alice", "researcher", "Prefers concise answers.");
    let pref_path = UserPreferenceModel::vfs_path("alice", "researcher");
    store::write_preference_model(&vfs, &pref_path, &pref)
        .await
        .unwrap();

    executor.init_memory_context().await;
    let prompt = executor.system_prompt();
    assert!(
        prompt.contains("[MEMORY CONTEXT"),
        "expected memory context block in system prompt"
    );
    assert!(prompt.contains("Prefers concise answers."));
}

// ── T-MD-07: SIGSTOP auto-logs session to episodic memory ─────────────────────

#[tokio::test]
async fn sigstop_auto_logs_session() {
    let write_tools: Vec<&str> = CapabilityToolMap::default()
        .tools_for_capability("memory:write")
        .to_vec();
    let (mut executor, vfs) =
        make_executor_with_memory_svc("alice", "researcher", 304, &write_tools).await;

    // Init memory tree so the VFS dirs exist
    executor.init_memory_tree().await;

    // Add conversation turns
    executor.push_conversation_message("user", "What is quantum computing?");
    executor.push_conversation_message("assistant", "Quantum computing uses qubits...");

    // Deliver SIGSTOP — should auto-log and then set killed=true
    executor.deliver_signal("SIGSTOP").await;

    assert!(
        executor.killed.load(Ordering::Acquire),
        "expected killed=true after SIGSTOP"
    );

    // An episodic record should now exist
    let episodic_dir = VfsPath::parse("/users/alice/memory/researcher/episodic").unwrap();
    let entries = vfs.list(&episodic_dir).await.unwrap();
    let yaml_entries: Vec<_> = entries.iter().filter(|e| e.ends_with(".yaml")).collect();
    assert!(
        !yaml_entries.is_empty(),
        "expected episodic record after SIGSTOP"
    );
}

// ── T-MD-08: SIGSTOP without memory svc does not panic ───────────────────────

#[tokio::test]
async fn sigstop_without_memory_svc_does_not_panic() {
    let (mut executor, _vfs) = make_executor_with_vfs("alice", "researcher", 305, &[]).await;
    executor.push_conversation_message("user", "Hello");
    executor.deliver_signal("SIGSTOP").await; // must not panic
    assert!(executor.killed.load(Ordering::Acquire));
}
