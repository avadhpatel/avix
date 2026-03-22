use crate::error::AvixError;
use std::path::Path;

/// Closure type for writing data to a VFS path.
pub type VfsWriteFn<'a> = &'a dyn Fn(&str, &[u8]) -> Result<(), AvixError>;

/// Write binary data to an OS filesystem path.
/// Returns the full file path as a String.
pub fn write_binary_output(
    scratch_dir: &Path,
    _kind: &str,
    data: &[u8],
    ext: &str,
) -> Result<String, AvixError> {
    let filename = format!("{}.{}", uuid::Uuid::new_v4(), ext);
    let full_path = scratch_dir.join(&filename);
    std::fs::write(&full_path, data).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    Ok(full_path.to_string_lossy().into_owned())
}

/// Write binary data to the VFS path `/proc/{agent_pid}/scratch/{kind}-{uuid}.{ext}`.
/// Uses the provided `write_fn` closure to perform the actual write (injected dependency).
/// Returns the VFS path string.
pub fn write_binary_to_vfs(
    agent_pid: u32,
    kind: &str,
    data: &[u8],
    ext: &str,
    write_fn: VfsWriteFn<'_>,
) -> Result<String, AvixError> {
    let uid = uuid::Uuid::new_v4();
    let vfs_path = format!("/proc/{}/scratch/{}-{}.{}", agent_pid, kind, uid, ext);
    write_fn(&vfs_path, data)?;
    Ok(vfs_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_write_binary_to_vfs_path_format() {
        let written: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));
        let written_clone = Arc::clone(&written);

        let write_fn = |path: &str, data: &[u8]| -> Result<(), AvixError> {
            written_clone
                .lock()
                .unwrap()
                .insert(path.to_string(), data.to_vec());
            Ok(())
        };

        let data = b"fake image bytes";
        let result = write_binary_to_vfs(42, "img", data, "png", &write_fn).unwrap();

        assert!(
            result.starts_with("/proc/42/scratch/img-"),
            "path: {result}"
        );
        assert!(result.ends_with(".png"), "path: {result}");

        let map = written.lock().unwrap();
        assert_eq!(map.get(&result).unwrap(), data);
    }

    #[test]
    fn test_write_binary_to_vfs_unique_paths() {
        let paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let paths_clone = Arc::clone(&paths);

        let write_fn = |path: &str, _data: &[u8]| -> Result<(), AvixError> {
            paths_clone.lock().unwrap().push(path.to_string());
            Ok(())
        };

        let p1 = write_binary_to_vfs(1, "audio", b"a", "mp3", &write_fn).unwrap();
        let p2 = write_binary_to_vfs(1, "audio", b"b", "mp3", &write_fn).unwrap();
        assert_ne!(p1, p2);
    }

    #[test]
    fn test_write_binary_to_vfs_propagates_error() {
        let write_fn = |_path: &str, _data: &[u8]| -> Result<(), AvixError> {
            Err(AvixError::ConfigParse("vfs write failed".into()))
        };

        let result = write_binary_to_vfs(1, "img", b"data", "png", &write_fn);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("vfs write failed"));
    }

    #[test]
    fn test_write_binary_to_vfs_includes_agent_pid() {
        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let captured_clone = Arc::clone(&captured);

        let write_fn = |path: &str, _data: &[u8]| -> Result<(), AvixError> {
            *captured_clone.lock().unwrap() = Some(path.to_string());
            Ok(())
        };

        let result = write_binary_to_vfs(999, "speech", b"audio", "wav", &write_fn).unwrap();
        assert!(result.contains("/proc/999/scratch/"), "path: {result}");
        assert!(result.ends_with(".wav"), "path: {result}");
    }
}
