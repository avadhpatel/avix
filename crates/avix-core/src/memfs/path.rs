use crate::error::AvixError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VfsPath(String);

impl VfsPath {
    pub fn parse(s: &str) -> Result<Self, AvixError> {
        if !s.starts_with('/') {
            return Err(AvixError::ConfigParse(format!(
                "VFS path must be absolute: '{s}'"
            )));
        }
        if s.contains("..") {
            return Err(AvixError::ConfigParse(format!(
                "VFS path must not contain '..': '{s}'"
            )));
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parent(&self) -> Option<VfsPath> {
        let (s, _) = self.0.rsplit_once('/')?;
        if s.is_empty() {
            Some(VfsPath("/".to_string()))
        } else {
            Some(VfsPath(s.to_string()))
        }
    }

    pub fn file_name(&self) -> Option<&str> {
        self.0.rsplit_once('/').map(|(_, name)| name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parent_of_root_child_is_root() {
        let p = VfsPath::parse("/foo").unwrap();
        assert_eq!(p.parent().unwrap().as_str(), "/");
    }

    #[test]
    fn test_file_name() {
        let p = VfsPath::parse("/foo/bar.txt").unwrap();
        assert_eq!(p.file_name(), Some("bar.txt"));
    }
}
