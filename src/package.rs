use crate::erofs;
use crate::tools::ToolPaths;
use crate::update_bin::{self, Component, UpdateLayout};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Component as PathComponent, Path, PathBuf};
use zip::ZipArchive;

const PACKAGE_MANIFEST: &str = "haucet-package.json";
const HARMONY_MAGIC: &[u8; 8] = b"HARMONY!";

#[derive(Debug, Serialize, Deserialize)]
struct PackageManifest {
    version: u32,
    source: String,
    update_bin_stored: bool,
    components: Vec<Component>,
    unpacked_erofs: Vec<String>,
    unpacked_ramdisks: Vec<String>,
}

pub fn unpack_full(
    input: &Path,
    out: &Path,
    tools: &ToolPaths,
    partitions: &[String],
    all_erofs: bool,
    layout: UpdateLayout,
    force: bool,
) -> Result<()> {
    prepare_output(out, force)?;
    let package_dir = out.join("package");
    let images_dir = out.join("images");
    let partitions_dir = out.join("partitions");
    fs::create_dir_all(&package_dir)?;
    fs::create_dir_all(&images_dir)?;
    fs::create_dir_all(&partitions_dir)?;

    let file =
        File::open(input).with_context(|| format!("opening Huawei package {}", input.display()))?;
    let mut archive = ZipArchive::new(file).context("opening ZIP/ZIP64 archive")?;
    let mut components = None;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let enclosed = entry
            .enclosed_name()
            .with_context(|| format!("unsafe ZIP entry name {:?}", entry.name()))?;
        if enclosed.file_name() == Some(OsStr::new("update.bin")) {
            ensure!(
                components.is_none(),
                "archive contains multiple update.bin entries"
            );
            let size = entry.size();
            eprintln!(
                "streaming update.bin ({} bytes) directly into component images",
                size
            );
            components = Some(update_bin::unpack_reader(
                &mut entry,
                Some(size),
                &images_dir,
                layout,
                force,
            )?);
        } else {
            extract_zip_entry(&mut entry, &package_dir, &enclosed, force)?;
        }
    }
    let components = components.context("archive does not contain update.bin")?;

    let selected = select_partitions(&components, partitions, all_erofs, &images_dir)?;
    let explicitly_selected = !partitions.is_empty();
    let mut unpacked_erofs = Vec::new();
    let mut unpacked_ramdisks = Vec::new();
    for component in selected {
        let image = images_dir.join(&component.output_name);
        if erofs::is_erofs(&image)? {
            let workspace = partitions_dir.join(&component.name);
            erofs::unpack(&image, &workspace, tools, force)?;
            unpacked_erofs.push(component.name.clone());
        } else if is_harmony_ramdisk(&image)? {
            let workspace = partitions_dir.join(&component.name);
            unpack_ramdisk(&image, &workspace, force)?;
            unpacked_ramdisks.push(component.name.clone());
        } else {
            let message = format!(
                "partition {} is neither EROFS nor a recognized HARMONY ramdisk; its image remains at {}",
                component.name,
                image.display()
            );
            if explicitly_selected {
                eprintln!("warning: {message}");
            } else {
                eprintln!("skipping {message}");
            }
        }
    }

    let manifest = PackageManifest {
        version: 1,
        source: input.to_string_lossy().into_owned(),
        update_bin_stored: false,
        components,
        unpacked_erofs,
        unpacked_ramdisks,
    };
    let json = serde_json::to_vec_pretty(&manifest)?;
    fs::write(out.join(PACKAGE_MANIFEST), json)?;
    eprintln!("wrote package workspace {}", out.display());
    Ok(())
}

fn prepare_output(out: &Path, force: bool) -> Result<()> {
    if out.exists() {
        let mut entries = fs::read_dir(out)?;
        if entries.next().is_some() {
            ensure!(force, "output directory is not empty: {}", out.display());
            fs::remove_dir_all(out)
                .with_context(|| format!("removing old output directory {}", out.display()))?;
        }
    }
    fs::create_dir_all(out)?;
    Ok(())
}

