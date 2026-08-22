use crate::formats::harmony::HvbFrame;
use crate::formats::hvb::{HvbCert, HvbFooter, HvbWrapper};
use crate::formats::rvt;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

const RVT_MAGIC: &[u8; 4] = b"rot\0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertSummary {
    pub version_major: u32,
    pub version_minor: u32,
    pub partition_name: String,
    pub image_original_len: u64,
    pub image_len: u64,
    pub verity_type: u32,
    pub hash_algo: u32,
    pub salt_size: u64,
    pub digest_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarmonySummary {
    pub hdr_size: u32,
    pub image_size: u32,
    pub flags: u32,
    pub buildvariant: String,
    pub footer: HvbFooter,
    pub cert: CertSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PartitionSummary {
    Harmony(HarmonySummary),
    Rvt(rvt::RvtInfo),
    HvbWrapped {
        footer: HvbFooter,
        cert: Option<CertSummary>,
        cert_error: Option<String>,
    },
}

pub fn info(image: &Path) -> io::Result<()> {
    match summarize(image)? {
        PartitionSummary::Harmony(summary) => {
            print_harmony_summary(&summary);
            Ok(())
        }
        PartitionSummary::Rvt(_) => {
            let path = image
                .to_str()
                .ok_or_else(|| invalid("path is not valid UTF-8"))?;
            rvt::parse_file(path)
        }
        PartitionSummary::HvbWrapped {
            footer,
            cert,
            cert_error,
        } => {
            print_wrapper_summary(&footer, cert.as_ref(), cert_error.as_deref());
            Ok(())
        }
    }
}

pub fn summarize(image: &Path) -> io::Result<PartitionSummary> {
    match HvbFrame::load(image) {
        Ok(frame) => {
            return Ok(PartitionSummary::Harmony(HarmonySummary {
                hdr_size: frame.harmony.hdr_size,
                image_size: frame.harmony.image_size,
                flags: frame.harmony.flags,
                buildvariant: frame.harmony.buildvariant.clone(),
                footer: frame.footer.clone(),
                cert: summarize_cert(&frame.cert),
            }));
        }
        Err(e) if e.to_string().contains("not HARMONY! magic") => {}
        Err(e) => return Err(e),
    }
    if starts_with_magic(image, RVT_MAGIC)? {
        return Ok(PartitionSummary::Rvt(rvt::parse_image(image)?));
    }
    if let Some(wrapper) = HvbWrapper::read_from(image)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?
    {
        let (cert, cert_error) = match HvbCert::parse(&wrapper.certificate) {
            Ok(cert) => (Some(summarize_cert(&cert)), None),
            Err(e) => (None, Some(e.to_string())),
        };
        return Ok(PartitionSummary::HvbWrapped {
            footer: wrapper.footer,
            cert,
            cert_error,
        });
    }
    Err(invalid("not a HARMONY!/HVB/RVT partition image"))
}

fn starts_with_magic(path: &Path, magic: &[u8; 4]) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let mut bytes = [0_u8; 4];
    let read = file.read(&mut bytes)?;
    Ok(read == magic.len() && &bytes == magic)
}

fn summarize_cert(cert: &HvbCert) -> CertSummary {
    CertSummary {
        version_major: cert.version_major,
        version_minor: cert.version_minor,
        partition_name: cert.partition_name.clone(),
        image_original_len: cert.image_original_len,
        image_len: cert.image_len,
        verity_type: cert.verity_type,
        hash_algo: cert.hash_algo,
        salt_size: cert.salt_size,
        digest_size: cert.digest_size,
    }
}

pub fn print_frame_summary(frame: &HvbFrame) {
    print_harmony_summary(&HarmonySummary {
        hdr_size: frame.harmony.hdr_size,
        image_size: frame.harmony.image_size,
        flags: frame.harmony.flags,
        buildvariant: frame.harmony.buildvariant.clone(),
        footer: frame.footer.clone(),
        cert: summarize_cert(&frame.cert),
    });
}

fn print_harmony_summary(summary: &HarmonySummary) {
    eprintln!("--- HARMONY! header ---");
    eprintln!("  hdr_size     = 0x{:X}", summary.hdr_size);
    eprintln!("  image_size   = 0x{:X}", summary.image_size);
    eprintln!("  flags        = 0x{:X}", summary.flags);
    eprintln!("  buildvariant = {:?}", summary.buildvariant);
    eprintln!("--- HVB footer ---");
    eprintln!("  cert_offset    = 0x{:X}", summary.footer.cert_offset);
    eprintln!("  cert_size      = {}", summary.footer.cert_size);
    eprintln!("  image_size     = 0x{:X}", summary.footer.image_size);
    eprintln!("  partition_size = 0x{:X}", summary.footer.partition_size);
    eprintln!("--- HVB cert ---");
    print_cert_summary(&summary.cert);
}

fn print_wrapper_summary(footer: &HvbFooter, cert: Option<&CertSummary>, cert_error: Option<&str>) {
    eprintln!("--- HVB footer ---");
    eprintln!("  cert_offset    = 0x{:X}", footer.cert_offset);
    eprintln!("  cert_size      = {}", footer.cert_size);
    eprintln!("  image_size     = 0x{:X}", footer.image_size);
    eprintln!("  partition_size = 0x{:X}", footer.partition_size);
    eprintln!("--- HVB cert ---");
    match cert {
        Some(cert) => print_cert_summary(cert),
        None => eprintln!("  (parse failed: {})", cert_error.unwrap_or("unknown")),
    }
}

fn print_cert_summary(cert: &CertSummary) {
    eprintln!(
        "  version           = {}.{}",
        cert.version_major, cert.version_minor
    );
    eprintln!("  partition_name    = {:?}", cert.partition_name);
    eprintln!("  image_original_len= 0x{:X}", cert.image_original_len);
    eprintln!("  image_len         = 0x{:X}", cert.image_len);
    eprintln!(
        "  verity_type       = {} ({})",
        cert.verity_type,
        match cert.verity_type {
            1 => "hash",
            2 => "hashtree",
            _ => "?",
        }
    );
    eprintln!(
        "  hash_algo         = {} ({})",
        cert.hash_algo,
        match cert.hash_algo {
            0 => "SHA256",
            1 => "SHA128",
            2 => "SHA512",
            3 => "SM3",
            _ => "?",
        }
    );
    eprintln!("  salt_size         = {}", cert.salt_size);
    eprintln!("  digest_size       = {}", cert.digest_size);
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
