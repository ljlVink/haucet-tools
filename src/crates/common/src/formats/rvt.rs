use super::harmony::{HVB_CERT_MAGIC, HVB_FOOTER_MAGIC, HVB_FOOTER_SIZE, HvbCert, HvbFooter};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;

const RVT_MAGIC: &[u8; 4] = b"rot\0";
const RVT_HEADER_SIZE: usize = 72;
const DESCRIPTOR_SIZE: usize = 80;
const RVT_MAX_SIZE: usize = 64 * 1024;

struct Descriptor<'a> {
    name: String,
    pubkey_offset: u64,
    pubkey_len: usize,
    pubkey: &'a [u8],
    backup: Option<&'a [u8]>,
}

pub fn parse_file(path: &str) -> io::Result<()> {
    let data = fs::read(path)?;
    eprintln!("file: {path}  size: {} ({:#x})", data.len(), data.len());

    let (rvt_data, expected_size) = if has_footer(&data) {
        eprintln!("detected: HVB-wrapped RVT image");
        let footer_pos = data.len() - HVB_FOOTER_SIZE;
        let footer = HvbFooter::parse(&data[footer_pos..])?;
        print_footer(&footer);

        let cert_start = usize::try_from(footer.cert_offset)
            .map_err(|_| invalid("HVB cert offset does not fit usize"))?;
        let cert_size = usize::try_from(footer.cert_size)
            .map_err(|_| invalid("HVB cert size does not fit usize"))?;
        let cert_end = cert_start
            .checked_add(cert_size)
            .ok_or_else(|| invalid("HVB cert range overflow"))?;
        if cert_end > data.len() {
            return Err(invalid("HVB cert range is outside the image"));
        }
        print_cert(cert_start, &HvbCert::parse(&data[cert_start..cert_end])?);

        let image_size = usize::try_from(footer.image_size)
            .map_err(|_| invalid("HVB image size does not fit usize"))?;
        if image_size > data.len() {
            return Err(invalid("HVB image segment is outside the file"));
        }
        (&data[..image_size], image_size)
    } else {
        eprintln!("detected: raw RVT image or partition dump");
        (&data[..], data.len())
    };

    let (descriptors, verity_num, raw_key_count, total_size) = parse_rvt(rvt_data)?;
    println!("=== RVT image ===");
    println!("magic                : rot\\0");
    println!("verity_num           : {verity_num}");
    if raw_key_count == 0 {
        println!("pubkey_num_per_ptn   : 0 (old-version value, treated as 1)");
    } else {
        println!("pubkey_num_per_ptn   : {raw_key_count}");
    }
    println!("chain descriptors    : {}", descriptors.len());
    println!("total raw size       : {total_size} bytes ({total_size:#x})");

    for (index, descriptor) in descriptors.iter().enumerate() {
        println!("\n--- descriptor #{index} ---");
        println!("  partition name       : {:?}", descriptor.name);
        println!("  pubkey offset        : {:#x}", descriptor.pubkey_offset);
        println!("  pubkey length        : {} bytes", descriptor.pubkey_len);
        println!(
            "  detected algorithm   : {}",
            algorithm(descriptor.pubkey_len)
        );
        println!("  pubkey SHA256        : {}", sha256_hex(descriptor.pubkey));
        println!(
            "  pubkey hex (first 32B): {}",
            hex(&descriptor.pubkey[..descriptor.pubkey.len().min(32)])
        );
        if let Some(backup) = descriptor.backup {
            println!("  pubkey backup SHA256 : {}", sha256_hex(backup));
            println!(
                "  pubkey backup (32B)  : {}",
                hex(&backup[..backup.len().min(32)])
            );
            println!("  backup == main       : {}", backup == descriptor.pubkey);
        }
    }

    println!("\n=== sanity checks ===");
    println!("  expected image size  = {expected_size:#x}");
    println!("  rvt.total_size       = {total_size:#x}");
    if total_size > expected_size {
        println!("  WARN: RVT content exceeds the image segment");
    } else {
        println!("  OK: RVT content fits in the image segment");
    }

    if !has_footer(&data)
        && let Some(cert_offset) = find_embedded_cert(&data, total_size)
    {
        println!("\n=== embedded HVB certificate ===");
        let cert = HvbCert::parse(&data[cert_offset..])?;
        print_cert(cert_offset, &cert);
    }

    Ok(())
}

