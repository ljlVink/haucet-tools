use crate::bytes;
use crate::formats::erofs;
use crate::formats::harmony::HARMONY_MAGIC;
use crate::formats::header::{FileFormat, check_fmt};
use crate::fs_util;
use crate::process::CommandWindow;
use crate::tools::ToolPaths;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::str::FromStr;
use zip::ZipArchive;

const HEADER_LEN: usize = 180;
const COMPINFO_LEN_OFFSET: usize = 178;
const RESERVE_LEN: usize = 16;
const L1_RECORD_LEN: usize = 71;
const L2_RECORD_LEN: usize = 87;
const L1_ADDRESS_LEN: usize = 16;
const L2_ADDRESS_LEN: usize = 32;
const MAX_COMPONENTS: usize = 4096;
const MAX_METADATA_TLV: u64 = 512 * 1024 * 1024;
const IO_BUFFER_SIZE: usize = 8 * 1024 * 1024;

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateLayout {
    #[default]
    Auto,
    L1,
    L2,
}

impl UpdateLayout {
    fn record_len(self) -> usize {
        match self {
            Self::L1 => L1_RECORD_LEN,
            Self::L2 => L2_RECORD_LEN,
            Self::Auto => unreachable!("auto layout must be resolved"),
        }
    }

    fn address_len(self) -> usize {
        match self {
            Self::L1 => L1_ADDRESS_LEN,
            Self::L2 => L2_ADDRESS_LEN,
            Self::Auto => unreachable!("auto layout must be resolved"),
        }
    }
}

impl FromStr for UpdateLayout {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "l1" => Ok(Self::L1),
            "l2" => Ok(Self::L2),
            _ => Err(format!(
                "unknown update layout {s:?}; expected auto, l1, or l2"
            )),
        }
    }
}

impl fmt::Display for UpdateLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Auto => "auto",
            Self::L1 => "l1",
            Self::L2 => "l2",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    pub output_name: String,
    pub component_type: u8,
    pub size: u64,
    pub data_offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageIndex {
    pub layout: UpdateLayout,
    pub data_offset: u64,
    pub components: Vec<Component>,
}

pub fn unpack_reader<R: Read>(
    mut reader: R,
    total_length: Option<u64>,
    out: &Path,
    layout: UpdateLayout,
    force: bool,
) -> Result<Vec<Component>> {
    let index = read_index(&mut reader, total_length, layout)?;
    prepare_outputs(out, &index.components, force)?;

    for (position, component) in index.components.iter().enumerate() {
        eprintln!(
            "[{}/{}] extracting {} ({} bytes)",
            position + 1,
            index.components.len(),
            component.output_name,
            component.size
        );
        write_component(&mut reader, out, component, position, force)?;
    }
    Ok(index.components)
}

pub fn read_index<R: Read>(
    reader: &mut R,
    total_length: Option<u64>,
    requested_layout: UpdateLayout,
) -> Result<PackageIndex> {
    let mut header = [0_u8; HEADER_LEN];
    reader
        .read_exact(&mut header)
        .context("reading the 180-byte update.bin header")?;

    let compinfo_len = bytes::read_u16(&header, COMPINFO_LEN_OFFSET)? as usize;
    ensure!(compinfo_len > 0, "update.bin component table is empty");
    let mut table = vec![0_u8; compinfo_len];
    reader
        .read_exact(&mut table)
        .context("reading the update.bin component table")?;

    let (layout, mut components) = resolve_layout(&header, &table, requested_layout)?;

    let mut reserve = [0_u8; RESERVE_LEN];
    reader
        .read_exact(&mut reserve)
        .context("reading the update.bin reserved field")?;
    let mut offset = (HEADER_LEN + compinfo_len + RESERVE_LEN) as u64;
    offset = skip_package_metadata(reader, offset)?;

    let payload_size = components.iter().try_fold(0_u64, |sum, component| {
        sum.checked_add(component.size)
            .context("component payload size overflow")
    })?;
    if let Some(length) = total_length {
        let required = offset
            .checked_add(payload_size)
            .context("update.bin length overflow")?;
        ensure!(
            required <= length,
            "update.bin is truncated: components end at {required}, file length is {length}"
        );
    }

    let mut data_offset = offset;
    for component in &mut components {
        component.data_offset = data_offset;
        data_offset = data_offset
            .checked_add(component.size)
            .context("component offset overflow")?;
    }

    Ok(PackageIndex {
        layout,
        data_offset: offset,
        components,
    })
}

