use anyhow::Result;
use std::path::Path;

#[cfg(unix)]
use anyhow::{Context, ensure};
#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Give an unpacked workspace back to the user who invoked the program.
///
/// Extractors need root privileges to reproduce image metadata faithfully. Once
/// extraction is complete, the host-side tree only needs to be editable; EROFS
/// repacking restores target ownership and modes from the recorded fs_config.
pub fn make_invoking_user_writable(path: &Path) -> Result<()> {
    make_invoking_user_writable_impl(path)
}

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
fn parse_id(name: &str, value: &std::ffi::OsStr) -> Result<u32> {
    let value = value
        .to_str()
        .with_context(|| format!("{name} is not valid UTF-8"))?;
    value
        .parse()
        .with_context(|| format!("invalid {name} value {value:?}"))
}

#[cfg(unix)]
fn normalize_entry(path: &Path, uid: libc::uid_t, gid: libc::gid_t, chown: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("reading extracted path metadata {}", path.display()))?;

    if chown {
        lchown(path, uid, gid)?;
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

#[cfg(unix)]
fn lchown(path: &Path, uid: libc::uid_t, gid: libc::gid_t) -> Result<()> {
    let path_bytes = path.as_os_str().as_bytes();
    ensure!(!path_bytes.contains(&0), "path contains a NUL byte");
    let path_c = CString::new(path_bytes).expect("NUL byte was checked");
    let result = unsafe { libc::lchown(path_c.as_ptr(), uid, gid) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
            .with_context(|| format!("changing extracted path owner {}", path.display()))
    }
}

#[cfg(not(unix))]
fn make_invoking_user_writable_impl(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn adds_owner_write_and_only_preserves_intended_execute_bits() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("locked");
        fs::create_dir(&directory).unwrap();
        let data = directory.join("data");
        let executable = directory.join("executable");
        fs::write(&data, b"data").unwrap();
        fs::write(&executable, b"executable").unwrap();
        fs::set_permissions(&data, fs::Permissions::from_mode(0o400)).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o050)).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o500)).unwrap();

        make_invoking_user_writable(&directory).unwrap();

        assert_eq!(mode(&directory), 0o700);
        assert_eq!(mode(&data), 0o600);
        assert_eq!(mode(&executable), 0o750);
    }

    #[test]
    fn does_not_follow_symlinks_outside_the_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        fs::create_dir(&workspace).unwrap();
        fs::write(&outside, b"outside").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o400)).unwrap();
        symlink(&outside, workspace.join("link")).unwrap();

        make_invoking_user_writable(&workspace).unwrap();

        assert_eq!(mode(&outside), 0o400);
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }
}
