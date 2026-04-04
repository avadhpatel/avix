# pkg-gap-C — TUI, Web-UI & GitHub Actions Workflow

> **Status:** Completed
> **Priority:** Medium
> **Depends on:** pkg-gap-A (syscalls), pkg-gap-B (CLI ATP wiring)
> **Blocks:** pkg-gap-D (polish)
> **Affects:**
> - `crates/avix-cli/src/tui/` (new catalog install view)
> - `crates/avix-app/src/` (new Extensions tab)
> - `.github/workflows/release-packages.yml` (new file)

---

## Problem

No TUI or Web-UI surface exists for browsing and installing packages. Developers have
no automated packaging pipeline — packages must be created manually on every release.

---

## Scope

1. **TUI** — interactive install prompt accessible via `avix install <name>` or `:install <name>` in the TUI.
2. **Web-UI** (Tauri `avix-app`) — new "Extensions" tab with Browse/Installed views.
3. **GitHub Actions workflow** — automates `.tar.xz` packaging and upload on every GitHub Release.

---

## What to Build

### 1. TUI — Install prompt

The TUI already has a command mode (`:` prefix). Add two new commands:

| Command | Action |
|---|---|
| `:install agent <source>` | Runs `proc/package/install-agent` and streams progress |
| `:install service <source>` | Runs `proc/package/install-service` and streams progress |

**State additions** (in `AppState` or equivalent):

```rust
pub enum InstallState {
    Idle,
    InProgress { name: String, progress: Vec<String> },
    Done { name: String },
    Failed { name: String, error: String },
}
```

**Rendering** — when `InstallState` is `InProgress`, render a progress panel below the
command bar. Each `install.progress` ATP event appends to `progress: Vec<String>` which
is displayed as a scrolling log. On `Done` or `Failed`, show a one-line status banner that
auto-dismisses after 3 seconds (use a `tokio::time::sleep` + `AppAction::DismissInstall`).

**Key binding** — no new key bindings required; install is driven entirely via command mode.

**Reducer actions to add:**

```rust
pub enum AppAction {
    // … existing …
    InstallStart { name: String },
    InstallProgress(String),
    InstallComplete(String),
    InstallError(String),
    DismissInstall,
}
```

Apply in the reducer:

```rust
AppAction::InstallStart { name } => {
    state.install = InstallState::InProgress { name, progress: vec![] };
}
AppAction::InstallProgress(msg) => {
    if let InstallState::InProgress { ref mut progress, .. } = state.install {
        progress.push(msg);
    }
}
// …etc
```

**Command mode parsing** — in the command dispatcher, match on `install agent <src>` and
`install service <src>`. Build the ATP body and call
`client.cmd("proc/package/install-agent", body)` in a `tokio::spawn` task.
Dispatch `InstallStart` before awaiting, `InstallProgress` per event, `InstallComplete`
or `InstallError` on finish.

### 2. Web-UI — Extensions tab (Tauri `avix-app`)

Add a new "Extensions" page alongside the existing catalog/history pages.

#### 2a. Tauri backend commands (`crates/avix-app/src/commands/extensions.rs`)

```rust
#[tauri::command]
pub async fn install_agent(
    source: String,
    scope: String,
    version: Option<String>,
    checksum: Option<String>,
    no_verify: bool,
    session_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({
        "source": source, "scope": scope, "version": version.unwrap_or_else(|| "latest".into()),
        "checksum": checksum, "no_verify": no_verify, "session_id": session_id,
    });
    state.atp_client.cmd("proc/package/install-agent", body)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn install_service(
    source: String,
    scope: String,
    version: Option<String>,
    checksum: Option<String>,
    no_verify: bool,
    session_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({
        "source": source, "scope": scope, "version": version.unwrap_or_else(|| "latest".into()),
        "checksum": checksum, "no_verify": no_verify, "session_id": session_id,
    });
    state.atp_client.cmd("proc/package/install-service", body)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_installed_agents(
    state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    state.atp_client.cmd("proc/list-installed", serde_json::json!({}))
        .await
        .map_err(|e| e.to_string())
}
```

Register these in `main.rs` alongside existing Tauri commands.

#### 2b. Frontend — `ExtensionsPage` component

Create `src/pages/ExtensionsPage.tsx` with three tabs: **Browse**, **Installed**, **Install URL**.

**Browse tab** — on mount, fetch
`https://api.github.com/repos/avadhpatel/avix/releases/latest` and parse assets whose
names end in `.tar.xz`. Render a card grid. Each card shows name, version, description
(from asset notes or release body), and an "Install" button. Clicking Install calls
`invoke('install_agent', { source: assetUrl })` with a loading spinner.

**Installed tab** — calls `invoke('list_installed_agents')` and renders a table with
columns: Name, Version, Scope, Install Date, Uninstall button (uninstall not in this gap —
disable the button with tooltip "coming soon").

**Install URL tab** — a form with a text input for a custom source URL and fields for
scope, version, checksum, and no_verify toggle. Submitting calls `install_agent` or
`install_service` depending on a radio selector. Shows a live progress log area that
subscribes to ATP `install.progress` events via the existing Tauri event bridge.

