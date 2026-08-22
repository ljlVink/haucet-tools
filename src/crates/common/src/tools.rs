use anyhow::{Context, Result, bail};
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ToolPaths {
    pub extract_erofs: PathBuf,
    pub mkfs_erofs: PathBuf,
}

impl ToolPaths {
    pub fn discover(explicit: Option<PathBuf>) -> Result<Self> {
        let directory = match explicit {
            Some(path) => path,
            None => default_tools_dir()?,
        };
        let paths = Self {
            extract_erofs: directory.join("extract.erofs"),
            mkfs_erofs: directory.join("mkfs.erofs"),
        };
        paths.validate()?;
        Ok(paths)
    }

    fn validate(&self) -> Result<()> {
        for path in [&self.extract_erofs, &self.mkfs_erofs] {
            if !path.is_file() {
                bail!("required tool was not found: {}", path.display());
            }
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
            .context("resolving bundled Linux tools");
    }

    bail!("required EROFS tools were not found in bin/: expected extract.erofs and mkfs.erofs")
}

fn has_tools(directory: &Path) -> bool {
    directory.join("extract.erofs").is_file() && directory.join("mkfs.erofs").is_file()
}
