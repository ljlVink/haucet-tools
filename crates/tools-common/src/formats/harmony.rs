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
        let hdr_size = u32::try_from(be_u64(0x08)).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "HARMONY header size is too large",
            )
        })?;
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
        let mut harmony = HarmonyHeader::parse(&data[..harmony_hdr_len])?;
        let hdr_size = harmony.hdr_size as usize;
        if hdr_size < 0x20 || hdr_size > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HARMONY header size is outside the file",
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
        if footer.partition_size != data.len() as u64 {
            return Err(invalid("HVB partition size does not match the file length"));
        }
        let cert_offset = usize::try_from(footer.cert_offset)
            .map_err(|_| invalid("HVB certificate offset does not fit in memory"))?;
        let cert_size = usize::try_from(footer.cert_size)
            .map_err(|_| invalid("HVB certificate size does not fit in memory"))?;
        let cert_end = cert_offset
            .checked_add(cert_size)
            .ok_or_else(|| invalid("HVB certificate range overflow"))?;
        let footer_offset = data.len() - HVB_FOOTER_SIZE;
        if cert_size == 0 || cert_end > footer_offset {
            return Err(invalid("HVB certificate range is invalid"));
        }
        let cert = HvbCert::parse(&data[cert_offset..cert_end])?;

        let image_end = usize::try_from(footer.image_size)
            .map_err(|_| invalid("HVB image size does not fit in memory"))?;
        let payload_end = hdr_size
            .checked_add(harmony.image_size as usize)
            .ok_or_else(|| invalid("HARMONY payload range overflow"))?;
        if image_end > cert_offset || hdr_size > image_end || payload_end > image_end {
            return Err(invalid("HVB image range is invalid"));
        }
        harmony.raw = data[..hdr_size].to_vec();
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
        let Some(end) = start.checked_add(self.harmony.image_size as usize) else {
            return &[];
        };
        if start > end || end > self.image.len() {
            return &[];
        }
        &self.image[start..end]
    }

    pub fn rebuild(&mut self, new_payload: &[u8]) -> io::Result<()> {
        let hdr_size = self.harmony.hdr_size as usize;
        let header = self
            .harmony
            .raw
            .get(..hdr_size)
            .ok_or_else(|| invalid("HARMONY header bytes are truncated"))?;
        let capacity = hdr_size
            .checked_add(new_payload.len())
            .and_then(|size| size.checked_add(0x2000))
            .ok_or_else(|| invalid("rebuilt HARMONY image size overflow"))?;
        let mut new_image = Vec::new();
        new_image
            .try_reserve(capacity)
            .map_err(|_| invalid("rebuilt HARMONY image is too large"))?;
        new_image.extend_from_slice(header);
        new_image.extend_from_slice(new_payload);
        let payload_size = u32::try_from(new_payload.len())
            .map_err(|_| invalid("HARMONY payload exceeds the 4 GiB header limit"))?;
        self.harmony.image_size = payload_size;
        self.harmony.raw[0x10..0x14].copy_from_slice(&payload_size.to_le_bytes());
        new_image[0x10..0x14].copy_from_slice(&payload_size.to_le_bytes());

        let new_image_size = align_up(new_image.len() as u64, 0x800)
            .ok_or_else(|| invalid("HARMONY image alignment overflow"))?;
        let aligned_len = usize::try_from(new_image_size)
            .map_err(|_| invalid("HARMONY image size does not fit in memory"))?;
        new_image.resize(aligned_len, 0);
        self.footer.image_size = new_image_size;
        let original_cert_offset = self.footer.cert_offset;
        let new_cert_offset = if new_image_size <= original_cert_offset {
            original_cert_offset
        } else {
            align_up(new_image_size, 0x1000)
                .ok_or_else(|| invalid("HVB certificate offset overflow"))?
        };
        self.footer.cert_offset = new_cert_offset;
        self.footer.cert_size = self.cert.raw.len() as u64;
        self.image = new_image;
        let cert_end = new_cert_offset
            .checked_add(self.footer.cert_size)
            .ok_or_else(|| invalid("HVB certificate range overflow"))?;
        let needed = cert_end
            .checked_add(HVB_FOOTER_SIZE as u64)
            .ok_or_else(|| invalid("HVB partition size overflow"))?;
        if self.footer.partition_size < needed {
            self.footer.partition_size = align_up(needed, 0x1000)
                .ok_or_else(|| invalid("HVB partition alignment overflow"))?;
        }
        Ok(())
    }

    pub fn serialize(&self) -> io::Result<Vec<u8>> {
        if self.footer.cert_size != self.cert.raw.len() as u64 {
            return Err(invalid("HVB certificate size does not match its data"));
        }
        let total = usize::try_from(self.footer.partition_size)
            .map_err(|_| invalid("HVB partition size does not fit in memory"))?;
        let footer_pos = total
            .checked_sub(HVB_FOOTER_SIZE)
            .ok_or_else(|| invalid("HVB partition is smaller than its footer"))?;
        let cert_off = usize::try_from(self.footer.cert_offset)
            .map_err(|_| invalid("HVB certificate offset does not fit in memory"))?;
        let cert_end = cert_off
            .checked_add(self.cert.raw.len())
            .ok_or_else(|| invalid("HVB certificate range overflow"))?;
        if cert_end > footer_pos {
            return Err(io::Error::other(
                "cert overlaps footer (partition_size too small)",
            ));
        }
        let image_size = usize::try_from(self.footer.image_size)
            .map_err(|_| invalid("HVB image size does not fit in memory"))?;
        if image_size > self.image.len() || image_size > cert_off {
            return Err(invalid("HVB image range is invalid"));
        }
        let mut out = vec![0u8; total];
        out[..image_size].copy_from_slice(&self.image[..image_size]);
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

fn align_up(x: u64, align: u64) -> Option<u64> {
    if align == 0 {
        Some(x)
    } else {
        let remainder = x % align;
        if remainder == 0 {
            Some(x)
        } else {
            x.checked_add(align - remainder)
        }
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