**Security badge** — in Browse and Install URL tabs, show a green "SHA-256 Verified" badge
when the install completes successfully, or a red "Unverified" badge when `--no-verify` was used.

#### 2c. Navigation wiring

Add "Extensions" to the sidebar nav list (alongside Agents, Catalog, History).
Route: `/extensions`.

### 3. GitHub Actions workflow — `.github/workflows/release-packages.yml`

```yaml
name: Package & Release Avix Extensions

on:
  release:
    types: [created]

jobs:
  package-agents:
    name: Package agent ${{ matrix.agent }}
    runs-on: ubuntu-latest
    strategy:
      matrix:
        agent:
          - universal-tool-explorer
          # Add more agents here
    steps:
      - uses: actions/checkout@v4

      - name: Validate manifest
        run: |
          if [ ! -f "agents/packs/${{ matrix.agent }}/manifest.yaml" ]; then
            echo "ERROR: missing manifest.yaml for ${{ matrix.agent }}"
            exit 1
          fi

      - name: Build archive
        run: |
          cd agents/packs/${{ matrix.agent }}
          VERSION=${{ github.ref_name }}
          ARCHIVE="${{ matrix.agent }}-${VERSION}.tar.xz"
          tar -cJf "../../${ARCHIVE}" .
          cd ../..
          sha256sum "${ARCHIVE}" >> checksums.sha256
          echo "ARCHIVE=${ARCHIVE}" >> $GITHUB_ENV

      - name: Upload to release
        uses: softprops/action-gh-release@v2
        with:
          files: |
            ${{ env.ARCHIVE }}
            checksums.sha256

  package-services:
    name: Package service ${{ matrix.service.name }} (${{ matrix.service.target }})
    runs-on: ${{ matrix.service.runner }}
    strategy:
      matrix:
        service:
          - { name: workspace, target: x86_64-unknown-linux-gnu, runner: ubuntu-latest }
          - { name: workspace, target: aarch64-unknown-linux-gnu, runner: ubuntu-latest }
          - { name: workspace, target: x86_64-apple-darwin, runner: macos-latest }
          - { name: workspace, target: aarch64-apple-darwin, runner: macos-latest }
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.service.target }}

      - name: Install cross-compilation tools (Linux)
        if: matrix.service.runner == 'ubuntu-latest' && matrix.service.target == 'aarch64-unknown-linux-gnu'
        run: sudo apt-get install -y gcc-aarch64-linux-gnu

      - name: Build service binary
        run: |
          cargo build --release --target ${{ matrix.service.target }} \
            --manifest-path services/${{ matrix.service.name }}/Cargo.toml
          mkdir -p build/package/bin
          cp target/${{ matrix.service.target }}/release/${{ matrix.service.name }} \
             build/package/bin/

      - name: Assemble package
        run: |
          cp -r services/${{ matrix.service.name }}/. build/package/ 2>/dev/null || true
          rm -rf build/package/target build/package/Cargo.toml build/package/src
          cd build/package
          VERSION=${{ github.ref_name }}
          OS=$(echo "${{ matrix.service.target }}" | cut -d- -f3)
          ARCH=$(echo "${{ matrix.service.target }}" | cut -d- -f1)
          ARCHIVE="${{ matrix.service.name }}-${VERSION}-${OS}-${ARCH}.tar.xz"
          tar -cJf "../../${ARCHIVE}" .
          cd ../..
          sha256sum "${ARCHIVE}" >> checksums.sha256
          echo "ARCHIVE=${ARCHIVE}" >> $GITHUB_ENV

      - name: Upload to release
        uses: softprops/action-gh-release@v2
        with:
          files: |
            ${{ env.ARCHIVE }}
            checksums.sha256
```

Place the workflow file at `.github/workflows/release-packages.yml`.

---

## Tests

### TUI
- `install_start_sets_in_progress_state()` — reducer unit test
- `install_progress_appends_log()` — reducer unit test
- `install_complete_transitions_to_done()` — reducer unit test
- `install_error_transitions_to_failed()` — reducer unit test
- `dismiss_install_clears_state()` — reducer unit test

### Tauri backend
- `install_agent_builds_correct_body()` — mock `AtpClient`, assert JSON body fields
- `install_service_defaults_scope_system()` — scope field in body

### Workflow (manual)
- Trigger a test release on a fork and verify assets appear in the release page.

---

## Success Criteria

- [ ] `:install agent universal-tool-explorer` in TUI shows a progress log and success banner
- [ ] `:install service workspace` in TUI installs and shows "registered with router"
- [ ] Extensions tab in Web-UI renders the GitHub Releases asset list
- [ ] One-click Install in Web-UI sends the correct ATP call and shows progress modal
- [ ] Installed tab lists currently installed agents via `proc/list-installed`
- [ ] GitHub Actions workflow produces `*.tar.xz` assets on Release creation
- [ ] Checksums file is uploaded alongside every archive
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
