/// Integration tests for VfsRouter (fs-gap-E).
///
/// Tests T-FE-01 through T-FE-11 from fs-gap-E-local-provider.md.
use avix_core::memfs::{LocalProvider, VfsPath, VfsRouter};
use tempfile::tempdir;

// ── T-FE-01: VfsRouter falls back to MemFs for unmounted paths ────────────────

#[tokio::test]
async fn t_fe_01_unmounted_path_uses_memfs() {
    let vfs = VfsRouter::new();
    let path = VfsPath::parse("/proc/test/file.txt").unwrap();
    vfs.write(&path, b"hello".to_vec()).await.unwrap();
    let got = vfs.read(&path).await.unwrap();
    assert_eq!(got, b"hello");
}

// ── T-FE-02: LocalProvider mounted path routes correctly ──────────────────────

#[tokio::test]
async fn t_fe_02_mounted_path_routes_to_local_provider() {
    let dir = tempdir().unwrap();
    let provider = LocalProvider::new(dir.path()).unwrap();
    let vfs = VfsRouter::new();
    vfs.mount("/users/alice".to_string(), provider).await;

    let path = VfsPath::parse("/users/alice/defaults.yaml").unwrap();
    vfs.write(&path, b"content".to_vec()).await.unwrap();

    // File should be on disk
    assert!(dir.path().join("defaults.yaml").exists());

    // Read back through VFS
    let got = vfs.read(&path).await.unwrap();
    assert_eq!(got, b"content");
}

// ── T-FE-03: Longer prefix takes precedence ───────────────────────────────────

#[tokio::test]
async fn t_fe_03_longest_prefix_wins() {
    let dir_users = tempdir().unwrap();
    let dir_alice = tempdir().unwrap();

    let vfs = VfsRouter::new();
    vfs.mount(
        "/users".to_string(),
        LocalProvider::new(dir_users.path()).unwrap(),
    )
    .await;
    vfs.mount(
        "/users/alice".to_string(),
        LocalProvider::new(dir_alice.path()).unwrap(),
    )
    .await;

    let path = VfsPath::parse("/users/alice/file.txt").unwrap();
    vfs.write(&path, b"alice".to_vec()).await.unwrap();

    // Should be in alice's dir, not the generic /users dir
    assert!(dir_alice.path().join("file.txt").exists());
    assert!(!dir_users.path().join("alice/file.txt").exists());
}

// ── T-FE-04: LocalProvider rejects path traversal ─────────────────────────────

#[tokio::test]
async fn t_fe_04_path_traversal_rejected() {
    let dir = tempdir().unwrap();
    let provider = LocalProvider::new(dir.path()).unwrap();
    // Construct a path that would traverse up with .. — VfsPath::parse already rejects these,
    // so use LocalProvider directly.
    let result = provider.read("../etc/passwd").await;
    assert!(result.is_err());
}

// ── T-FE-05: write auto-creates parent directories ────────────────────────────

#[tokio::test]
async fn t_fe_05_write_creates_parent_dirs() {
    let dir = tempdir().unwrap();
    let provider = LocalProvider::new(dir.path()).unwrap();
    provider
        .write("deep/nested/dir/file.txt", b"data".to_vec())
        .await
        .unwrap();
    assert!(dir.path().join("deep/nested/dir/file.txt").exists());
}

// ── T-FE-06: exists() returns correct results ─────────────────────────────────

#[tokio::test]
async fn t_fe_06_exists_correct() {
    let dir = tempdir().unwrap();
    let vfs = VfsRouter::new();
    vfs.mount(
        "/users/alice".to_string(),
        LocalProvider::new(dir.path()).unwrap(),
    )
    .await;

    let path = VfsPath::parse("/users/alice/x.txt").unwrap();
    assert!(!vfs.exists(&path).await);
    vfs.write(&path, b"x".to_vec()).await.unwrap();
    assert!(vfs.exists(&path).await);
}

// ── T-FE-07: delete removes file from disk ───────────────────────────────────

