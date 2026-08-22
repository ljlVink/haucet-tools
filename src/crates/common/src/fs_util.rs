use anyhow::{Context, Result, ensure};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Component, Path, PathBuf};

const IO_BUFFER_SIZE: usize = 8 * 1024 * 1024;

pub fn prepare_dir(dir: &Path, what: &str, force: bool) -> Result<()> {
    if dir.exists() {
        let mut entries = fs::read_dir(dir)?;
        if entries.next().is_some() {
            ensure!(force, "{what} is not empty: {}", dir.display());
            fs::remove_dir_all(dir)
                .with_context(|| format!("removing old {what} {}", dir.display()))?;
        }
    }
    fs::create_dir_all(dir)?;
    Ok(())
}

pub fn is_simple_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        )
        && Path::new(name).components().count() == 1
}

pub fn safe_join(base: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    ensure!(
        !path.is_absolute(),
        "manifest path must be relative: {relative:?}"
    );
    ensure!(
        path.components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir)),
        "unsafe manifest path {relative:?}"
    );
    Ok(base.join(path))
}

pub fn sibling_temporary(output: &Path, label: &str) -> Result<PathBuf> {
    let name = output
        .file_name()
        .and_then(OsStr::to_str)
        .context("output filename is not UTF-8")?;
    Ok(output.with_file_name(format!(".{name}.{label}.part")))
}

pub fn atomic_write<F>(final_path: &Path, label: &str, write: F) -> Result<()>
where
    F: FnOnce(&mut BufWriter<File>) -> Result<()>,
{
    let temporary = sibling_temporary(final_path, label)?;
    ensure!(
        !temporary.exists(),
        "temporary output already exists: {}",
        temporary.display()
    );
    if let Some(parent) = final_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let result = (|| -> Result<()> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .with_context(|| format!("creating {}", temporary.display()))?;
        let mut writer = BufWriter::with_capacity(IO_BUFFER_SIZE, file);
        write(&mut writer)?;
        writer.flush()?;
        fs::rename(&temporary, final_path).with_context(|| {
            format!("moving {} to {}", temporary.display(), final_path.display())
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn set_unix_mode(path: &Path, mode: Option<u32>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))?;
    }
    Ok(())
}
