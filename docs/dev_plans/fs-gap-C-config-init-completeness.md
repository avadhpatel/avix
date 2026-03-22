# Filesystem Gap C — `avix config init` Must Write All `/etc/avix/` Config Files

> **Finding:** `run_config_init` (Day 11) only writes `auth.conf`. The spec requires five
> additional files under `/etc/avix/`: `kernel.yaml`, `users.yaml`, `crews.yaml`,
> `crontab.yaml`, and `fstab.yaml`. All have Rust type definitions that parse correctly,
> but none are ever written to disk by config init.
>
> **Scope:** `src/cli/config_init.rs` — extend `run_config_init` to write all six `/etc/avix/`
> config files. Each file is written with a valid YAML skeleton that bootstrap can parse.
> If the file already exists and `--force` is not set, skip writing it (idempotent).

---

## Files to create

| Path | Kind | Notes |
|---|---|---|
| `<root>/etc/auth.conf` | `AuthConfig` | Already written — no change needed |
| `<root>/etc/kernel.yaml` | `KernelConfig` | Master key source, log level, boot settings |
| `<root>/etc/users.yaml` | `UsersConfig` | The initializing user as the first identity |
| `<root>/etc/crews.yaml` | `CrewsConfig` | Empty crew list (no crews at init time) |
| `<root>/etc/crontab.yaml` | `Crontab` | Empty scheduled jobs list |
| `<root>/etc/fstab.yaml` | `Fstab` | Single local mount for `<root>/data/` tree |

---

## YAML skeletons

### `kernel.yaml`

```yaml
apiVersion: avix/v1
kind: KernelConfig
metadata:
  createdAt: "<timestamp>"
spec:
  log:
    level: info
    format: json
  secrets:
    algorithm: aes-256-gcm
    masterKey:
      source: env
      envVar: AVIX_MASTER_KEY
    store:
      path: /secrets
      provider: local
    audit:
      enabled: true
      logReads: true
      logWrites: true
```

### `users.yaml`

Uses the `--user` and `--role` params passed to `config init`.

```yaml
apiVersion: avix/v1
kind: Users
spec:
  users:
    - name: "<identity_name>"
      uid: 1001
      role: "<role>"
      credential:
        type: "<credential_type>"
        keyHash: "<hmac_hash_of_api_key>"
```

### `crews.yaml`

```yaml
apiVersion: avix/v1
kind: Crews
spec:
  crews: []
```

### `crontab.yaml`

```yaml
apiVersion: avix/v1
kind: Crontab
spec:
  jobs: []
```

### `fstab.yaml`

Uses the root path passed to `config init`.

```yaml
apiVersion: avix/v1
kind: Fstab
spec:
  mounts:
    - path: /etc/avix
      provider: local
      config:
        root: "<root>/etc"
      options:
        readonly: false

    - path: /users/<identity_name>
      provider: local
      config:
        root: "<root>/data/users/<identity_name>"
      options: {}

    - path: /secrets
      provider: local
      config:
        root: "<root>/secrets"
      options:
        encrypted: true
```

---

## Step 1 — Write Tests First

Add to `crates/avix-core/tests/atp_token.rs` (or wherever `config_init` tests live):

