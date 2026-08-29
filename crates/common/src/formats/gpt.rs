use crate::bytes::{read_u32, read_u64};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

pub const GPT_HEADER_OFFSET: u64 = 512;
pub const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";

const GPT_HEADER_SIZE: usize = 92;
const GPT_ENTRY_MIN_SIZE: u32 = 128;
const GPT_ENTRY_MAX_SIZE: u32 = 4096;
const GPT_ENTRY_MAX_COUNT: u32 = 16_384;
const LOGICAL_BLOCK_SIZE: u64 = 512;
const PTABLE_SCAN_LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GptHeader {
    pub revision: u32,
    pub header_size: u32,
    pub current_lba: u64,
    pub backup_lba: u64,
    pub first_usable_lba: u64,
    pub last_usable_lba: u64,
    pub disk_guid: String,
    pub partition_entry_lba: u64,
    pub partition_entry_count: u32,
    pub partition_entry_size: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GptPartition {
    pub index: u32,
    pub type_guid: String,
    pub unique_guid: String,
    pub first_lba: u64,
    pub last_lba: u64,
    pub attributes: u64,
    pub name: String,
}

impl GptPartition {
    pub fn sector_count(&self) -> u64 {
        self.last_lba - self.first_lba + 1
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GptTable {
    pub image_offset: u64,
    pub entry_array_offset: u64,
    pub header: GptHeader,
    pub partitions: Vec<GptPartition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GptInfo {
    pub tables: Vec<GptTable>,
}

impl GptInfo {
    pub fn partition_count(&self) -> usize {
        self.tables.iter().map(|table| table.partitions.len()).sum()
    }
}

pub fn parse_image(path: &Path) -> io::Result<GptInfo> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    if length < GPT_HEADER_OFFSET + GPT_HEADER_SIZE as u64 {
        return Err(invalid("image is too small for a GPT header"));
    }

    let header_offsets = find_header_offsets(&mut file, length)?;
    if header_offsets.is_empty() {
        return Err(invalid("not an EFI GPT image (EFI PART signature missing)"));
    }

    let mut tables = Vec::new();
    let mut empty_tables = Vec::new();
    for header_offset in header_offsets {
        let header = match parse_header(&mut file, length, header_offset) {
            Ok(header) => header,
            Err(_) if header_offset != GPT_HEADER_OFFSET => continue,
            Err(error) => return Err(error),
        };

        let standard_entry_offset = header
            .partition_entry_lba
            .checked_mul(LOGICAL_BLOCK_SIZE)
            .ok_or_else(|| invalid("GPT partition entry offset overflows"))?;
        let adjacent_entry_offset = header_offset
            .checked_add(LOGICAL_BLOCK_SIZE)
            .ok_or_else(|| invalid("GPT adjacent partition entry offset overflows"))?;
        let entry_offsets = if standard_entry_offset == adjacent_entry_offset {
            vec![standard_entry_offset]
        } else {
            vec![standard_entry_offset, adjacent_entry_offset]
        };

        let mut best_candidate: Option<(u64, Vec<GptPartition>)> = None;
        for entry_offset in entry_offsets {
            let Ok(partitions) = parse_partitions(&mut file, length, entry_offset, &header) else {
                continue;
            };
            if best_candidate
                .as_ref()
                .is_none_or(|(_, current)| partitions.len() > current.len())
            {
                best_candidate = Some((entry_offset, partitions));
            }
        }
        let Some((entry_array_offset, partitions)) = best_candidate else {
            if header_offset == GPT_HEADER_OFFSET {
                return Err(invalid(
                    "GPT partition entry array extends beyond the image",
                ));
            }
            continue;
        };

        let table = GptTable {
            image_offset: header_offset,
            entry_array_offset,
            header,
            partitions,
        };
        if table.partitions.is_empty() {
            empty_tables.push(table);
        } else {
            tables.push(table);
        }
    }

    if tables.is_empty() {
        if let Some(table) = empty_tables.into_iter().next() {
            tables.push(table);
        }
    }
    Ok(GptInfo { tables })
}

fn find_header_offsets(file: &mut File, length: u64) -> io::Result<Vec<u64>> {
    let scan_length = length.min(PTABLE_SCAN_LIMIT);
    let scan_length = usize::try_from(scan_length)
        .map_err(|_| invalid("GPT scan region does not fit in memory"))?;
    let mut bytes = vec![0_u8; scan_length];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut bytes)?;

    let mut offsets = Vec::new();
    for offset in (GPT_HEADER_OFFSET as usize..=scan_length - GPT_SIGNATURE.len())
        .step_by(LOGICAL_BLOCK_SIZE as usize)
    {
        if &bytes[offset..offset + GPT_SIGNATURE.len()] == GPT_SIGNATURE {
            offsets.push(offset as u64);
        }
    }
    Ok(offsets)
}

fn parse_header(file: &mut File, length: u64, offset: u64) -> io::Result<GptHeader> {
    let header_end = offset
        .checked_add(GPT_HEADER_SIZE as u64)
        .ok_or_else(|| invalid("GPT header offset overflows"))?;
    if header_end > length {
        return Err(invalid("GPT header extends beyond the image"));
    }
    let mut header_bytes = [0_u8; GPT_HEADER_SIZE];
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(&mut header_bytes)?;
    if &header_bytes[..GPT_SIGNATURE.len()] != GPT_SIGNATURE {
        return Err(invalid("not an EFI GPT image (EFI PART signature missing)"));
    }

    let header_size = read_u32(&header_bytes, 12)?;
    if !(GPT_HEADER_SIZE as u32..=LOGICAL_BLOCK_SIZE as u32).contains(&header_size) {
        return Err(invalid("GPT header size is outside the logical block"));
    }

    let entry_count = read_u32(&header_bytes, 80)?;
    let entry_size = read_u32(&header_bytes, 84)?;
    if entry_count > GPT_ENTRY_MAX_COUNT {
        return Err(invalid("GPT partition entry count is unreasonably large"));
    }
    if !(GPT_ENTRY_MIN_SIZE..=GPT_ENTRY_MAX_SIZE).contains(&entry_size) {
        return Err(invalid("GPT partition entry size is unsupported"));
    }

    let entry_lba = read_u64(&header_bytes, 72)?;
    Ok(GptHeader {
        revision: read_u32(&header_bytes, 8)?,
        header_size,
        current_lba: read_u64(&header_bytes, 24)?,
        backup_lba: read_u64(&header_bytes, 32)?,
        first_usable_lba: read_u64(&header_bytes, 40)?,
        last_usable_lba: read_u64(&header_bytes, 48)?,
        disk_guid: format_guid(&header_bytes[56..72]),
        partition_entry_lba: entry_lba,
        partition_entry_count: entry_count,
        partition_entry_size: entry_size,
    })
}

fn parse_partitions(
    file: &mut File,
    length: u64,
    entry_offset: u64,
    header: &GptHeader,
) -> io::Result<Vec<GptPartition>> {
    let entry_bytes = u64::from(header.partition_entry_count)
        .checked_mul(u64::from(header.partition_entry_size))
        .ok_or_else(|| invalid("GPT partition entry array size overflows"))?;
    let entry_end = entry_offset
        .checked_add(entry_bytes)
        .ok_or_else(|| invalid("GPT partition entry array end overflows"))?;
    if entry_end > length {
        return Err(invalid(
            "GPT partition entry array extends beyond the image",
        ));
    }

    file.seek(SeekFrom::Start(entry_offset))?;
    let mut entry = vec![0_u8; header.partition_entry_size as usize];
    let mut partitions = Vec::new();
    for index in 0..header.partition_entry_count {
        file.read_exact(&mut entry)?;
        if entry[..16].iter().all(|&byte| byte == 0) {
            continue;
        }

        let first_lba = read_u64(&entry, 32)?;
        let last_lba = read_u64(&entry, 40)?;
        if last_lba < first_lba {
            return Err(invalid("GPT partition has an invalid LBA range"));
        }
        partitions.push(GptPartition {
            index,
            type_guid: format_guid(&entry[..16]),
            unique_guid: format_guid(&entry[16..32]),
            first_lba,
            last_lba,
            attributes: read_u64(&entry, 48)?,
            name: parse_name(&entry[56..128]),
        });
    }

    Ok(partitions)
}

fn parse_name(bytes: &[u8]) -> String {
    let code_units = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|&unit| unit != 0)
        .collect::<Vec<_>>();
    String::from_utf16_lossy(&code_units)
}

fn format_guid(bytes: &[u8]) -> String {
    debug_assert_eq!(bytes.len(), 16);
    format!(
        "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        bytes[3],
        bytes[2],
        bytes[1],
        bytes[0],
        bytes[5],
        bytes[4],
        bytes[7],
        bytes[6],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    )
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