fn extract_zip_entry<R: io::Read>(
    entry: &mut zip::read::ZipFile<'_, R>,
    package_dir: &Path,
    enclosed: &Path,
    force: bool,
) -> Result<()> {
    let output = package_dir.join(enclosed);
    ensure!(
        output.starts_with(package_dir),
        "ZIP path escaped output directory"
    );
    if entry.is_dir() {
        fs::create_dir_all(&output)?;
        return Ok(());
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    if output.exists() {
        ensure!(force, "output already exists: {}", output.display());
        fs::remove_file(&output)?;
    }
    let temporary = output.with_extension("haucet-part");
    ensure!(
        !temporary.exists(),
        "temporary output already exists: {}",
        temporary.display()
    );
    let result = (|| -> Result<()> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let mut writer = BufWriter::new(file);
        let copied = io::copy(entry, &mut writer)?;
        ensure!(
            copied == entry.size(),
            "ZIP entry {:?} is truncated",
            entry.name()
        );
        writer.flush()?;
        fs::rename(&temporary, &output)?;
        set_unix_mode(&output, entry.unix_mode())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn select_partitions<'a>(
    components: &'a [Component],
    requested: &[String],
    all_erofs: bool,
    images_dir: &Path,
) -> Result<Vec<&'a Component>> {
    if all_erofs {
        let mut selected = Vec::new();
        for component in components.iter().filter(|item| item.component_type == 0) {
            if erofs::is_erofs(&images_dir.join(&component.output_name))? {
                selected.push(component);
            }
        }
        return Ok(selected);
    }

    if requested.is_empty() {
        let mut selected = Vec::new();
        for component in components.iter().filter(|item| item.component_type == 0) {
            let image = images_dir.join(&component.output_name);
            if erofs::is_erofs(&image)? || is_harmony_ramdisk(&image)? {
                selected.push(component);
            }
        }
        return Ok(selected);
    }

    let names: Vec<&str> = requested.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    let mut selected = Vec::new();
    for raw_name in names {
        let name = raw_name.strip_suffix(".img").unwrap_or(raw_name);
        validate_partition_name(name)?;
        if !seen.insert(name.to_owned()) {
            continue;
        }
        match components.iter().find(|component| component.name == name) {
            Some(component) => selected.push(component),
            None => bail!("partition {name:?} is not present in update.bin"),
        }
    }
    Ok(selected)
}

fn is_harmony_ramdisk(path: &Path) -> Result<bool> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let file_size = file.metadata()?.len();
    if file_size < 32 {
        return Ok(false);
    }
    let mut header = [0_u8; 16];
    file.read_exact(&mut header)?;
    if &header[..8] != HARMONY_MAGIC {
        return Ok(false);
    }
    let header_size = u64::from_be_bytes(header[8..16].try_into().expect("fixed range"));
    if !(32..=1024 * 1024).contains(&header_size) || header_size + 16 > file_size {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(header_size))?;
    let mut payload = [0_u8; 16];
    file.read_exact(&mut payload)?;
    Ok(is_ramdisk_payload(&payload))
}

fn is_ramdisk_payload(payload: &[u8; 16]) -> bool {
    payload.starts_with(&[0x1f, 0x8b, 0x08])
        || payload.starts_with(&[0xfd, 0x37, 0x7a, 0x58])
        || payload.starts_with(b"BZh")
        || payload.starts_with(&[0x5d, 0x00, 0x00])
        || payload.starts_with(&[0x04, 0x22, 0x4d, 0x18])
        || payload.starts_with(&[0x02, 0x21, 0x4c, 0x18])
        || payload.starts_with(b"070701")
        || payload.starts_with(b"070702")
}

fn unpack_ramdisk(image: &Path, workspace: &Path, force: bool) -> Result<()> {
    if workspace.exists() && fs::read_dir(workspace)?.next().is_some() {
        ensure!(
            force,
            "ramdisk workspace is not empty: {}",
            workspace.display()
        );
        fs::remove_dir_all(workspace)?;
    }
    fs::create_dir_all(workspace)?;
    let image = image
        .canonicalize()
        .with_context(|| format!("resolving ramdisk image {}", image.display()))?;
    let _working_directory = WorkingDirectory::enter(workspace)?;
    let args = vec![
        "ramdisk-tools".to_owned(),
        "unpack".to_owned(),
        image.to_string_lossy().into_owned(),
    ];
    let code = ramdisk_tools::cli::run(&args);
    ensure!(code == 0, "ramdisk-tools failed for {}", image.display());
    Ok(())
}

struct WorkingDirectory {
    original: PathBuf,
}