```rust
// ── Finding C: config init writes all /etc/avix/ files ───────────────────────

#[test]
fn config_init_creates_kernel_yaml() {
    use avix_core::cli::config_init::{ConfigInitParams, run_config_init};
    use tempfile::tempdir;

    let tmp = tempdir().unwrap();
    run_config_init(ConfigInitParams {
        root: tmp.path().to_path_buf(),
        identity_name: "alice".into(),
        credential_type: "api_key".into(),
        role: "admin".into(),
        master_key_source: "env".into(),
        mode: "cli".into(),
    }).unwrap();

    let path = tmp.path().join("etc/kernel.yaml");
    assert!(path.exists(), "kernel.yaml must exist after config init");
    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.contains("KernelConfig"), "kernel.yaml must have kind: KernelConfig");
    assert!(content.contains("AVIX_MASTER_KEY"), "kernel.yaml must reference AVIX_MASTER_KEY");
}

#[test]
fn config_init_creates_users_yaml_with_identity() {
    use avix_core::cli::config_init::{ConfigInitParams, run_config_init};
    use tempfile::tempdir;

    let tmp = tempdir().unwrap();
    run_config_init(ConfigInitParams {
        root: tmp.path().to_path_buf(),
        identity_name: "bob".into(),
        credential_type: "api_key".into(),
        role: "user".into(),
        master_key_source: "env".into(),
        mode: "cli".into(),
    }).unwrap();

    let content = std::fs::read_to_string(tmp.path().join("etc/users.yaml")).unwrap();
    assert!(content.contains("bob"), "users.yaml must contain the identity name");
    assert!(content.contains("user"), "users.yaml must contain the role");
    assert!(content.contains("UsersConfig") || content.contains("Users"),
        "users.yaml must have kind: Users or UsersConfig");
}

#[test]
fn config_init_creates_crews_yaml() {
    use avix_core::cli::config_init::{ConfigInitParams, run_config_init};
    use tempfile::tempdir;

    let tmp = tempdir().unwrap();
    run_config_init(ConfigInitParams {
        root: tmp.path().to_path_buf(),
        identity_name: "alice".into(),
        credential_type: "api_key".into(),
        role: "admin".into(),
        master_key_source: "env".into(),
        mode: "cli".into(),
    }).unwrap();

    let path = tmp.path().join("etc/crews.yaml");
    assert!(path.exists(), "crews.yaml must exist after config init");
    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.contains("Crews"), "crews.yaml must have kind: Crews");
}

#[test]
fn config_init_creates_crontab_yaml() {
    use avix_core::cli::config_init::{ConfigInitParams, run_config_init};
    use tempfile::tempdir;

    let tmp = tempdir().unwrap();
    run_config_init(ConfigInitParams {
        root: tmp.path().to_path_buf(),
        identity_name: "alice".into(),
        credential_type: "api_key".into(),
        role: "admin".into(),
        master_key_source: "env".into(),
        mode: "cli".into(),
    }).unwrap();

    let path = tmp.path().join("etc/crontab.yaml");
    assert!(path.exists(), "crontab.yaml must exist after config init");
    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.contains("Crontab"), "crontab.yaml must have kind: Crontab");
}

#[test]
fn config_init_creates_fstab_yaml_with_local_mounts() {
    use avix_core::cli::config_init::{ConfigInitParams, run_config_init};
    use tempfile::tempdir;

    let tmp = tempdir().unwrap();
    run_config_init(ConfigInitParams {
        root: tmp.path().to_path_buf(),
        identity_name: "alice".into(),
        credential_type: "api_key".into(),
        role: "admin".into(),
        master_key_source: "env".into(),
        mode: "cli".into(),
    }).unwrap();

    let path = tmp.path().join("etc/fstab.yaml");
    assert!(path.exists(), "fstab.yaml must exist after config init");
    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.contains("Fstab"), "fstab.yaml must have kind: Fstab");
    assert!(content.contains("local"), "fstab.yaml must define at least one local mount");
    assert!(content.contains("/etc/avix") || content.contains("etc"),
        "fstab.yaml must mount the etc/avix tree");
    assert!(content.contains("/secrets"), "fstab.yaml must mount /secrets");
}

#[test]
fn config_init_all_files_idempotent_without_force() {
    use avix_core::cli::config_init::{ConfigInitParams, run_config_init};
    use tempfile::tempdir;

    let tmp = tempdir().unwrap();
    let params = || ConfigInitParams {
        root: tmp.path().to_path_buf(),
        identity_name: "alice".into(),
        credential_type: "api_key".into(),
        role: "admin".into(),
        master_key_source: "env".into(),
        mode: "cli".into(),
    };

    run_config_init(params()).unwrap();
    // Capture mtime of kernel.yaml
    let mtime1 = std::fs::metadata(tmp.path().join("etc/kernel.yaml"))
        .unwrap().modified().unwrap();

    // Second call — no-op
    run_config_init(params()).unwrap();
    let mtime2 = std::fs::metadata(tmp.path().join("etc/kernel.yaml"))
        .unwrap().modified().unwrap();

    assert_eq!(mtime1, mtime2, "kernel.yaml must not be rewritten on second config init without --force");
}

#[test]
fn config_init_creates_data_dirs_for_mounts() {
    use avix_core::cli::config_init::{ConfigInitParams, run_config_init};
    use tempfile::tempdir;

    let tmp = tempdir().unwrap();
    run_config_init(ConfigInitParams {
        root: tmp.path().to_path_buf(),
        identity_name: "alice".into(),
        credential_type: "api_key".into(),
        role: "admin".into(),
        master_key_source: "env".into(),
        mode: "cli".into(),
    }).unwrap();

    // The user workspace dir referenced in fstab must be created
    assert!(
        tmp.path().join("data/users/alice").exists(),
        "data/users/<identity> directory must be created at config init"
    );
    assert!(
        tmp.path().join("secrets").exists(),
        "secrets directory must be created at config init"
    );
}
```

