use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub use super::hvb::{
    CERT_MAGIC as HVB_CERT_MAGIC, FOOTER_MAGIC as HVB_FOOTER_MAGIC, FOOTER_SIZE as HVB_FOOTER_SIZE,
    HvbCert, HvbFooter,
};

pub const HARMONY_MAGIC: &[u8; 8] = b"HARMONY!";

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
        let buildvariant =
            String::from_utf8_lossy(raw.get(0x40..bv_end).unwrap_or(&[])).into_owned();
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
        out[footer_pos..footer_pos + HVB_FOOTER_SIZE].copy_from_slice(&self.footer.to_bytes());
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
