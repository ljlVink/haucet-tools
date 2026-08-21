//! HARMONY ramdisk image framing and HVB metadata handling.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub const HARMONY_MAGIC: &[u8; 8] = b"HARMONY!";
pub const HVB_FOOTER_MAGIC: &[u8; 8] = b"HVB\0\0\0\0\0";
pub const HVB_CERT_MAGIC: &[u8; 4] = b"HVB\0";
pub const HVB_FOOTER_SIZE: usize = 104;

#[derive(Debug, Clone)]
pub struct HarmonyHeader {
    pub hdr_size: u32,
    pub image_size: u32,
    pub flags: u32,
    pub buildvariant: String,
    pub raw: Vec<u8>,
}

impl HarmonyHeader {
    pub fn parse(raw: &[u8]) -> io::Result<Self> {
        if raw.len() < 8 || &raw[0..8] != HARMONY_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not HARMONY! magic",
            ));
        }
        if raw.len() < 0x20 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HARMONY! header too small",
            ));
        }
        let le_u32 =
            |off: usize| u32::from_le_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]]);
        let be_u64 = |off: usize| {
            u64::from_be_bytes([
                raw[off],
                raw[off + 1],
                raw[off + 2],
                raw[off + 3],
                raw[off + 4],
                raw[off + 5],
                raw[off + 6],
                raw[off + 7],
            ])
        };
        let hdr_size = be_u64(0x08) as u32;
        let image_size = le_u32(0x10);
        let flags = (be_u64(0x18) & 0xFFFFFFFF) as u32;
        let mut bv_end = 0x40;
        while bv_end < raw.len() && raw[bv_end] != 0 {
            bv_end += 1;
        }
        let buildvariant = String::from_utf8_lossy(&raw[0x40..bv_end]).into_owned();
        Ok(Self {
            hdr_size,
            image_size,
            flags,
            buildvariant,
            raw: raw.to_vec(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct HvbFooter {
    pub cert_offset: u64,
    pub cert_size: u64,
    pub image_size: u64,
    pub partition_size: u64,
}

impl HvbFooter {
    pub fn parse(buf: &[u8]) -> io::Result<Self> {
        if buf.len() < HVB_FOOTER_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "footer too small",
            ));
        }
        if &buf[0..8] != HVB_FOOTER_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not HVB footer magic",
            ));
        }
        let le = |off| u64::from_le_bytes(buf[off..off + 8].try_into().unwrap());
        Ok(Self {
            cert_offset: le(0x08),
            cert_size: le(0x10),
            image_size: le(0x18),
            partition_size: le(0x20),
        })
    }

    pub fn serialize(&self) -> [u8; HVB_FOOTER_SIZE] {
        let mut buf = [0u8; HVB_FOOTER_SIZE];
        buf[0..8].copy_from_slice(HVB_FOOTER_MAGIC);
        buf[0x08..0x10].copy_from_slice(&self.cert_offset.to_le_bytes());
        buf[0x10..0x18].copy_from_slice(&self.cert_size.to_le_bytes());
        buf[0x18..0x20].copy_from_slice(&self.image_size.to_le_bytes());
        buf[0x20..0x28].copy_from_slice(&self.partition_size.to_le_bytes());
        buf
    }
}

