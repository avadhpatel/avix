use crate::error::AvixError;
use std::path::Path;

pub async fn git_clone_to(url: &str, dest: &Path) -> Result<(), AvixError> {
    let status = tokio::process::Command::new("git")
        .args(["clone", "--depth=1", url, &dest.to_string_lossy()])
        .status()
        .await
        .map_err(|e| AvixError::ConfigParse(format!("git clone failed: {e}")))?;

    if !status.success() {
        return Err(AvixError::ConfigParse(format!(
            "git clone exited with code: {:?}",
            status.code()
        )));
    }
    Ok(())
}