#[tokio::test]
async fn t_fe_07_delete_removes_file() {
    let dir = tempdir().unwrap();
    let vfs = VfsRouter::new();
    vfs.mount(
        "/users/alice".to_string(),
        LocalProvider::new(dir.path()).unwrap(),
    )
    .await;

    let path = VfsPath::parse("/users/alice/tmp.txt").unwrap();
    vfs.write(&path, b"temp".to_vec()).await.unwrap();
    assert!(vfs.exists(&path).await);

    vfs.delete(&path).await.unwrap();
    assert!(!vfs.exists(&path).await);
    assert!(!dir.path().join("tmp.txt").exists());
}

// ── T-FE-08: list() returns immediate children ───────────────────────────────

#[tokio::test]
async fn t_fe_08_list_immediate_children() {
    let dir = tempdir().unwrap();
    let vfs = VfsRouter::new();
    vfs.mount(
        "/users/alice".to_string(),
        LocalProvider::new(dir.path()).unwrap(),
    )
    .await;

    vfs.write(
        &VfsPath::parse("/users/alice/a.yaml").unwrap(),
        b"a".to_vec(),
    )
    .await
    .unwrap();
    vfs.write(
        &VfsPath::parse("/users/alice/b.yaml").unwrap(),
        b"b".to_vec(),
    )
    .await
    .unwrap();
    // Nested file should NOT appear in list of parent
    vfs.write(
        &VfsPath::parse("/users/alice/sub/c.yaml").unwrap(),
        b"c".to_vec(),
    )
    .await
    .unwrap();

    let dir_path = VfsPath::parse("/users/alice").unwrap();
    let mut entries = vfs.list(&dir_path).await.unwrap();
    entries.sort();
    assert!(entries.contains(&"a.yaml".to_string()));
    assert!(entries.contains(&"b.yaml".to_string()));
    // "sub" directory should appear, but not "sub/c.yaml"
    assert!(entries.contains(&"sub".to_string()));
    assert!(!entries.iter().any(|e| e.contains('/')));
}

// ── T-FE-09: phase1 paths stay in MemFs (not on disk) ────────────────────────

#[tokio::test]
async fn t_fe_09_phase1_paths_stay_in_memfs() {
    use avix_core::bootstrap::phase1;

    let dir = tempdir().unwrap();
    let vfs = VfsRouter::new();
    // Mount /users to disk, but NOT /kernel or /proc
    vfs.mount(
        "/users/alice".to_string(),
        LocalProvider::new(dir.path()).unwrap(),
    )
    .await;

    phase1::run(&vfs).await;

    // /kernel paths should be readable (from MemFs)
    let kpath = VfsPath::parse("/kernel/defaults/agent-manifest.yaml").unwrap();
    let kdata = vfs.read(&kpath).await.unwrap();
    assert!(!kdata.is_empty());

    // Nothing written to disk (only /kernel and /proc written by phase1)
    let disk_entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        disk_entries.is_empty(),
        "phase1 should not write to disk mounts"
    );
}

// ── T-FE-10: ResolverInputLoader works with VfsRouter ────────────────────────

#[tokio::test]
async fn t_fe_10_resolver_works_with_vfs_router() {
    use avix_core::bootstrap::phase1;
    use avix_core::params::resolver::ResolverInputLoader;

    let vfs = VfsRouter::new();
    phase1::run(&vfs).await;

    let loader = ResolverInputLoader::new(&vfs);
    let input = loader.load("alice", &[]).await.unwrap();
    assert!(input.system_defaults.entrypoint.is_some());
    assert!(input.user_defaults.is_none()); // no user defaults on disk
}

// ── T-FE-11: LocalProvider.read on missing file returns ENOENT-style error ───

#[tokio::test]
async fn t_fe_11_read_missing_file_errors() {
    let dir = tempdir().unwrap();
    let provider = LocalProvider::new(dir.path()).unwrap();
    let result = provider.read("nonexistent.yaml").await;
    assert!(result.is_err());
}
