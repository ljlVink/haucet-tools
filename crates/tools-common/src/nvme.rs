//TODO THE WRITE PROCESS MAY BE NOT VALIDATED, USE IT AT RISK
//SOME LOGICAL PROBLEMS EXISTS

use anyhow::{Context, Result, bail, ensure};
use crc32c::{crc32c as calculate_crc32c, crc32c_append};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const NVE_BLOCK_SIZE: usize = 0x20_000;
pub const NVE_ITEM_SIZE: usize = 128;
pub const NVE_ITEMS_PER_BLOCK: usize = 1023;
pub const NVE_HEADER_SIZE: usize = 128;
pub const NVE_DATA_SIZE: usize = 104;
pub const NVE_NAME_SIZE: usize = 8;
pub const NVE_INVALID_NUMBER: u32 = 0xFFFF_FFFF;
pub const NVE_HEADER_MAGIC: &[u8] = b"Hisi-NV-Partition";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NveImageSummary {
    pub file_size: u64,
    pub total_blocks: usize,
    pub active_blocks: usize,
    pub partition_name: String,
    pub version: u32,
    pub valid_items: usize,
    pub crc_valid: usize,
    pub crc_invalid: usize,
    pub blocks: Vec<NveBlockSummary>,
    pub items: Vec<NveItemSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NveBlockSummary {
    pub block_index: usize,
    pub offset: u64,
    pub header_present: bool,
    pub partition_name: String,
    pub version: u32,
    pub block_id: u32,
    pub block_count: u32,
    pub declared_valid_items: u32,
    pub valid_items: usize,
    pub age: u32,
    pub crc_valid: usize,
    pub crc_invalid: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NveItemSummary {
    pub block_index: usize,
    pub slot: usize,
    pub number: u32,
    pub name: String,
    pub property: u32,
    pub valid_size: usize,
    pub crc: u32,
    pub crc_valid: bool,
    pub value_hex: String,
    pub value_text: String,
    pub kernel_protected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NveEditResult {
    pub backup_path: String,
    pub updated_items: usize,
    pub value_size: usize,
    pub value_hex: String,
    pub mode: String,
    pub synced_all_blocks: bool,
    pub hashed_usrkey: bool,
}

#[derive(Debug, Clone)]
struct NveItem {
    slot: usize,
    raw: [u8; NVE_ITEM_SIZE],
}

impl NveItem {
    fn number(&self) -> u32 {
        read_u32(&self.raw, 0)
    }

    fn name(&self) -> String {
        decode_name(&self.raw[4..12])
    }

    fn property(&self) -> u32 {
        read_u32(&self.raw, 12)
    }

    fn valid_size(&self) -> usize {
        (read_u32(&self.raw, 16) as usize).min(NVE_DATA_SIZE)
    }

    fn crc(&self) -> u32 {
        read_u32(&self.raw, 20)
    }

    fn value(&self) -> &[u8] {
        &self.raw[24..24 + self.valid_size()]
    }

    fn is_valid(&self) -> bool {
        self.number() != NVE_INVALID_NUMBER && !self.name().is_empty()
    }

    fn crc_valid(&self) -> bool {
        self.crc() == compute_item_crc(&self.raw)
    }

    fn update_value(&mut self, value: &[u8]) {
        self.raw[16..20].copy_from_slice(&(value.len() as u32).to_le_bytes());
        self.raw[24..].fill(0);
        self.raw[24..24 + value.len()].copy_from_slice(value);
        let crc = compute_item_crc(&self.raw);
        self.raw[20..24].copy_from_slice(&crc.to_le_bytes());
    }

    fn summary(&self, block_index: usize) -> NveItemSummary {
        let name = self.name();
        let value = self.value();
        NveItemSummary {
            block_index,
            slot: self.slot,
            number: self.number(),
            name: name.clone(),
            property: self.property(),
            valid_size: self.valid_size(),
            crc: self.crc(),
            crc_valid: self.crc_valid(),
            value_hex: hex::encode(value),
            value_text: value_text(&name, value),
            kernel_protected: matches!(self.number(), 2 | 193 | 194 | 364),
        }
    }
}

#[derive(Debug, Clone)]
struct NveHeader {
    partition_name: String,
    version: u32,
    block_id: u32,
    block_count: u32,
    valid_items: u32,
    age: u32,
}

#[derive(Debug, Clone)]
struct NveBlock {
    index: usize,
    header: Option<NveHeader>,
    items: Vec<NveItem>,
}

impl NveBlock {
    fn active(&self) -> bool {
        self.header.is_some() || self.items.iter().any(NveItem::is_valid)
    }

    fn valid_items(&self) -> impl Iterator<Item = &NveItem> {
        self.items.iter().filter(|item| item.is_valid())
    }

    fn summary(&self) -> NveBlockSummary {
        let (partition_name, version, block_id, block_count, declared_valid_items, age) =
            match &self.header {
                Some(header) => (
                    header.partition_name.clone(),
                    header.version,
                    header.block_id,
                    header.block_count,
                    header.valid_items,
                    header.age,
                ),
                None => (String::new(), 0, 0, 0, 0, 0),
            };
        let mut crc_valid = 0;
        let mut crc_invalid = 0;
        for item in self.valid_items() {
            if item.crc_valid() {
                crc_valid += 1;
            } else {
                crc_invalid += 1;
            }
        }
        NveBlockSummary {
            block_index: self.index,
            offset: (self.index * NVE_BLOCK_SIZE) as u64,
            header_present: self.header.is_some(),
            partition_name,
            version,
            block_id,
            block_count,
            declared_valid_items,
            valid_items: crc_valid + crc_invalid,
            age,
            crc_valid,
            crc_invalid,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NveImage {
    raw: Vec<u8>,
    blocks: Vec<NveBlock>,
}

impl NveImage {
    pub fn from_file(path: &Path) -> Result<Self> {
        let raw =
            fs::read(path).with_context(|| format!("reading NVE image {}", path.display()))?;
        Self::from_bytes(raw)
    }

    pub fn from_bytes(raw: Vec<u8>) -> Result<Self> {
        ensure!(
            raw.len() >= NVE_BLOCK_SIZE,
            "NVE image is too small: {} bytes",
            raw.len()
        );
        ensure!(
            raw.len() % NVE_BLOCK_SIZE == 0,
            "NVE image size must be a multiple of 0x{:X}: 0x{:X}",
            NVE_BLOCK_SIZE,
            raw.len()
        );

        let mut blocks = Vec::with_capacity(raw.len() / NVE_BLOCK_SIZE);
        for (index, block_raw) in raw.chunks_exact(NVE_BLOCK_SIZE).enumerate() {
            let header = parse_header(&block_raw[NVE_BLOCK_SIZE - NVE_HEADER_SIZE..]);
            let mut items = Vec::with_capacity(NVE_ITEMS_PER_BLOCK);
            for slot in 0..NVE_ITEMS_PER_BLOCK {
                let start = slot * NVE_ITEM_SIZE;
                let mut item_raw = [0_u8; NVE_ITEM_SIZE];
                item_raw.copy_from_slice(&block_raw[start..start + NVE_ITEM_SIZE]);
                items.push(NveItem {
                    slot,
                    raw: item_raw,
                });
            }
            blocks.push(NveBlock {
                index,
                header,
                items,
            });
        }

        ensure!(
            blocks.iter().any(NveBlock::active),
            "NVE image contains no valid partition blocks"
        );
        Ok(Self { raw, blocks })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.raw
    }

    pub fn summary(&self) -> NveImageSummary {
        let active = self.blocks.iter().filter(|block| block.active());
        let active_blocks = active.collect::<Vec<_>>();
        let first = active_blocks.first().copied();
        let mut crc_valid = 0;
        let mut crc_invalid = 0;
        for block in &active_blocks {
            for item in block.valid_items() {
                if item.crc_valid() {
                    crc_valid += 1;
                } else {
                    crc_invalid += 1;
                }
            }
        }

        let items: Vec<NveItemSummary> = first
            .map(|block| {
                block
                    .valid_items()
                    .map(|item| item.summary(block.index))
                    .collect()
            })
            .unwrap_or_default();
        let (partition_name, version) = first
            .and_then(|block| block.header.as_ref())
            .map(|header| (header.partition_name.clone(), header.version))
            .unwrap_or_default();
        let valid_items = items.len();

        NveImageSummary {
            file_size: self.raw.len() as u64,
            total_blocks: self.blocks.len(),
            active_blocks: active_blocks.len(),
            partition_name,
            version,
            valid_items,
            crc_valid,
            crc_invalid,
            blocks: active_blocks.iter().map(|block| block.summary()).collect(),
            items,
        }
    }

    pub fn write_entry(&mut self, key: &str, value: &[u8], sync_all_blocks: bool) -> Result<usize> {
        let key = normalize_key(key)?;
        ensure!(
            value.len() <= NVE_DATA_SIZE,
            "NVE value is too long: {} bytes (maximum {})",
            value.len(),
            NVE_DATA_SIZE
        );

        let first_active = self
            .blocks
            .iter()
            .position(NveBlock::active)
            .ok_or_else(|| anyhow::anyhow!("NVE image contains no active block"))?;
        let mut updated = 0;
        for (index, block) in self.blocks.iter_mut().enumerate() {
            if !block.active() || (!sync_all_blocks && index != first_active) {
                continue;
            }
            for item in &mut block.items {
                if item.is_valid() && item.name().eq_ignore_ascii_case(&key) {
                    item.update_value(value);
                    let start = index * NVE_BLOCK_SIZE + item.slot * NVE_ITEM_SIZE;
                    self.raw[start..start + NVE_ITEM_SIZE].copy_from_slice(&item.raw);
                    updated += 1;
                }
            }
        }
        ensure!(updated != 0, "NVE entry not found: {key}");
        Ok(updated)
    }
}

pub fn inspect(path: &Path) -> Result<NveImageSummary> {
    Ok(NveImage::from_file(path)?.summary())
}

pub fn edit_file_in_place(
    path: &Path,
    key: &str,
    input_value: &str,
    value_format: &str,
    sync_all_blocks: bool,
) -> Result<NveEditResult> {
    let mut image = NveImage::from_file(path)?;
    let key = normalize_key(key)?;
    let (value, mode) = encode_value(&key, input_value, value_format)?;
    let value_hex = hex::encode(&value);
    let hashed_usrkey = mode == "SHA-256";
    let updated_items = image.write_entry(&key, &value, sync_all_blocks)?;
    let backup_path = create_backup(path)?;
    write_in_place(path, image.as_bytes())?;
    Ok(NveEditResult {
        backup_path: backup_path.display().to_string(),
        updated_items,
        value_size: value.len(),
        value_hex,
        mode,
        synced_all_blocks: sync_all_blocks,
        hashed_usrkey,
    })
}

fn encode_value(
    key: &str,
    input: &str,
    value_format: &str,
) -> Result<(Vec<u8>, String)> {
    match value_format.trim().to_ascii_lowercase().as_str() {
        "hex" => {
            let compact = input
                .strip_prefix("0x")
                .or_else(|| input.strip_prefix("0X"))
                .unwrap_or(input)
                .chars()
                .filter(|ch| !ch.is_ascii_whitespace())
                .collect::<String>();
            let value = hex::decode(&compact).with_context(|| "invalid hexadecimal NVE value")?;
            ensure!(
                value.len() <= NVE_DATA_SIZE,
                "NVE value is too long: {} bytes (maximum {})",
                value.len(),
                NVE_DATA_SIZE
            );
            Ok((value, "Hex".to_owned()))
        }
        "text" | "ascii" | "utf8" => {
            let trimmed = input.trim();

            //Special judge for irregular partitions
            if key.eq_ignore_ascii_case("FBLOCK") {
                return match trimmed {
                    "0" => Ok((vec![0], "FBLOCK (unlocked)".to_owned())),
                    "1" => Ok((vec![1], "FBLOCK (locked)".to_owned())),
                    _ => bail!("FBLOCK only accepts 0 or 1"),
                };
            }

            // In OpenHarmony based os, I think it is useless to set a USRKEY.
            /*if key.eq_ignore_ascii_case("USRKEY")
                && auto_hash_usrkey
                && image.detect_hashed_usrkey()
                && trimmed.len() == 16
            {
                let digest = Sha256::digest(trimmed.as_bytes());
                return Ok((digest.to_vec(), "SHA-256".to_owned()));
            }*/

            let value = input.as_bytes().to_vec();
            ensure!(
                value.len() <= NVE_DATA_SIZE,
                "NVE value is too long: {} bytes (maximum {})",
                value.len(),
                NVE_DATA_SIZE
            );
            Ok((value, "Text".to_owned()))
        }
        other => bail!("unsupported NVE value format: {other} (use text or hex)"),
    }
}

fn normalize_key(key: &str) -> Result<String> {
    let key = key.trim();
    ensure!(!key.is_empty(), "NVE entry name cannot be empty");
    ensure!(
        key.len() <= NVE_NAME_SIZE,
        "NVE entry name is longer than 8 bytes"
    );
    ensure!(key.is_ascii(), "NVE entry name must be ASCII");
    Ok(key.to_ascii_uppercase())
}

fn parse_header(raw: &[u8]) -> Option<NveHeader> {
    if raw.len() != NVE_HEADER_SIZE || !raw.starts_with(NVE_HEADER_MAGIC) {
        return None;
    }
    Some(NveHeader {
        partition_name: decode_name(&raw[..32]),
        version: read_u32(raw, 32),
        block_id: read_u32(raw, 36),
        block_count: read_u32(raw, 40),
        valid_items: read_u32(raw, 44),
        age: read_u32(raw, 124),
    })
}

fn decode_name(raw: &[u8]) -> String {
    let end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).trim().to_owned()
}

fn read_u32(raw: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        raw[offset],
        raw[offset + 1],
        raw[offset + 2],
        raw[offset + 3],
    ])
}

fn compute_item_crc(raw: &[u8; NVE_ITEM_SIZE]) -> u32 {
    let crc = calculate_crc32c(&raw[..20]);
    crc32c_append(crc, &raw[24..])
}

fn value_text(name: &str, value: &[u8]) -> String {

    // Special Judge for some irregular partitions.
    if name.eq_ignore_ascii_case("FBLOCK") && value.len() == 1 {
        return match value[0] {
            0 => "0".to_owned(),
            1 => "1".to_owned(),
            _ => String::new(),
        };
    }

    let trimmed = value
        .iter()
        .rposition(|byte| *byte != 0)
        .map(|last| &value[..=last])
        .unwrap_or(&[]);
    if trimmed
        .iter()
        .all(|byte| *byte == b'\t' || *byte == b'\n' || *byte == b'\r' || (32..=126).contains(byte))
    {
        String::from_utf8_lossy(trimmed).trim().to_owned()
    } else {
        String::new()
    }
}

fn create_backup(path: &Path) -> Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("NVE image path has no file name"))?
        .to_string_lossy();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX epoch")?;
    let base = parent.join(format!(
        "{name}.bak_{}_{}",
        now.as_secs(),
        now.subsec_nanos()
    ));
    let mut backup = base.clone();
    let mut suffix = 1_u32;
    while backup.exists() {
        backup = parent.join(format!(
            "{name}.bak_{}_{}-{suffix}",
            now.as_secs(),
            now.subsec_nanos()
        ));
        suffix += 1;
    }
    fs::copy(path, &backup).with_context(|| {
        format!(
            "creating NVE backup {} from {}",
            backup.display(),
            path.display()
        )
    })?;
    Ok(backup)
}

fn write_in_place(path: &Path, raw: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("opening NVE image for writing {}", path.display()))?;
    file.write_all(raw)
        .with_context(|| format!("writing NVE image {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("flushing NVE image {}", path.display()))?;
    Ok(())
}
