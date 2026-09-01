use anyhow::{Context, Result, bail};
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ToolPaths {
    pub mkfs_erofs: PathBuf,
}

#[cfg(windows)]
const TOOL_SUFFIX: &str = ".exe";
#[cfg(not(windows))]
const TOOL_SUFFIX: &str = "";

impl ToolPaths {
    pub fn discover(explicit: Option<PathBuf>) -> Result<Self> {
        let directory = match explicit {
            Some(path) => path,
            None => default_tools_dir()?,
        };
        let paths = Self {
            mkfs_erofs: directory.join(format!("mkfs.erofs{TOOL_SUFFIX}")),
        };
        paths.validate()?;
        Ok(paths)
    }

    fn validate(&self) -> Result<()> {
        if !self.mkfs_erofs.is_file() {
            bail!("required tool was not found: {}", self.mkfs_erofs.display());
        }
        Ok(())
    }
}

fn default_tools_dir() -> Result<PathBuf> {
    let executable = env::current_exe().context("locating the haucet-tools executable")?;
    for ancestor in executable.ancestors() {
        let candidate = ancestor.join("bin");
        if has_tools(&candidate) {
            return Ok(candidate);
        }
    }

    let manifest_candidate = Path::new(env!("CARGO_MANIFEST_DIR")).join("bin");
    if has_tools(&manifest_candidate) {
        return manifest_candidate
            .canonicalize()
            .context("resolving bundled EROFS tools");
    }

    bail!("required EROFS repacking tool was not found in bin/: expected mkfs.erofs{TOOL_SUFFIX}")
}

fn has_tools(directory: &Path) -> bool {
    directory.join(format!("mkfs.erofs{TOOL_SUFFIX}")).is_file()
}
