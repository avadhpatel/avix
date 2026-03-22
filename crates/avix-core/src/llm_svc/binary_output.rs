use crate::error::AvixError;
use std::path::Path;

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
