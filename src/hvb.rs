// Streaming HVB support is implemented separately from ramdisk-tools because
// Huawei EROFS partitions can be several gigabytes and must not be buffered.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const FOOTER_SIZE: u64 = 104;
const FOOTER_MAGIC: &[u8; 8] = b"HVB\0\0\0\0\0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HvbFooter {
    pub cert_offset: u64,
    pub cert_size: u64,
    pub image_size: u64,
    pub partition_size: u64,
}

#[derive(Debug, Clone)]
pub struct HvbWrapper {
    pub footer: HvbFooter,
    pub certificate: Vec<u8>,
}

impl HvbFooter {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != FOOTER_SIZE as usize || &bytes[..8] != FOOTER_MAGIC {
            bail!("not an HVB footer")
        }
        Ok(Self {
            cert_offset: read_u64(bytes, 8),
            cert_size: read_u64(bytes, 16),
            image_size: read_u64(bytes, 24),
            partition_size: read_u64(bytes, 32),
        })
    }

    pub fn to_bytes(&self) -> [u8; FOOTER_SIZE as usize] {
        let mut bytes = [0_u8; FOOTER_SIZE as usize];
        bytes[..8].copy_from_slice(FOOTER_MAGIC);
        bytes[8..16].copy_from_slice(&self.cert_offset.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.cert_size.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.image_size.to_le_bytes());
        bytes[32..40].copy_from_slice(&self.partition_size.to_le_bytes());
        bytes
    }
}

impl HvbWrapper {
    pub fn read_from(path: &Path) -> Result<Option<Self>> {
        let mut file =
            File::open(path).with_context(|| format!("opening image {}", path.display()))?;
        let length = file.metadata()?.len();
        if length < FOOTER_SIZE {
            return Ok(None);
        }
        file.seek(SeekFrom::End(-(FOOTER_SIZE as i64)))?;
        let mut bytes = [0_u8; FOOTER_SIZE as usize];
        file.read_exact(&mut bytes)?;
        let footer = match HvbFooter::parse(&bytes) {
            Ok(footer) => footer,
            Err(_) => return Ok(None),
        };
        let cert_end = footer
            .cert_offset
            .checked_add(footer.cert_size)
            .context("HVB certificate range overflow")?;
        if footer.partition_size != length
            || cert_end > length - FOOTER_SIZE
            || footer.cert_size > 16 * 1024 * 1024
        {
            bail!("invalid HVB footer ranges in {}", path.display());
        }
        file.seek(SeekFrom::Start(footer.cert_offset))?;
        let mut certificate = vec![0_u8; footer.cert_size as usize];
        file.read_exact(&mut certificate)?;
        Ok(Some(Self {
            footer,
            certificate,
        }))
    }

    pub fn write_repacked(&self, raw_image: &Path, output: &Path) -> Result<()> {
        let raw_size = fs::metadata(raw_image)?.len();
        if raw_size > self.footer.cert_offset {
            bail!(
                "rebuilt EROFS image is {} bytes but only {} bytes are available before the HVB certificate",
                raw_size,
                self.footer.cert_offset
            );
        }
        let footer_end = self
            .footer
            .partition_size
            .checked_sub(FOOTER_SIZE)
            .context("HVB partition is smaller than its footer")?;
        let cert_end = self
            .footer
            .cert_offset
            .checked_add(self.certificate.len() as u64)
            .context("HVB certificate range overflow")?;
        if cert_end > footer_end {
            bail!("HVB certificate overlaps the footer")
        }

        let source = File::open(raw_image)?;
        let destination = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)
            .with_context(|| format!("creating {}", output.display()))?;
        destination.set_len(self.footer.partition_size)?;
        let mut source = BufReader::new(source);
        let mut destination = BufWriter::new(destination);
        let copied = io::copy(&mut source, &mut destination)?;
        if copied != raw_size {
            bail!("short read while copying rebuilt EROFS image")
        }
        destination.seek(SeekFrom::Start(self.footer.cert_offset))?;
        destination.write_all(&self.certificate)?;

        let mut footer = self.footer.clone();
        footer.image_size = raw_size;
        destination.seek(SeekFrom::Start(footer_end))?;
        destination.write_all(&footer.to_bytes())?;
        destination.flush()?;
        Ok(())
    }

    pub fn partition_name(&self) -> Option<&str> {
        if self.certificate.len() < 128 || &self.certificate[..4] != b"HVB\0" {
            return None;
        }
        let field = &self.certificate[64..128];
        let end = field
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(field.len());
        std::str::from_utf8(&field[..end])
            .ok()
            .filter(|name| !name.is_empty())
    }
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("fixed range"))
}
