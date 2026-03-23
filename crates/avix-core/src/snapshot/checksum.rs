use sha2::{Digest, Sha256};

use super::capture::SnapshotFile;

/// Compute a SHA-256 hex digest of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    hex::encode(hash)
}

/// Compute the integrity checksum for `file`.
///
/// The checksum is computed over the canonical YAML of the snapshot with the
/// `checksum` field zeroed out (set to `""`), so the checksum is stable and
/// can be verified after embedding.
pub fn compute_checksum(file: &SnapshotFile) -> String {
    let mut zeroed = file.clone();
    zeroed.spec.checksum = String::new();
    let yaml = zeroed.to_yaml().unwrap_or_default();
    sha256_hex(yaml.as_bytes())
}

/// Verify that the embedded checksum in `file` matches its content.
///
/// Returns `Ok(())` if the checksum is valid.
/// Returns `Err` with a description if there is a mismatch.
pub fn verify_checksum(file: &SnapshotFile) -> Result<(), crate::error::AvixError> {
    let stored =
        file.spec.checksum.strip_prefix("sha256:").ok_or_else(|| {
            crate::error::AvixError::ConfigParse("invalid checksum format".into())
        })?;
    let computed = compute_checksum(file);
    if stored != computed {
        return Err(crate::error::AvixError::ConfigParse(format!(
            "snapshot integrity check failed for '{}': stored={stored} computed={computed}",
            file.metadata.name
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::capture::{
        capture, CaptureParams, CapturedBy, SnapshotMemory, SnapshotTrigger,
    };

    fn minimal_params(goal: &str) -> SnapshotFile {
        capture(CaptureParams {
            agent_name: "researcher",
            pid: 57,
            username: "alice",
            goal,
            message_history: &[],
            temperature: 0.7,
            granted_tools: &["fs/read".to_string()],
            trigger: SnapshotTrigger::Manual,
            captured_by: CapturedBy::Kernel,
            memory: SnapshotMemory::default(),
            pending_requests: vec![],
            open_pipes: vec![],
        })
    }

    // T-SB-01 (partial): capture produces non-empty checksum starting with sha256:
    #[test]
    fn capture_produces_checksum() {
        let snap = minimal_params("test goal");
        assert!(!snap.spec.checksum.is_empty());
        assert!(snap.spec.checksum.starts_with("sha256:"));
    }

    // T-SB-02: checksum changes when content changes
    #[test]
    fn checksum_detects_content_change() {
        let snap1 = minimal_params("goal A");
        let snap2 = minimal_params("goal B");
        assert_ne!(snap1.spec.checksum, snap2.spec.checksum);
    }

    // T-SC-01: verify_checksum passes for a freshly captured snapshot
    #[test]
    fn verify_checksum_passes_for_fresh_snapshot() {
        let snap = minimal_params("test goal");
        assert!(verify_checksum(&snap).is_ok());
    }

    // T-SC-02: verify_checksum fails for a tampered snapshot
    #[test]
    fn verify_checksum_detects_tampering() {
        let mut snap = minimal_params("test goal");
        snap.spec.goal = "TAMPERED".into();
        assert!(verify_checksum(&snap).is_err());
        let msg = verify_checksum(&snap).unwrap_err().to_string();
        assert!(
            msg.contains("integrity"),
            "expected integrity error, got: {msg}"
        );
    }
}
