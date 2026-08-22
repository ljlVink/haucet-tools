use crate::formats::harmony::HvbFrame;
use crate::formats::hvb::{HvbCert, HvbWrapper};
use std::io;
use std::path::Path;

pub fn info(image: &Path) -> io::Result<()> {
    match HvbFrame::load(image) {
        Ok(frame) => {
            print_frame_summary(&frame);
            Ok(())
        }
        Err(e) if e.to_string().contains("not HARMONY! magic") => {
            let wrapper = HvbWrapper::read_from(image)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?
                .ok_or_else(|| invalid("not a HARMONY!/HVB partition image"))?;
            print_wrapper_summary(&wrapper);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

pub fn print_frame_summary(frame: &HvbFrame) {
    eprintln!("--- HARMONY! header ---");
    eprintln!("  hdr_size     = 0x{:X}", frame.harmony.hdr_size);
    eprintln!("  image_size   = 0x{:X}", frame.harmony.image_size);
    eprintln!("  flags        = 0x{:X}", frame.harmony.flags);
    eprintln!("  buildvariant = {:?}", frame.harmony.buildvariant);
    eprintln!("--- HVB footer ---");
    eprintln!("  cert_offset    = 0x{:X}", frame.footer.cert_offset);
    eprintln!("  cert_size      = {}", frame.footer.cert_size);
    eprintln!("  image_size     = 0x{:X}", frame.footer.image_size);
    eprintln!("  partition_size = 0x{:X}", frame.footer.partition_size);
    eprintln!("--- HVB cert ---");
    print_cert_summary(&frame.cert);
}

fn print_wrapper_summary(wrapper: &HvbWrapper) {
    eprintln!("--- HVB footer ---");
    eprintln!("  cert_offset    = 0x{:X}", wrapper.footer.cert_offset);
    eprintln!("  cert_size      = {}", wrapper.footer.cert_size);
    eprintln!("  image_size     = 0x{:X}", wrapper.footer.image_size);
    eprintln!("  partition_size = 0x{:X}", wrapper.footer.partition_size);
    eprintln!("--- HVB cert ---");
    match HvbCert::parse(&wrapper.certificate) {
        Ok(cert) => print_cert_summary(&cert),
        Err(e) => eprintln!("  (parse failed: {e})"),
    }
}

fn print_cert_summary(cert: &HvbCert) {
    eprintln!(
        "  version           = {}.{}",
        cert.version_major, cert.version_minor
    );
    eprintln!("  partition_name    = {:?}", cert.partition_name);
    eprintln!(
        "  image_original_len= 0x{:X}",
        cert.image_original_len
    );
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