---

## Step 2 — Implementation

### 2a. Add a `write_if_absent` helper to `config_init.rs`

```rust
/// Write `content` to `path` only if the file does not already exist.
/// Returns `true` if the file was written, `false` if it was skipped.
fn write_if_absent(path: &std::path::Path, content: &str) -> Result<bool, AvixError> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(true)
}
```

### 2b. Extend `run_config_init`

After writing `auth.conf`, add:

```rust
let root = &params.root;
let now = chrono::Utc::now().to_rfc3339();
let identity = &params.identity_name;
let role = &params.role;
let cred_type = &params.credential_type;
let key_hash = &result.api_key_hash;   // already computed for auth.conf
let root_str = root.display();

// kernel.yaml
write_if_absent(
    &root.join("etc/kernel.yaml"),
    &format!(KERNEL_YAML_TEMPLATE, now = now),
)?;

// users.yaml
write_if_absent(
    &root.join("etc/users.yaml"),
    &format!(USERS_YAML_TEMPLATE,
        identity = identity, role = role,
        cred_type = cred_type, key_hash = key_hash),
)?;

// crews.yaml
write_if_absent(&root.join("etc/crews.yaml"), CREWS_YAML_TEMPLATE)?;

// crontab.yaml
write_if_absent(&root.join("etc/crontab.yaml"), CRONTAB_YAML_TEMPLATE)?;

// fstab.yaml
write_if_absent(
    &root.join("etc/fstab.yaml"),
    &format!(FSTAB_YAML_TEMPLATE,
        root = root_str, identity = identity),
)?;

// Data directories referenced by fstab mounts
std::fs::create_dir_all(root.join(format!("data/users/{identity}")))?;
std::fs::create_dir_all(root.join("secrets"))?;
```

Define template constants at the bottom of `config_init.rs` (use `const` string literals
with `{field}` placeholders, formatted with `format!`).

---

## Step 3 — Verify

```bash
cargo test --workspace
# All 7 new config init tests must pass
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

---

## Success Criteria

- [ ] `etc/kernel.yaml` written with `kind: KernelConfig` and `AVIX_MASTER_KEY` env var reference
- [ ] `etc/users.yaml` written with identity name and role from `--user` / `--role` params
- [ ] `etc/crews.yaml` written with empty crews list
- [ ] `etc/crontab.yaml` written with empty jobs list
- [ ] `etc/fstab.yaml` written with local mounts for `/etc/avix`, `/users/<identity>`, `/secrets`
- [ ] `data/users/<identity>` and `secrets/` directories created
- [ ] Second `config init` without `--force` leaves all files unchanged (idempotent)
- [ ] 7 new tests pass, 0 clippy warnings