fn parse_rvt(data: &[u8]) -> io::Result<(Vec<Descriptor<'_>>, u32, u32, usize)> {
    if data.len() < RVT_HEADER_SIZE {
        return Err(invalid("file is too small for an RVT header"));
    }
    if &data[..4] != RVT_MAGIC {
        return Err(invalid("RVT magic rot\\0 not found"));
    }
    if data.len() > RVT_MAX_SIZE {
        eprintln!(
            "WARN: file/image segment exceeds the 64 KiB RVT limit; parsing only the header-defined content"
        );
    }

    let verity_num = le_u32(data, 4)?;
    if verity_num >= 32 {
        return Err(invalid("RVT verity_num must be less than 32"));
    }
    let raw_key_count = le_u32(data, 8)?;
    let key_count = if raw_key_count == 0 {
        1
    } else {
        raw_key_count as usize
    };
    if key_count != 1 && key_count != 2 {
        return Err(invalid("RVT pubkey_num_per_ptn must be 0, 1, or 2"));
    }
    if data[12..RVT_HEADER_SIZE].iter().any(|byte| *byte != 0) {
        eprintln!("WARN: RVT reserved header bytes are not all zero");
    }

    let mut offset = RVT_HEADER_SIZE;
    let mut descriptors = Vec::with_capacity(verity_num as usize);
    for index in 0..verity_num {
        let header_end = offset
            .checked_add(DESCRIPTOR_SIZE)
            .ok_or_else(|| invalid("RVT descriptor offset overflow"))?;
        if header_end > data.len() {
            return Err(invalid(format!(
                "RVT descriptor #{index} header is outside the image"
            )));
        }

        let name_raw = &data[offset..offset + 64];
        let name_end = name_raw
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(name_raw.len());
        if name_end < name_raw.len() && name_raw[name_end + 1..].iter().any(|byte| *byte != 0) {
            eprintln!("WARN: descriptor #{index} name padding is not all zero");
        }
        let name = String::from_utf8_lossy(&name_raw[..name_end]).into_owned();
        let pubkey_offset = le_u64(data, offset + 64)?;
        let pubkey_len = usize::try_from(le_u64(data, offset + 72)?)
            .map_err(|_| invalid(format!("descriptor #{index} public key is too large")))?;
        let payload_size = pubkey_len
            .checked_mul(key_count)
            .ok_or_else(|| invalid(format!("descriptor #{index} public key size overflow")))?;
        let entry_end = header_end
            .checked_add(payload_size)
            .ok_or_else(|| invalid(format!("descriptor #{index} range overflow")))?;
        if entry_end > data.len() {
            return Err(invalid(format!(
                "descriptor #{index} public key is outside the image"
            )));
        }

        let pubkey = &data[header_end..header_end + pubkey_len];
        let backup = (key_count == 2).then(|| &data[header_end + pubkey_len..entry_end]);
        descriptors.push(Descriptor {
            name,
            pubkey_offset,
            pubkey_len,
            pubkey,
            backup,
        });
        offset = entry_end;
    }
    Ok((descriptors, verity_num, raw_key_count, offset))
}

fn has_footer(data: &[u8]) -> bool {
    data.len() >= HVB_FOOTER_SIZE
        && &data[data.len() - HVB_FOOTER_SIZE..data.len() - HVB_FOOTER_SIZE + 8] == HVB_FOOTER_MAGIC
}

fn find_embedded_cert(data: &[u8], after: usize) -> Option<usize> {
    let end = data.len().min(after.saturating_add(RVT_MAX_SIZE * 8));
    let mut offset = after;
    while offset.checked_add(12)? <= end {
        let relative = data[offset..end]
            .windows(HVB_CERT_MAGIC.len())
            .position(|window| window == HVB_CERT_MAGIC)?;
        let candidate = offset + relative;
        if candidate + 12 <= data.len()
            && le_u32(data, candidate + 4).ok() == Some(1)
            && matches!(le_u32(data, candidate + 8).ok(), Some(0 | 1))
        {
            return Some(candidate);
        }
        offset = candidate + HVB_CERT_MAGIC.len();
    }
    None
}

fn print_footer(footer: &HvbFooter) {
    println!("=== HVB footer ===");
    println!("  cert_offset     : {:#x}", footer.cert_offset);
    println!(
        "  cert_size       : {:#x} ({})",
        footer.cert_size, footer.cert_size
    );
    println!(
        "  image_size      : {:#x} ({})",
        footer.image_size, footer.image_size
    );
    println!(
        "  partition_size  : {:#x} ({})",
        footer.partition_size, footer.partition_size
    );
}

fn print_cert(offset: usize, cert: &HvbCert) {
    println!("  cert offset       : {offset:#x}");
    println!(
        "  version           : {}.{}",
        cert.version_major, cert.version_minor
    );
    println!("  partition_name    : {:?}", cert.partition_name);
    println!("  image_original_len: {:#x}", cert.image_original_len);
    println!("  image_len         : {:#x}", cert.image_len);
    println!("  verity_type       : {}", cert.verity_type);
    println!("  hash_algo         : {}", cert.hash_algo);
    println!(
        "  salt_offset/size  : {:#x} / {}",
        cert.salt_offset, cert.salt_size
    );
    println!(
        "  digest_offset/size: {:#x} / {}",
        cert.digest_offset, cert.digest_size
    );
}

fn algorithm(pubkey_len: usize) -> &'static str {
    match pubkey_len {
        528 => "SHA256_RSA2048",
        784 => "SHA256_RSA3072",
        1040 => "SHA256_RSA4096",
        64 => "SM2_SM3",
        _ => "UNKNOWN",
    }
}

fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hex(data: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(data.len() * 2);
    for byte in data {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn le_u32(data: &[u8], offset: usize) -> io::Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| invalid("unexpected end of RVT data"))?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn le_u64(data: &[u8], offset: usize) -> io::Result<u64> {
    let bytes = data
        .get(offset..offset + 8)
        .ok_or_else(|| invalid("unexpected end of RVT data"))?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
