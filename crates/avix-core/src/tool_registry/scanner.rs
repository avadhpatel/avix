use std::path::Path;

use crate::error::AvixError;
use crate::tool_registry::entry::ToolEntry;
use crate::types::tool::{ToolName, ToolState, ToolVisibility};

use super::descriptor::{ToolDescriptor, ToolVisibilitySpec};

pub struct ToolScanner;

impl ToolScanner {
    /// Scan `service_dir/tools/` for `*.tool.yaml` files and return parsed descriptors.
    /// Missing `tools/` directory → empty vec (not an error).
    pub fn scan(service_dir: &Path) -> Result<Vec<ToolDescriptor>, AvixError> {
        let tools_dir = service_dir.join("tools");
        if !tools_dir.exists() {
            return Ok(vec![]);
        }
        let mut descriptors = Vec::new();
        for entry in
            std::fs::read_dir(&tools_dir).map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
            let entry = entry.map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let path = entry.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if !name.ends_with(".tool.yaml") {
                continue;
            }
            let content = std::fs::read_to_string(&path)
                .map_err(|e| AvixError::ConfigParse(format!("{}: {e}", path.display())))?;
            let desc: ToolDescriptor = serde_yaml::from_str(&content)
                .map_err(|e| AvixError::ConfigParse(format!("{}: {e}", path.display())))?;
            descriptors.push(desc);
        }
        Ok(descriptors)
    }

    /// Scan and convert descriptors to `ToolEntry` records ready for the registry.
    pub fn scan_as_entries(
        service_name: &str,
        service_dir: &Path,
    ) -> Result<Vec<ToolEntry>, AvixError> {
        let entries = Self::scan(service_dir)?
            .into_iter()
            .filter_map(|desc| {
                ToolName::parse(&desc.name).ok().map(|name| ToolEntry {
                    name,
                    owner: service_name.to_string(),
                    state: match desc.status.state.as_str() {
                        "available" => ToolState::Available,
                        "degraded" => ToolState::Degraded,
                        _ => ToolState::Unavailable,
                    },
                    visibility: match &desc.visibility {
                        ToolVisibilitySpec::All => ToolVisibility::All,
                        ToolVisibilitySpec::User(u) => ToolVisibility::User(u.clone()),
                        ToolVisibilitySpec::Crew(c) => ToolVisibility::Crew(c.clone()),
                    },
                    descriptor: serde_json::to_value(&desc).unwrap_or_default(),
                })
            })
            .collect();
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_tool(dir: &TempDir, filename: &str, content: &str) {
        let tools_dir = dir.path().join("tools");
        std::fs::create_dir_all(&tools_dir).unwrap();
        std::fs::write(tools_dir.join(filename), content).unwrap();
    }

    #[test]
    fn parses_minimal_tool_descriptor() {
        let dir = TempDir::new().unwrap();
        write_tool(
            &dir,
            "fs-read.tool.yaml",
            "name: fs/read\ndescription: Read file contents\ncapabilities_required: [fs:read]\n",
        );
        let descs = ToolScanner::scan(dir.path()).unwrap();
        assert_eq!(descs.len(), 1);
        assert_eq!(descs[0].name, "fs/read");
        assert_eq!(descs[0].description, "Read file contents");
    }

    #[test]
    fn skips_non_tool_yaml_files() {
        let dir = TempDir::new().unwrap();
        write_tool(&dir, "README.md", "# readme");
        write_tool(&dir, "config.yaml", "key: val");
        write_tool(&dir, "fs-read.tool.yaml", "name: fs/read\ndescription: x\n");
        let descs = ToolScanner::scan(dir.path()).unwrap();
        assert_eq!(descs.len(), 1);
    }

    #[test]
    fn empty_vec_when_no_tools_dir() {
        let dir = TempDir::new().unwrap();
        let descs = ToolScanner::scan(dir.path()).unwrap();
        assert!(descs.is_empty());
    }

    #[test]
    fn scan_multiple_tools() {
        let dir = TempDir::new().unwrap();
        for n in ["github-list-prs", "github-create-issue"] {
            write_tool(
                &dir,
                &format!("{n}.tool.yaml"),
                &format!(
                    "name: github/{}\ndescription: tool\n",
                    n.replace("github-", "")
                ),
            );
        }
        let descs = ToolScanner::scan(dir.path()).unwrap();
        assert_eq!(descs.len(), 2);
    }

    #[test]
    fn scan_as_entries_produces_tool_entries() {
        let dir = TempDir::new().unwrap();
        write_tool(
            &dir,
            "list-prs.tool.yaml",
            "name: github/list-prs\ndescription: List PRs\n",
        );
        let entries = ToolScanner::scan_as_entries("github-svc", dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].owner, "github-svc");
    }

    #[test]
    fn tool_descriptor_streaming_defaults_false() {
        let dir = TempDir::new().unwrap();
        write_tool(&dir, "x-y.tool.yaml", "name: x/y\ndescription: d\n");
        let descs = ToolScanner::scan(dir.path()).unwrap();
        assert!(!descs[0].streaming);
        assert!(!descs[0].job);
    }

    #[test]
    fn tool_descriptor_job_flag() {
        let dir = TempDir::new().unwrap();
        write_tool(
            &dir,
            "video-transcode.tool.yaml",
            "name: video/transcode\ndescription: Encode\njob: true\njob_timeout: 3600s\n",
        );
        let descs = ToolScanner::scan(dir.path()).unwrap();
        assert!(descs[0].job);
        assert_eq!(descs[0].job_timeout.as_deref(), Some("3600s"));
    }

    #[test]
    fn invalid_yaml_returns_error() {
        let dir = TempDir::new().unwrap();
        write_tool(&dir, "bad.tool.yaml", "name: [invalid yaml{{");
        assert!(ToolScanner::scan(dir.path()).is_err());
    }

    #[test]
    fn scan_as_entries_skips_invalid_tool_names() {
        let dir = TempDir::new().unwrap();
        // Tool names with __ are invalid (wire-mangled names rejected by ToolName::parse)
        write_tool(
            &dir,
            "bad-name.tool.yaml",
            "name: fs__read\ndescription: bad\n",
        );
        let entries = ToolScanner::scan_as_entries("svc", dir.path()).unwrap();
        assert!(entries.is_empty());
    }
}