fn resolve_layout(
    header: &[u8; HEADER_LEN],
    table: &[u8],
    requested: UpdateLayout,
) -> Result<(UpdateLayout, Vec<Component>)> {
    if requested != UpdateLayout::Auto {
        let components = parse_component_table(table, requested)?;
        return Ok((requested, components));
    }

    let header_tlv_type = bytes::read_u16(header, 0)?;
    let preferred = match header_tlv_type {
        0x01 => Some(UpdateLayout::L2),
        0x11 => Some(UpdateLayout::L1),
        _ => None,
    };
    let mut candidates = Vec::new();
    if let Some(layout) = preferred {
        candidates.push(layout);
    }
    for layout in [UpdateLayout::L2, UpdateLayout::L1] {
        if !candidates.contains(&layout) {
            candidates.push(layout);
        }
    }

    let mut errors = Vec::new();
    for layout in candidates {
        match parse_component_table(table, layout) {
            Ok(components) => return Ok((layout, components)),
            Err(error) => errors.push(format!("{layout:?}: {error:#}")),
        }
    }
    bail!(
        "could not detect component table layout ({})",
        errors.join("; ")
    )
}

fn parse_component_table(table: &[u8], layout: UpdateLayout) -> Result<Vec<Component>> {
    let record_len = layout.record_len();
    let address_len = layout.address_len();
    ensure!(
        table.len().is_multiple_of(record_len),
        "component table length {} is not divisible by {record_len}",
        table.len()
    );
    let count = table.len() / record_len;
    ensure!(
        (1..=MAX_COMPONENTS).contains(&count),
        "invalid component count {count}"
    );

    let mut names = HashSet::with_capacity(count);
    let mut components = Vec::with_capacity(count);
    for record in table.chunks_exact(record_len) {
        let raw_name = &record[..address_len];
        let name_end = raw_name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(raw_name.len());
        let name = std::str::from_utf8(&raw_name[..name_end])
            .context("component name is not UTF-8")?
            .trim_matches('/')
            .to_owned();
        validate_component_name(&name)?;

        let component_type = record[address_len + 4];
        let size_offset = address_len + 15;
        let size = bytes::read_u64(record, size_offset)?;
        let output_name = output_name(&name, component_type);
        ensure!(
            names.insert(output_name.clone()),
            "duplicate component output name {output_name:?}"
        );
        components.push(Component {
            name,
            output_name,
            component_type,
            size,
            data_offset: 0,
        });
    }
    Ok(components)
}

fn validate_component_name(name: &str) -> Result<()> {
    ensure!(!name.is_empty(), "empty component name");
    ensure!(
        fs_util::is_simple_name(name),
        "unsafe component name {name:?}"
    );
    Ok(())
}

fn output_name(name: &str, component_type: u8) -> String {
    match name {
        "version_list" => "VERSION.mbn".to_owned(),
        "board_list" => "BOARD.list".to_owned(),
        _ if component_type == 0 => format!("{name}.img"),
        _ if component_type == 1 => format!("{name}.zip"),
        _ => name.to_owned(),
    }
}

fn skip_package_metadata<R: Read>(reader: &mut R, mut offset: u64) -> Result<u64> {
    let (kind, length) = read_tlv_header(reader).context("reading update.bin metadata TLV")?;
    offset += 6;
    match kind {
        0x06 => {
            ensure!(length <= MAX_METADATA_TLV, "hash-info TLV is too large");
            skip_exact(reader, length).context("skipping hash-info TLV")?;
            offset += length;

            let (hash_kind, hash_length) = read_tlv_header(reader)?;
            ensure!(
                hash_kind == 0x07,
                "expected hash-data TLV, found {hash_kind:#x}"
            );
            ensure!(
                hash_length <= MAX_METADATA_TLV,
                "hash-data TLV is too large"
            );
            offset += 6;
            skip_exact(reader, hash_length).context("skipping hash-data TLV")?;
            offset += hash_length;

            let (sign_kind, sign_length) = read_tlv_header(reader)?;
            ensure!(
                sign_kind == 0x08,
                "expected signature TLV, found {sign_kind:#x}"
            );
            ensure!(
                sign_length <= MAX_METADATA_TLV,
                "signature TLV is too large"
            );
            offset += 6;
            skip_exact(reader, sign_length).context("skipping signature TLV")?;
            offset += sign_length;
        }
        0x08 => {
            ensure!(length <= MAX_METADATA_TLV, "signature TLV is too large");
            skip_exact(reader, length).context("skipping signature TLV")?;
            offset += length;
        }
        other => bail!("unsupported update.bin metadata TLV {other:#x}"),
    }
    Ok(offset)
}

