use crate::config::kernel::KernelSpec;
use crate::error::AvixError;
use serde::Serialize;
use std::path::PathBuf;

pub struct ReloadParams {
    pub root: PathBuf,
    /// If true, validate and classify sections but do not write the reload-pending marker.
    pub check_only: bool,
}

pub struct ReloadResult {
    /// Sections that are hot-reloadable (will be applied without restart).
    pub reloaded_sections: Vec<String>,
    /// Sections that changed in a way that requires a full kernel restart.
    pub restart_required: Vec<String>,
    /// Validation errors, if any (non-empty only if we returned Ok despite soft issues).
    pub errors: Vec<String>,
}

/// Validate the current `kernel.yaml` and classify changed sections as hot-reloadable
/// or restart-required. When `check_only` is false, write a reload-pending marker file
/// at `{root}/run/avix/reload-pending` for the kernel daemon to poll.
pub async fn run_config_reload(params: ReloadParams) -> Result<ReloadResult, AvixError> {
    // 1. Read and parse kernel.yaml
    let kernel_path = params.root.join("etc/kernel.yaml");
    let yaml = std::fs::read_to_string(&kernel_path)
        .map_err(|e| AvixError::ConfigParse(format!("cannot read kernel.yaml: {e}")))?;
    let new_config = crate::config::kernel::KernelConfig::from_str(&yaml)?;

    // 2. Validate — fail fast on any constraint violation
    new_config.validate()?;

    // 3. Compare new spec against compiled-in baseline section by section
    let baseline = KernelSpec::default();
    let new_spec = &new_config.spec;

    let mut reloaded_sections: Vec<String> = vec![];
    let mut restart_required: Vec<String> = vec![];

    // Hot-reloadable sections: always listed (valid config means they can be applied)
    for s in ["scheduler", "memory", "safety", "observability"] {
        reloaded_sections.push(s.to_string());
    }

    // IPC: any field change requires a restart (new socket path, transport, etc.)
    if section_changed(&new_spec.ipc, &baseline.ipc) {
        restart_required.push("ipc".to_string());
    }

    // Models: kernel model change requires restart; other model fields are hot-reloadable
    if new_spec.models.kernel != baseline.models.kernel {
        restart_required.push("models".to_string());
    } else if section_changed(&new_spec.models, &baseline.models) {
        reloaded_sections.push("models".to_string());
    }

    // Secrets: masterKey or store change requires restart; audit config is hot-reloadable
    if section_changed(&new_spec.secrets.master_key, &baseline.secrets.master_key)
        || section_changed(&new_spec.secrets.store, &baseline.secrets.store)
    {
        restart_required.push("secrets".to_string());
    } else if section_changed(&new_spec.secrets, &baseline.secrets) {
        reloaded_sections.push("secrets".to_string());
    }

    if params.check_only {
        return Ok(ReloadResult {
            reloaded_sections,
            restart_required,
            errors: vec![],
        });
    }

    // 4. Write reload-pending marker file for the kernel daemon to poll
    let run_dir = params.root.join("run/avix");
    std::fs::create_dir_all(&run_dir)
        .map_err(|e| AvixError::ConfigParse(format!("cannot create run dir: {e}")))?;
    std::fs::write(run_dir.join("reload-pending"), b"")
        .map_err(|e| AvixError::ConfigParse(format!("cannot write reload-pending: {e}")))?;

    Ok(ReloadResult {
        reloaded_sections,
        restart_required,
        errors: vec![],
    })
}

/// Returns true if `a` and `b` produce different YAML (i.e., any field differs).
fn section_changed<T: Serialize>(a: &T, b: &T) -> bool {
    let a_val = serde_yaml::to_value(a).unwrap_or(serde_yaml::Value::Null);
    let b_val = serde_yaml::to_value(b).unwrap_or(serde_yaml::Value::Null);
    a_val != b_val
}
