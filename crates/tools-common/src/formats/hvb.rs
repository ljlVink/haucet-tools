use crate::bytes::{read_u32, read_u64};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const FOOTER_SIZE: usize = 104;
pub const FOOTER_MAGIC: &[u8; 8] = b"HVB\0\0\0\0\0";
pub const CERT_MAGIC: &[u8; 4] = b"HVB\0";
const CERT_MIN_LEN: usize = 240;
const CERT_NAME_OFFSET: usize = 64;
const CERT_NAME_END: usize = 128;
const MAX_CERT_SIZE: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HvbFooter {
    pub cert_offset: u64,
    pub cert_size: u64,
    pub image_size: u64,
    pub partition_size: u64,
}

impl HvbFooter {
    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() != FOOTER_SIZE || &bytes[..8] != FOOTER_MAGIC {
            return Err(invalid("not an HVB footer"));
        }
        Ok(Self {
            cert_offset: read_u64(bytes, 8)?,
            cert_size: read_u64(bytes, 16)?,
            image_size: read_u64(bytes, 24)?,
            partition_size: read_u64(bytes, 32)?,
        })
    }

    pub fn to_bytes(&self) -> [u8; FOOTER_SIZE] {
        let mut bytes = [0_u8; FOOTER_SIZE];
        bytes[..8].copy_from_slice(FOOTER_MAGIC);
        bytes[8..16].copy_from_slice(&self.cert_offset.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.cert_size.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.image_size.to_le_bytes());
        bytes[32..40].copy_from_slice(&self.partition_size.to_le_bytes());
        bytes
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HvbCert {
    pub version_major: u32,
    pub version_minor: u32,
    pub image_original_len: u64,
    pub image_len: u64,
    pub partition_name: String,
    pub verity_type: u32,
    pub hash_algo: u32,
    pub salt_offset: u64,
    pub salt_size: u64,
    pub digest_offset: u64,
    pub digest_size: u64,
    pub raw: Vec<u8>,
}

impl HvbCert {
    pub fn parse(buf: &[u8]) -> io::Result<Self> {
        if buf.len() < CERT_MIN_LEN {
            return Err(invalid("HVB certificate too small"));
        }
        if &buf[..4] != CERT_MAGIC {
            return Err(invalid("not an HVB certificate"));
        }
        Ok(Self {
            version_major: read_u32(buf, 4)?,
            version_minor: read_u32(buf, 8)?,
            image_original_len: read_u64(buf, 48)?,
            image_len: read_u64(buf, 56)?,
            partition_name: {
                let mut end = CERT_NAME_OFFSET;
                while end < CERT_NAME_END && buf[end] != 0 {
                    end += 1;
                }
                String::from_utf8_lossy(&buf[CERT_NAME_OFFSET..end]).into_owned()
            },
            verity_type: read_u32(buf, 144)?,
            hash_algo: read_u32(buf, 148)?,
            salt_offset: read_u64(buf, 152)?,
            salt_size: read_u64(buf, 160)?,
            digest_offset: read_u64(buf, 168)?,
            digest_size: read_u64(buf, 176)?,
            raw: buf.to_vec(),
        })
    }
}

fn cert_partition_name(buf: &[u8]) -> Option<&str> {
    if buf.len() < CERT_NAME_END || &buf[..4] != CERT_MAGIC {
        return None;
    }
    let field = &buf[CERT_NAME_OFFSET..CERT_NAME_END];
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    std::str::from_utf8(&field[..end])
        .ok()
        .filter(|name| !name.is_empty())
}

#[derive(Debug, Clone)]
pub struct HvbWrapper {
    pub footer: HvbFooter,
    pub certificate: Vec<u8>,
}

impl HvbWrapper {
    pub fn read_from(path: &Path) -> Result<Option<Self>> {
        let mut file =
            File::open(path).with_context(|| format!("opening image {}", path.display()))?;
        let length = file.metadata()?.len();
        if length < FOOTER_SIZE as u64 {
            return Ok(None);
        }
        file.seek(SeekFrom::End(-(FOOTER_SIZE as i64)))?;
        let mut bytes = [0_u8; FOOTER_SIZE];
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
            || cert_end > length - FOOTER_SIZE as u64
            || footer.cert_size > MAX_CERT_SIZE
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
            .checked_sub(FOOTER_SIZE as u64)
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
        cert_partition_name(&self.certificate)
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