impl WorkingDirectory {
    fn enter(directory: &Path) -> Result<Self> {
        let original = std::env::current_dir()?;
        std::env::set_current_dir(directory)
            .with_context(|| format!("entering ramdisk workspace {}", directory.display()))?;
        Ok(Self { original })
    }
}

impl Drop for WorkingDirectory {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
    }
}

fn validate_partition_name(name: &str) -> Result<()> {
    let mut parts = Path::new(name).components();
    ensure!(
        matches!(parts.next(), Some(PathComponent::Normal(_))) && parts.next().is_none(),
        "unsafe partition name {name:?}"
    );
    ensure!(
        !name.contains('/') && !name.contains('\\'),
        "unsafe partition name {name:?}"
    );
    Ok(())
}

#[cfg(unix)]
fn set_unix_mode(path: &Path, mode: Option<u32>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_unix_mode(_path: &Path, _mode: Option<u32>) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    fn component(name: &str) -> Component {
        Component {
            name: name.to_owned(),
            output_name: format!("{name}.img"),
            component_type: 0,
            size: 1,
            data_offset: 0,
        }
    }

    #[test]
    fn selects_named_partitions_once() {
        let components = vec![component("system"), component("vendor")];
        let selected = select_partitions(
            &components,
            &["vendor".to_owned(), "vendor.img".to_owned()],
            false,
            Path::new("."),
        )
        .unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "vendor");
    }

    #[test]
    fn streams_update_bin_from_outer_zip() {
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("update_full_base.zip");
        let mut archive = ZipWriter::new(File::create(&archive_path).unwrap());
        archive
            .start_file("update.tag", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"tag").unwrap();
        archive
            .start_file("update.bin", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&synthetic_update_bin()).unwrap();
        archive.finish().unwrap();

        let tools = ToolPaths {
            extract_erofs: PathBuf::from("unused-extract.erofs"),
            mkfs_erofs: PathBuf::from("unused-mkfs.erofs"),
        };
        let output = temp.path().join("workspace");
        unpack_full(
            &archive_path,
            &output,
            &tools,
            &[],
            false,
            UpdateLayout::Auto,
            false,
        )
        .unwrap();
        assert_eq!(fs::read(output.join("package/update.tag")).unwrap(), b"tag");
        assert_eq!(
            fs::read(output.join("images/test_partition.img")).unwrap(),
            b"image"
        );
        assert!(output.join(PACKAGE_MANIFEST).is_file());
    }

    #[test]
    fn auto_selects_erofs_and_harmony_ramdisk() {
        let temp = tempfile::tempdir().unwrap();
        let mut erofs_image = vec![0_u8; 1028];
        erofs_image[1024..1028].copy_from_slice(&[0xe2, 0xe1, 0xf5, 0xe0]);
        fs::write(temp.path().join("system.img"), erofs_image).unwrap();

        let mut ramdisk_image = vec![0_u8; 0x810];
        ramdisk_image[..8].copy_from_slice(HARMONY_MAGIC);
        ramdisk_image[8..16].copy_from_slice(&0x800_u64.to_be_bytes());
        ramdisk_image[0x800..0x803].copy_from_slice(&[0x1f, 0x8b, 0x08]);
        fs::write(temp.path().join("ramdisk.img"), ramdisk_image).unwrap();

        let components = vec![component("system"), component("ramdisk"), component("boot")];
        fs::write(temp.path().join("boot.img"), [0_u8; 64]).unwrap();
        let selected = select_partitions(&components, &[], false, temp.path()).unwrap();
        let names: Vec<&str> = selected.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, ["system", "ramdisk"]);
    }

    fn synthetic_update_bin() -> Vec<u8> {
        const HEADER: usize = 180;
        const RECORD: usize = 87;
        const ADDRESS: usize = 32;
        let mut bytes = vec![0_u8; HEADER];
        bytes[0..2].copy_from_slice(&1_u16.to_le_bytes());
        bytes[178..180].copy_from_slice(&(RECORD as u16).to_le_bytes());
        let mut record = [0_u8; RECORD];
        let name = b"test_partition";
        record[..name.len()].copy_from_slice(name);
        record[ADDRESS + 4] = 0;
        record[ADDRESS + 15..ADDRESS + 23].copy_from_slice(&5_u64.to_le_bytes());
        bytes.extend(record);
        bytes.extend([0_u8; 16]);
        bytes.extend(8_u16.to_le_bytes());
        bytes.extend(0_u32.to_le_bytes());
        bytes.extend(b"image");
        bytes
    }
}
