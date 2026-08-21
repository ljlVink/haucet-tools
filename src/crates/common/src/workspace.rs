use anyhow::Result;
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;

pub struct WorkingDirectory {
    original: PathBuf,
}

impl WorkingDirectory {
    pub fn enter(directory: &Path) -> Result<Self> {
        let original = std::env::current_dir()?;
        std::env::set_current_dir(directory)
            .with_context(|| format!("entering workspace {}", directory.display()))?;
        Ok(Self { original })
    }
}

impl Drop for WorkingDirectory {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
    }
}

pub fn make_invoking_user_writable(path: &Path) -> Result<()> {
    make_invoking_user_writable_impl(path)
}

fn make_invoking_user_writable_impl(path: &Path) -> Result<()> {
    let effective_uid = unsafe { libc::geteuid() };
    let effective_gid = unsafe { libc::getegid() };
    let identity = if effective_uid == 0 {
        sudo_identity()?
    } else {
        Some((effective_uid, effective_gid))
    };

    let Some((uid, gid)) = identity else {
        eprintln!(
            "warning: running as root without a non-root SUDO_UID/SUDO_GID; \
             extracted ownership and modes were left unchanged"
        );
        return Ok(());
    };

    normalize_entry(path, uid, gid, effective_uid == 0)?;
    eprintln!(
        "made extracted workspace {} writable for uid {uid}, gid {gid}",
        path.display()
    );
    Ok(())
}

fn sudo_identity() -> Result<Option<(libc::uid_t, libc::gid_t)>> {
    let uid = std::env::var_os("SUDO_UID");
    let gid = std::env::var_os("SUDO_GID");
    match (uid, gid) {
        (None, None) => Ok(None),
        (Some(uid), Some(gid)) => {
            let uid = parse_id("SUDO_UID", &uid)?;
            let gid = parse_id("SUDO_GID", &gid)?;
            if uid == 0 {
                Ok(None)
            } else {
                Ok(Some((uid, gid)))
            }
        }
        _ => anyhow::bail!("SUDO_UID and SUDO_GID must either both be set or both be absent"),
    }
}

fn parse_id(name: &str, value: &std::ffi::OsStr) -> Result<u32> {
    let value = value
        .to_str()
        .with_context(|| format!("{name} is not valid UTF-8"))?;
    value
        .parse()
        .with_context(|| format!("invalid {name} value {value:?}"))
}

fn normalize_entry(path: &Path, uid: libc::uid_t, gid: libc::gid_t, chown: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("reading extracted path metadata {}", path.display()))?;

    if chown {
        let path_bytes = path.as_os_str().as_bytes();
        ensure!(!path_bytes.contains(&0), "path contains a NUL byte");
        let path_c = CString::new(path_bytes).expect("NUL byte was checked");
        let result = unsafe { libc::lchown(path_c.as_ptr(), uid, gid) };
        if result == 0 {
            return Ok(())
        } else {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("changing extracted path owner {}", path.display()))
        }

    }

    if metadata.file_type().is_symlink() {
        return Ok(());
    }

    let old_mode = metadata.permissions().mode();
    let mut new_mode = old_mode | 0o600;
    if metadata.is_dir() || old_mode & 0o111 != 0 {
        new_mode |= 0o100;
    }
    if new_mode != old_mode {
        fs::set_permissions(path, fs::Permissions::from_mode(new_mode))
            .with_context(|| format!("making extracted path writable {}", path.display()))?;
    }

    if metadata.is_dir() {
        for entry in fs::read_dir(path)
            .with_context(|| format!("reading extracted directory {}", path.display()))?
        {
            normalize_entry(&entry?.path(), uid, gid, chown)?;
        }
    }
    Ok(())
}
