use anyhow::{Context, Result, bail, ensure};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Component as PathComponent, Path, PathBuf};

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

#[derive(Debug, Copy, Clone, Default, ValueEnum, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    pub output_name: String,
    pub component_type: u8,
    pub size: u64,
    pub data_offset: u64,
}

#[derive(Debug, Clone)]
pub struct PackageIndex {
    pub layout: UpdateLayout,
    pub data_offset: u64,
    pub components: Vec<Component>,
}

pub fn list_file(input: &Path, layout: UpdateLayout) -> Result<()> {
    let file =
        File::open(input).with_context(|| format!("opening update package {}", input.display()))?;
    let length = file.metadata()?.len();
    let mut reader = BufReader::with_capacity(IO_BUFFER_SIZE, file);
    let index = read_index(&mut reader, Some(length), layout)?;

    println!(
        "layout={:?} components={} data_offset={}",
        index.layout,
        index.components.len(),
        index.data_offset
    );
    for (number, component) in index.components.iter().enumerate() {
        println!(
            "{:>3}  {:<36} type={} size={} offset={}",
            number + 1,
            component.output_name,
            component.component_type,
            component.size,
            component.data_offset
        );
    }
    Ok(())
}

pub fn unpack_file(
    input: &Path,
    out: &Path,
    layout: UpdateLayout,
    force: bool,
) -> Result<Vec<Component>> {
    let file =
        File::open(input).with_context(|| format!("opening update package {}", input.display()))?;
    let length = file.metadata()?.len();
    let reader = BufReader::with_capacity(IO_BUFFER_SIZE, file);
    unpack_reader(reader, Some(length), out, layout, force)
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

    let compinfo_len = read_u16(&header, COMPINFO_LEN_OFFSET) as usize;
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

    let header_tlv_type = read_u16(header, 0);
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
        let size = read_u64(record, size_offset);
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
        !name.contains('/') && !name.contains('\\') && !name.contains('\0'),
        "unsafe component name {name:?}"
    );
    let mut parts = Path::new(name).components();
    ensure!(
        matches!(parts.next(), Some(PathComponent::Normal(_))) && parts.next().is_none(),
        "unsafe component path {name:?}"
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
    Ok((read_u16(&header, 0), read_u32(&header, 2) as u64))
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
    let temporary_path = temporary_path(out, position);
    if temporary_path.exists() {
        if force {
            fs::remove_file(&temporary_path)?;
        } else {
            bail!(
                "temporary output already exists: {}",
                temporary_path.display()
            );
        }
    }

    let result = (|| -> Result<()> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .with_context(|| format!("creating {}", temporary_path.display()))?;
        let mut writer = BufWriter::with_capacity(IO_BUFFER_SIZE, file);
        let copied = io::copy(&mut reader.take(component.size), &mut writer)
            .with_context(|| format!("extracting {}", component.output_name))?;
        ensure!(
            copied == component.size,
            "component {} is truncated: expected {} bytes, read {copied}",
            component.output_name,
            component.size
        );
        writer.flush()?;
        if force && final_path.exists() {
            fs::remove_file(&final_path)?;
        }
        fs::rename(&temporary_path, &final_path).with_context(|| {
            format!(
                "moving {} to {}",
                temporary_path.display(),
                final_path.display()
            )
        })?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn temporary_path(out: &Path, position: usize) -> PathBuf {
    out.join(format!(".haucet-component-{position}.part"))
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().expect("fixed range"))
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("fixed range"))
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("fixed range"))
}