fn read_tlv_header<R: Read>(reader: &mut R) -> Result<(u16, u64)> {
    let mut header = [0_u8; 6];
    reader.read_exact(&mut header)?;
    Ok((
        bytes::read_u16(&header, 0)?,
        bytes::read_u32(&header, 2)? as u64,
    ))
}

fn skip_exact<R: Read>(reader: &mut R, length: u64) -> Result<()> {
    let copied = io::copy(&mut reader.take(length), &mut io::sink())?;
    ensure!(copied == length, "metadata is truncated");
    Ok(())
}

fn prepare_outputs(out: &Path, components: &[Component], force: bool) -> Result<()> {
    fs::create_dir_all(out)
        .with_context(|| format!("creating component output directory {}", out.display()))?;
    if !force {
        for component in components {
            let path = out.join(&component.output_name);
            ensure!(!path.exists(), "output already exists: {}", path.display());
        }
    }
    Ok(())
}

fn write_component<R: Read>(
    reader: &mut R,
    out: &Path,
    component: &Component,
    position: usize,
    force: bool,
) -> Result<()> {
    let final_path = out.join(&component.output_name);
    let label = format!("component-{position}");
    let temporary = fs_util::sibling_temporary(&final_path, &label)?;
    if temporary.exists() {
        ensure!(
            force,
            "temporary output already exists: {}",
            temporary.display()
        );
        fs::remove_file(&temporary)?;
    }
    if force && final_path.exists() {
        fs::remove_file(&final_path)?;
    }
    fs_util::atomic_write(&final_path, &label, |writer| {
        let copied = io::copy(&mut reader.take(component.size), writer)
            .with_context(|| format!("extracting {}", component.output_name))?;
        ensure!(
            copied == component.size,
            "component {} is truncated: expected {} bytes, read {copied}",
            component.output_name,
            component.size
        );
        Ok(())
    })
}

const PACKAGE_MANIFEST: &str = "haucet-package.json";
#[derive(Debug, Serialize, Deserialize)]
struct PackageManifest {
    version: u32,
    source: String,
    update_bin_stored: bool,
    components: Vec<Component>,
    unpacked_erofs: Vec<String>,
    unpacked_ramdisks: Vec<String>,
}

pub fn inspect(input: &Path, layout: UpdateLayout) -> Result<PackageIndex> {
    if is_update_bin_file(input) {
        let file = File::open(input)
            .with_context(|| format!("opening update package {}", input.display()))?;
        let length = file.metadata()?.len();
        return read_index(
            &mut BufReader::with_capacity(IO_BUFFER_SIZE, file),
            Some(length),
            layout,
        );
    }

    let file =
        File::open(input).with_context(|| format!("opening Huawei package {}", input.display()))?;
    let mut archive = ZipArchive::new(file).context("opening ZIP/ZIP64 archive")?;
    let mut index = None;
    for entry_index in 0..archive.len() {
        let mut entry = archive.by_index(entry_index)?;
        let enclosed = entry
            .enclosed_name()
            .with_context(|| format!("unsafe ZIP entry name {:?}", entry.name()))?;
        if enclosed.file_name() == Some(OsStr::new("update.bin")) {
            ensure!(
                index.is_none(),
                "archive contains multiple update.bin entries"
            );
            let size = entry.size();
            index = Some(read_index(&mut entry, Some(size), layout)?);
        }
    }
    index.context("archive does not contain update.bin")
}

pub fn unpack_full(
    input: &Path,
    out: &Path,
    partitions: &[String],
    all_erofs: bool,
    layout: UpdateLayout,
    force: bool,
) -> Result<()> {
    fs_util::ensure_output_does_not_contain(input, out)?;
    let tools = ToolPaths::discover(None)?;
    unpack_full_with_tools_window(
        input,
        out,
        &tools,
        FullUnpackOptions {
            partitions,
            all_erofs,
            layout,
            force,
            window: CommandWindow::Inherit,
        },
    )
}

