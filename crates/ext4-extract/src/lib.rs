use anyhow::{Context, Result, anyhow, ensure};
use ext4_view::{Ext4, FileType, PathBuf as Ext4PathBuf};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

const COPY_BUFFER_SIZE: usize = 1024 * 1024;
const VFS_CAP_REVISION_MASK: u32 = 0xFF00_0000;
const VFS_CAP_REVISION_1: u32 = 0x0100_0000;
const VFS_CAP_REVISION_2: u32 = 0x0200_0000;
const VFS_CAP_REVISION_3: u32 = 0x0300_0000;
const XATTR_CAPS_SZ_1: usize = 12;
const XATTR_CAPS_SZ_2: usize = 20;
const XATTR_CAPS_SZ_3: usize = 24;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractReport {
    pub directories: u64,
    pub files: u64,
    pub symlinks: u64,
    pub special_files_skipped: u64,
    pub bytes_written: u64,
    pub nodes: Vec<NodeMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeMetadata {
    pub path: String,
    pub uid: u32,
    pub gid: u32,
    pub mode: u16,
    pub selinux_context: Option<String>,
    pub capabilities: Option<u64>,
}

pub fn extract(image: &Path, output: &Path) -> Result<ExtractReport> {
    ensure!(
        output.is_dir(),
        "ext4 extraction output is not a directory: {}",
        output.display()
    );
    let output_root = output
        .canonicalize()
        .with_context(|| format!("resolving ext4 output directory {}", output.display()))?;
    let filesystem = Ext4::load_from_path(image)
        .map_err(|error| anyhow!(error.to_string()))
        .with_context(|| format!("opening ext4 filesystem {}", image.display()))?;
    let root = Ext4PathBuf::try_from("/").expect("the ext4 root path is valid");
    let mut extractor = Extractor {
        filesystem,
        output_root,
        report: ExtractReport::default(),
    };
    extractor.extract_directory(&root, output)?;
    extractor.report.nodes = read_node_metadata(image)?;
    Ok(extractor.report)
}

fn read_node_metadata(image: &Path) -> Result<Vec<NodeMetadata>> {
    let file = File::open(image)
        .with_context(|| format!("opening ext4 metadata source {}", image.display()))?;
    let options = ext4_metadata::Options {
        checksums: ext4_metadata::Checksums::Enabled,
    };
    let filesystem = ext4_metadata::SuperBlock::new_with_options(file, &options)
        .with_context(|| format!("reading ext4 metadata from {}", image.display()))?;
    let root = filesystem
        .root()
        .with_context(|| format!("reading ext4 root inode from {}", image.display()))?;
    let mut nodes = Vec::new();
    filesystem
        .walk(&root, "", &mut |_, path, inode, _| {
            let path = if path.is_empty() { "/" } else { path };
            let selinux_context = inode
                .stat
                .xattrs
                .get("security.selinux")
                .map(|value| {
                    let end = value
                        .iter()
                        .rposition(|byte| *byte != 0)
                        .map_or(0, |index| index + 1);
                    let value = &value[..end];
                    String::from_utf8_lossy(value).into_owned()
                })
                .filter(|value| !value.is_empty());
            let capabilities = inode
                .stat
                .xattrs
                .get("security.capability")
                .and_then(|value| parse_vfs_cap_data(value))
                .filter(|value| *value != 0);
            nodes.push(NodeMetadata {
                path: path.to_owned(),
                uid: inode.stat.uid,
                gid: inode.stat.gid,
                mode: inode.stat.file_mode,
                selinux_context,
                capabilities,
            });
            Ok(true)
        })
        .with_context(|| format!("walking ext4 metadata in {}", image.display()))?;
    nodes.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    Ok(nodes)
}

fn parse_vfs_cap_data(data: &[u8]) -> Option<u64> {
    let magic_etc = u32::from_le_bytes(data.get(0..4)?.try_into().ok()?);
    match magic_etc & VFS_CAP_REVISION_MASK {
        VFS_CAP_REVISION_1 if data.len() == XATTR_CAPS_SZ_1 => {
            Some(u32::from_le_bytes(data[4..8].try_into().ok()?) as u64)
        }
        VFS_CAP_REVISION_2 | VFS_CAP_REVISION_3
            if data.len() == XATTR_CAPS_SZ_2 || data.len() == XATTR_CAPS_SZ_3 =>
        {
            let low = u32::from_le_bytes(data[4..8].try_into().ok()?) as u64;
            let high = u32::from_le_bytes(data[12..16].try_into().ok()?) as u64;
            Some(low | high << 32)
        }
        _ => None,
    }
}

struct Extractor {
    filesystem: Ext4,
    output_root: PathBuf,
    report: ExtractReport,
}

impl Extractor {
    fn extract_directory(&mut self, image_dir: &Ext4PathBuf, host_dir: &Path) -> Result<()> {
        let entries = self
            .filesystem
            .read_dir(image_dir)
            .map_err(|error| anyhow!(error.to_string()))
            .with_context(|| format!("reading ext4 directory {}", image_dir.display()))?;

        for entry in entries {
            let entry = entry
                .map_err(|error| anyhow!(error.to_string()))
                .with_context(|| format!("reading entries in {}", image_dir.display()))?;
            let name = entry.file_name();
            if name == b"." || name == b".." {
                continue;
            }

            let host_name = host_component(name.as_ref()).with_context(|| {
                format!(
                    "mapping ext4 entry {:?} below {} to a host filename",
                    name,
                    image_dir.display()
                )
            })?;
            let host_path = host_dir.join(host_name);
            let image_path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|error| anyhow!(error.to_string()))
                .with_context(|| format!("reading type of {}", image_path.display()))?;
            let metadata = entry
                .metadata()
                .map_err(|error| anyhow!(error.to_string()))
                .with_context(|| format!("reading metadata of {}", image_path.display()))?;

            match file_type {
                FileType::Directory => {
                    fs::create_dir(&host_path).with_context(|| {
                        format!("creating extracted directory {}", host_path.display())
                    })?;
                    self.report.directories += 1;
                    self.extract_directory(&image_path, &host_path)?;
                    set_mode(&host_path, metadata.mode())?;
                }
                FileType::Regular => {
                    self.extract_file(&image_path, &host_path, metadata.mode())?;
                }
                FileType::Symlink => {
                    self.extract_symlink(&image_path, &host_path)?;
                }
                FileType::BlockDevice
                | FileType::CharacterDevice
                | FileType::Fifo
                | FileType::Socket => {
                    self.report.special_files_skipped += 1;
                    eprintln!(
                        "skipping unsupported ext4 special file {} ({file_type:?})",
                        image_path.display()
                    );
                }
            }
        }
        Ok(())
    }

    fn extract_file(
        &mut self,
        image_path: &Ext4PathBuf,
        host_path: &Path,
        mode: u16,
    ) -> Result<()> {
        let mut input = self
            .filesystem
            .open(image_path)
            .map_err(|error| anyhow!(error.to_string()))
            .with_context(|| format!("opening ext4 file {}", image_path.display()))?;
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(host_path)
            .with_context(|| format!("creating extracted file {}", host_path.display()))?;
        let mut output = BufWriter::with_capacity(COPY_BUFFER_SIZE, output);
        let written = io::copy(&mut input, &mut output).with_context(|| {
            format!(
                "extracting ext4 file {} to {}",
                image_path.display(),
                host_path.display()
            )
        })?;
        output.flush()?;
        drop(output);
        set_mode(host_path, mode)?;
        self.report.files += 1;
        self.report.bytes_written = self
            .report
            .bytes_written
            .checked_add(written)
            .context("extracted ext4 byte count overflow")?;
        Ok(())
    }

    fn extract_symlink(&mut self, image_path: &Ext4PathBuf, host_path: &Path) -> Result<()> {
        let target = self
            .filesystem
            .read_link(image_path)
            .map_err(|error| anyhow!(error.to_string()))
            .with_context(|| format!("reading ext4 symlink {}", image_path.display()))?;
        let target_is_dir = self
            .filesystem
            .metadata(image_path)
            .is_ok_and(|metadata| metadata.is_dir());
        create_symlink(target.as_ref(), host_path, target_is_dir, &self.output_root).with_context(
            || {
                format!(
                    "creating extracted symlink {} -> {}",
                    host_path.display(),
                    target.display()
                )
            },
        )?;
        self.report.symlinks += 1;
        Ok(())
    }
}

