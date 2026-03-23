use std::path::Path;

use crate::error::AvixError;
use crate::memfs::{LocalProvider, VfsRouter};

/// Phase 2: Mount persistent trees onto disk-backed `LocalProvider` instances.
///
/// The four invariant mounts:
///   `/etc/avix`  → `{root}/etc`          (read-only at runtime, written only by config-init)
///   `/users`     → `{root}/data/users`   (persistent agent workspace + memory)
///   `/crews`     → `{root}/data/crews`   (shared crew memory)
///   `/services`  → `{root}/data/services` (service state)
///
/// Ephemeral paths (`/proc/`, `/kernel/`) are NOT mounted — they stay in `MemFs`.
pub async fn mount_persistent_trees(vfs: &VfsRouter, root: &Path) -> Result<(), AvixError> {
    let mounts = [
        ("/etc/avix", root.join("etc")),
        ("/users", root.join("data/users")),
        ("/crews", root.join("data/crews")),
        ("/services", root.join("data/services")),
    ];

    for (prefix, dir) in &mounts {
        // Ensure the target directory exists before mounting
        std::fs::create_dir_all(dir).map_err(|e| {
            AvixError::Io(format!(
                "create dir {} for mount {prefix}: {e}",
                dir.display()
            ))
        })?;

        let provider = LocalProvider::new(dir)?;
        vfs.mount(prefix.to_string(), provider).await;
        tracing::debug!(prefix, dir = %dir.display(), "phase2: mounted");
    }

    tracing::info!("phase2: persistent trees mounted");
    Ok(())
}