pub fn unpack_full_with_tools(
    input: &Path,
    out: &Path,
    tools: &ToolPaths,
    partitions: &[String],
    all_erofs: bool,
    layout: UpdateLayout,
    force: bool,
) -> Result<()> {
    fs_util::ensure_output_does_not_contain(input, out)?;
    unpack_full_with_tools_window(
        input,
        out,
        tools,
        FullUnpackOptions {
            partitions,
            all_erofs,
            layout,
            force,
            window: CommandWindow::Hidden,
        },
    )
}

struct FullUnpackOptions<'a> {
    partitions: &'a [String],
    all_erofs: bool,
    layout: UpdateLayout,
    force: bool,
    window: CommandWindow,
}

fn unpack_full_with_tools_window(
    input: &Path,
    out: &Path,
    tools: &ToolPaths,
    options: FullUnpackOptions<'_>,
) -> Result<()> {
    let FullUnpackOptions {
        partitions,
        all_erofs,
        layout,
        force,
        window,
    } = options;
    prepare_output(out, input, force)?;
    let package_dir = out.join("package");
    let images_dir = out.join("images");
    let partitions_dir = out.join("partitions");
    fs::create_dir_all(&package_dir)?;
    fs::create_dir_all(&images_dir)?;
    fs::create_dir_all(&partitions_dir)?;

    let components = if is_update_bin_file(input) {
        let file = File::open(input)
            .with_context(|| format!("opening update package {}", input.display()))?;
        let size = file.metadata()?.len();
        eprintln!(
            "streaming update.bin ({} bytes) directly into component images",
            size
        );
        unpack_reader(
            BufReader::with_capacity(IO_BUFFER_SIZE, file),
            Some(size),
            &images_dir,
            layout,
            force,
        )?
    } else {
        let file = File::open(input)
            .with_context(|| format!("opening Huawei package {}", input.display()))?;
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
                components = Some(unpack_reader(
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
        components.context("archive does not contain update.bin")?
    };

    let selected = select_partitions(&components, partitions, all_erofs, &images_dir)?;
    let explicitly_selected = !partitions.is_empty();
    let mut unpacked_erofs = Vec::new();
    let mut unpacked_ramdisks = Vec::new();
    for component in selected {
        let image = images_dir.join(&component.output_name);
        if erofs::is_erofs(&image)? {
            let workspace = partitions_dir.join(&component.name);
            erofs::unpack_with_tools_window(&image, &workspace, tools, force, window)?;
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

fn prepare_output(out: &Path, input: &Path, force: bool) -> Result<()> {
    fs_util::prepare_dir_excluding(out, "output directory", force, &[input])
}

fn is_update_bin_file(input: &Path) -> bool {
    input
        .extension()
        .map(|extension| extension.to_string_lossy().eq_ignore_ascii_case("bin"))
        .unwrap_or(false)
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
    if output.exists() {
        ensure!(force, "output already exists: {}", output.display());
        fs::remove_file(&output)?;
    }
    let unix_mode = entry.unix_mode();
    fs_util::atomic_write(&output, "zip-entry", |writer| {
        let copied = io::copy(entry, writer)?;
        ensure!(
            copied == entry.size(),
            "ZIP entry {:?} is truncated",
            entry.name()
        );
        Ok(())
    })?;
    fs_util::set_unix_mode(&output, unix_mode)?;
    Ok(())
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
    let format = check_fmt(payload);
    format.is_compressed() || format == FileFormat::RAW
}

fn unpack_ramdisk(image: &Path, workspace: &Path, force: bool) -> Result<()> {
    fs_util::prepare_dir_excluding(workspace, "ramdisk workspace", force, &[image])?;
    let image = image
        .canonicalize()
        .with_context(|| format!("resolving ramdisk image {}", image.display()))?;
    crate::ramdisk::unpack(&image, workspace)
        .with_context(|| format!("unpacking ramdisk image {}", image.display()))
}

fn validate_partition_name(name: &str) -> Result<()> {
    ensure!(
        fs_util::is_simple_name(name),
        "unsafe partition name {name:?}"
    );
    Ok(())
}