#[derive(Debug, Clone)]
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
        if buf.len() < 240 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "cert too small"));
        }
        if &buf[0..4] != HVB_CERT_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not HVB cert magic",
            ));
        }
        let le = |off| u64::from_le_bytes(buf[off..off + 8].try_into().unwrap());
        let le32 = |off| u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]);
        let partition_name = {
            let mut end = 64;
            while end < 128 && buf[end] != 0 {
                end += 1;
            }
            String::from_utf8_lossy(&buf[64..end]).into_owned()
        };
        Ok(Self {
            version_major: le32(4),
            version_minor: le32(8),
            image_original_len: le(48),
            image_len: le(56),
            partition_name,
            verity_type: le32(144),
            hash_algo: le32(148),
            salt_offset: le(152),
            salt_size: le(160),
            digest_offset: le(168),
            digest_size: le(176),
            raw: buf.to_vec(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct HvbFrame {
    pub harmony: HarmonyHeader,
    pub footer: HvbFooter,
    pub cert: HvbCert,
    pub image: Vec<u8>,
}

impl HvbFrame {
    pub fn load<P: AsRef<Path>>(p: P) -> io::Result<Self> {
        let data = fs::read(p.as_ref())?;
        Self::from_bytes(&data)
    }

    pub fn from_bytes(data: &[u8]) -> io::Result<Self> {
        if data.len() < HVB_FOOTER_SIZE + 8 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "file too small"));
        }
        let harmony_hdr_len = 0x800.min(data.len());
        let harmony = HarmonyHeader::parse(&data[..harmony_hdr_len])?;
        let hdr_size = harmony.hdr_size as usize;
        if hdr_size > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "hdr_size > file",
            ));
        }
        let footer = match HvbFooter::parse(&data[data.len() - HVB_FOOTER_SIZE..]) {
            Ok(f) => f,
            Err(e) => {
                eprintln!(
                    "WARN: HVB footer parse failed ({e:?}); writing unsigned repack is unsupported"
                );
                return Err(e);
            }
        };
        let cert_end = (footer.cert_offset + footer.cert_size) as usize;
        let cert = if cert_end <= data.len() && footer.cert_size >= 1 {
            HvbCert::parse(&data[footer.cert_offset as usize..cert_end])?
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "cert range invalid",
            ));
        };

        let image_end = (footer.image_size as usize).min(data.len());
        let image = data[..image_end].to_vec();

        Ok(Self {
            harmony,
            footer,
            cert,
            image,
        })
    }

    pub fn extract_image_payload(&self) -> &[u8] {
        let start = self.harmony.hdr_size as usize;
        let end = self.footer.image_size as usize;
        if start > end || end > self.image.len() {
            return &[];
        }
        &self.image[start..end]
    }

    pub fn rebuild(&mut self, new_payload: &[u8]) {
        let hdr_size = self.harmony.hdr_size as usize;
        let mut new_image = Vec::with_capacity(hdr_size + new_payload.len() + 0x2000);
        new_image.extend_from_slice(&self.harmony.raw[..hdr_size.min(self.harmony.raw.len())]);
        new_image.extend_from_slice(new_payload);
        let new_image_size = new_image.len() as u64;
        self.footer.image_size = new_image_size;
        let new_cert_offset = align_up(new_image_size, 0x1000);
        self.footer.cert_offset = new_cert_offset;
        self.footer.cert_size = self.cert.raw.len() as u64;
        self.image = new_image;
        let cert_end = new_cert_offset + self.footer.cert_size;
        let needed = cert_end + HVB_FOOTER_SIZE as u64;
        if self.footer.partition_size < needed {
            self.footer.partition_size = align_up(needed, 0x1000);
        }
    }

    pub fn serialize(&self) -> io::Result<Vec<u8>> {
        let total = self.footer.partition_size as usize;
        let cert_end = (self.footer.cert_offset + self.footer.cert_size) as usize;
        let footer_pos = total.saturating_sub(HVB_FOOTER_SIZE);
        if cert_end > footer_pos {
            return Err(io::Error::other(
                "cert overlaps footer (partition_size too small)",
            ));
        }
        let mut out = vec![0u8; total];
        let img_len = (self.footer.image_size as usize).min(self.image.len());
        out[..img_len].copy_from_slice(&self.image[..img_len]);
        let cert_off = self.footer.cert_offset as usize;
        let cert_len = self.cert.raw.len();
        out[cert_off..cert_off + cert_len].copy_from_slice(&self.cert.raw);
        out[footer_pos..footer_pos + HVB_FOOTER_SIZE].copy_from_slice(&self.footer.serialize());
        Ok(out)
    }

    pub fn write<P: AsRef<Path>>(&self, p: P) -> io::Result<()> {
        let bytes = self.serialize()?;
        let mut f = fs::File::create(p)?;
        f.write_all(&bytes)?;
        Ok(())
    }
}

fn align_up(x: u64, align: u64) -> u64 {
    if align == 0 {
        x
    } else {
        (x + align - 1) & !(align - 1)
    }
}
