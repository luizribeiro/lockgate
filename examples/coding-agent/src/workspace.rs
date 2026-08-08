//! Read-only workspace operations exposed to explicitly granted tool plugins.

use anyhow::{Context, Result, bail};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_LISTED_FILES: usize = 500;

pub(crate) struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub(crate) fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .with_context(|| format!("failed to resolve workspace {}", root.as_ref().display()))?;
        if !root.is_dir() {
            bail!("workspace is not a directory: {}", root.display());
        }
        Ok(Self { root })
    }

    pub(crate) fn display(&self) -> String {
        self.root.display().to_string()
    }

    pub(crate) fn read_file(&self, path: &str) -> Result<String, String> {
        let path = self.resolve_file(path)?;
        let metadata = path.metadata().map_err(|error| error.to_string())?;
        if metadata.len() > MAX_FILE_BYTES {
            return Err(format!(
                "file is larger than the {MAX_FILE_BYTES}-byte example limit"
            ));
        }
        fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub(crate) fn list_files(&self) -> Result<Vec<String>, String> {
        let mut files = Vec::new();
        self.visit(&self.root, &mut files)
            .map_err(|error| error.to_string())?;
        files.sort();
        Ok(files)
    }

    fn resolve_file(&self, requested: &str) -> Result<PathBuf, String> {
        let relative = Path::new(requested);
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        {
            return Err("path must be a normalized, relative workspace path".to_owned());
        }
        let path = self
            .root
            .join(relative)
            .canonicalize()
            .map_err(|error| format!("{requested}: {error}"))?;
        if !path.starts_with(&self.root) || !path.is_file() {
            return Err("path does not name a file inside the workspace".to_owned());
        }
        Ok(path)
    }

    fn visit(&self, directory: &Path, files: &mut Vec<String>) -> std::io::Result<()> {
        if files.len() >= MAX_LISTED_FILES {
            return Ok(());
        }
        let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if files.len() >= MAX_LISTED_FILES {
                break;
            }
            let file_type = entry.file_type()?;
            let path = entry.path();
            let name = entry.file_name();
            if file_type.is_dir() {
                if name != ".git" && name != "target" {
                    self.visit(&path, files)?;
                }
            } else if file_type.is_file() {
                files.push(
                    path.strip_prefix(&self.root)
                        .expect("visited paths start beneath the workspace")
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lockgate-agent-{nonce}"));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        root
    }

    #[test]
    fn workspace_reads_and_lists_relative_files() {
        let root = fixture();
        let workspace = Workspace::new(&root).unwrap();

        assert_eq!(workspace.list_files().unwrap(), ["src/main.rs"]);
        assert_eq!(
            workspace.read_file("src/main.rs").unwrap(),
            "fn main() {}\n"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_rejects_paths_that_can_escape() {
        let root = fixture();
        let workspace = Workspace::new(&root).unwrap();

        assert!(workspace.read_file("../secret").is_err());
        assert!(workspace.read_file("/etc/passwd").is_err());

        fs::remove_dir_all(root).unwrap();
    }
}