#[cfg(unix)]
fn host_component(bytes: &[u8]) -> Result<OsString> {
    use std::os::unix::ffi::OsStringExt;

    ensure!(!bytes.is_empty(), "empty ext4 filename");
    ensure!(bytes != b"." && bytes != b"..", "unsafe ext4 filename");
    ensure!(
        !bytes.contains(&b'/') && !bytes.contains(&0),
        "unsafe ext4 filename"
    );
    Ok(OsString::from_vec(bytes.to_vec()))
}

#[cfg(windows)]
fn host_component(bytes: &[u8]) -> Result<OsString> {
    let name = std::str::from_utf8(bytes).context("ext4 filename is not UTF-8")?;
    ensure!(
        !name.is_empty()
            && !matches!(name, "." | "..")
            && !name.ends_with(['.', ' '])
            && !name.chars().any(|character| {
                character < '\u{20}'
                    || matches!(
                        character,
                        '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                    )
            }),
        "ext4 filename cannot be represented safely on Windows: {name:?}"
    );
    let stem = name.split('.').next().unwrap_or(name);
    ensure!(
        !is_windows_reserved_name(stem),
        "ext4 filename is reserved on Windows: {name:?}"
    );
    Ok(OsString::from(name))
}

#[cfg(windows)]
fn is_windows_reserved_name(stem: &str) -> bool {
    let stem = stem.to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u16) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(u32::from(mode)))
        .with_context(|| format!("setting permissions on {}", path.display()))
}

#[cfg(windows)]
fn set_mode(path: &Path, mode: u16) -> Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(mode & 0o222 == 0);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("setting permissions on {}", path.display()))
}

#[cfg(unix)]
fn create_symlink(
    target: &[u8],
    host_path: &Path,
    _target_is_dir: bool,
    _output_root: &Path,
) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;

    symlink(std::ffi::OsStr::from_bytes(target), host_path)?;
    Ok(())
}

#[cfg(windows)]
fn create_symlink(
    target: &[u8],
    host_path: &Path,
    target_is_dir: bool,
    output_root: &Path,
) -> Result<()> {
    use std::os::windows::fs::{symlink_dir, symlink_file};

    let target = windows_symlink_target(target, output_root)?;
    if target_is_dir {
        symlink_dir(target, host_path)?;
    } else {
        symlink_file(target, host_path)?;
    }
    Ok(())
}

#[cfg(windows)]
fn windows_symlink_target(target: &[u8], output_root: &Path) -> Result<PathBuf> {
    let absolute = target.first() == Some(&b'/');
    let mut result = if absolute {
        output_root.to_owned()
    } else {
        PathBuf::new()
    };
    for component in target.split(|byte| *byte == b'/') {
        if component.is_empty() || component == b"." {
            continue;
        }
        if component == b".." {
            ensure!(!absolute, "absolute ext4 symlink target escapes its root");
            result.push("..");
        } else {
            result.push(host_component(component)?);
        }
    }
    Ok(result)
}