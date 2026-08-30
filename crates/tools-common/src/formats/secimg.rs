use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use x509_parser::certificate::X509Certificate;
use x509_parser::extensions::X509Extension;
use x509_parser::parse_x509_certificate;

pub const HEADER_SIZE: usize = 0x2000;
pub const CERTIFICATE_COUNT: usize = 3;

pub const IMAGE_NAME_OID: &str = "2.20.2.8";
pub const PARTITION_NAME_OID: &str = "2.20.2.14";
pub const PAYLOAD_SHA256_OID: &str = "2.20.2.65";
pub const PAYLOAD_SIZE_OID: &str = "2.20.2.67";
pub const SECONDARY_SIZE_OID: &str = "2.20.2.69";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SecImageExtensionValue {
    Integer { value: u64 },
    Octets { hex: String, text: Option<String> },
    Der { hex: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecImageExtension {
    pub oid: String,
    pub critical: bool,
    pub value: SecImageExtensionValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecImageCertificate {
    pub chain_index: usize,
    pub offset: u64,
    pub size: u64,
    pub subject: String,
    pub issuer: String,
    pub serial_hex: String,
    pub not_before: String,
    pub not_after: String,
    pub signature_algorithm_oid: String,
    pub proprietary_extensions: Vec<SecImageExtension>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecImageInfo {
    pub image_name: String,
    pub partition_name: String,
    pub file_size: u64,
    pub header_size: u64,
    pub certificate_chain_size: u64,
    pub header_padding_size: u64,
    pub payload_offset: u64,
    pub payload_size: u64,
    pub secondary_size: Option<u64>,
    pub trailing_size: u64,
    pub declared_payload_sha256: String,
    pub actual_payload_sha256: String,
    pub payload_hash_valid: bool,
    pub certificates: Vec<SecImageCertificate>,
    pub warnings: Vec<String>,
}

impl SecImageInfo {
    pub fn from_bytes(data: &[u8]) -> io::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(invalid(
                "file is smaller than the 0x2000 secure-image header",
            ));
        }

        let (certificates, chain_end) = parse_certificate_chain(data)?;
        let metadata = parse_leaf_metadata(&certificates)?;
        let payload_size = usize::try_from(metadata.payload_size)
            .map_err(|_| invalid("secure-image payload size does not fit in memory"))?;
        let payload_end = HEADER_SIZE
            .checked_add(payload_size)
            .ok_or_else(|| invalid("secure-image payload range overflows"))?;
        if payload_end > data.len() {
            return Err(invalid("secure-image payload extends beyond the file"));
        }

        let actual_hash = Sha256::digest(&data[HEADER_SIZE..payload_end]);
        let actual_payload_sha256 = hex::encode(actual_hash);
        let declared_payload_sha256 = hex::encode(metadata.payload_sha256);
        let payload_hash_valid = actual_payload_sha256 == declared_payload_sha256;

        let mut warnings = Vec::new();
        let nonzero_padding = data[chain_end..HEADER_SIZE]
            .iter()
            .filter(|byte| **byte != 0)
            .count();
        if nonzero_padding != 0 {
            warnings.push(format!(
                "{nonzero_padding} non-zero byte(s) occur between the certificate chain and payload"
            ));
        }
        if !payload_hash_valid {
            warnings
                .push("payload SHA-256 does not match leaf certificate OID 2.20.2.65".to_owned());
        }
        if metadata
            .secondary_size
            .is_some_and(|size| size != metadata.payload_size)
        {
            warnings.push(
                "leaf certificate OIDs 2.20.2.67 and 2.20.2.69 contain different sizes".to_owned(),
            );
        }

        Ok(Self {
            image_name: metadata.image_name,
            partition_name: metadata.partition_name,
            file_size: data.len() as u64,
            header_size: HEADER_SIZE as u64,
            certificate_chain_size: chain_end as u64,
            header_padding_size: (HEADER_SIZE - chain_end) as u64,
            payload_offset: HEADER_SIZE as u64,
            payload_size: metadata.payload_size,
            secondary_size: metadata.secondary_size,
            trailing_size: (data.len() - payload_end) as u64,
            declared_payload_sha256,
            actual_payload_sha256,
            payload_hash_valid,
            certificates,
            warnings,
        })
    }

    pub fn payload<'a>(&self, data: &'a [u8]) -> io::Result<&'a [u8]> {
        if data.len() as u64 != self.file_size {
            return Err(invalid(
                "data length does not match the parsed secure image",
            ));
        }
        let start = usize::try_from(self.payload_offset)
            .map_err(|_| invalid("secure-image payload offset does not fit in memory"))?;
        let size = usize::try_from(self.payload_size)
            .map_err(|_| invalid("secure-image payload size does not fit in memory"))?;
        let end = start
            .checked_add(size)
            .ok_or_else(|| invalid("secure-image payload range overflows"))?;
        data.get(start..end)
            .ok_or_else(|| invalid("secure-image payload extends beyond the data"))
    }
}

pub fn parse_image(path: &Path) -> io::Result<SecImageInfo> {
    SecImageInfo::from_bytes(&fs::read(path)?)
}

pub fn probe(data: &[u8]) -> bool {
    probe_metadata(data).is_ok()
}

pub fn probe_image(path: &Path) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    if length < HEADER_SIZE as u64 {
        return Ok(false);
    }
    let mut header = vec![0_u8; HEADER_SIZE];
    file.read_exact(&mut header)?;
    let metadata = match probe_metadata(&header) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(false),
    };
    let Some(payload_end) = (HEADER_SIZE as u64).checked_add(metadata.payload_size) else {
        return Ok(false);
    };
    Ok(payload_end <= length)
}

struct LeafMetadata {
    image_name: String,
    partition_name: String,
    payload_size: u64,
    secondary_size: Option<u64>,
    payload_sha256: [u8; 32],
}

fn probe_metadata(data: &[u8]) -> io::Result<LeafMetadata> {
    let (certificates, _) = parse_certificate_chain(data)?;
    parse_leaf_metadata(&certificates)
}

fn parse_certificate_chain(data: &[u8]) -> io::Result<(Vec<SecImageCertificate>, usize)> {
    let mut input = data;
    let mut offset = 0_usize;
    let mut certificates = Vec::with_capacity(CERTIFICATE_COUNT);
    let mut previous_subject: Option<String> = None;

    for chain_index in 0..CERTIFICATE_COUNT {
        let before = input.len();
        let (remainder, certificate) = parse_x509_certificate(input).map_err(|error| {
            invalid(format!(
                "invalid secure-image X.509 certificate #{} at offset {offset:#x}: {error}",
                chain_index + 1
            ))
        })?;
        let size = before
            .checked_sub(remainder.len())
            .ok_or_else(|| invalid("X.509 parser returned an invalid remainder"))?;
        if size == 0 {
            return Err(invalid("X.509 certificate has zero length"));
        }

        let subject = certificate.subject().to_string();
        let issuer = certificate.issuer().to_string();
        if chain_index == 0 && issuer != subject {
            return Err(invalid("secure-image root certificate is not self-issued"));
        }
        if let Some(expected_issuer) = &previous_subject
            && &issuer != expected_issuer
        {
            return Err(invalid(format!(
                "secure-image certificate #{} issuer does not match its parent subject",
                chain_index + 1
            )));
        }

        certificates.push(summarize_certificate(
            &certificate,
            chain_index,
            offset,
            size,
        )?);
        previous_subject = Some(subject);
        offset = offset
            .checked_add(size)
            .ok_or_else(|| invalid("secure-image certificate offset overflows"))?;
        if offset > HEADER_SIZE {
            return Err(invalid(
                "secure-image certificate chain exceeds the 0x2000 header",
            ));
        }
        input = remainder;
    }

    Ok((certificates, offset))
}

fn summarize_certificate(
    certificate: &X509Certificate<'_>,
    chain_index: usize,
    offset: usize,
    size: usize,
) -> io::Result<SecImageCertificate> {
    let mut seen_oids = HashSet::new();
    let mut proprietary_extensions = Vec::new();
    for extension in certificate.extensions() {
        let oid = extension.oid.to_id_string();
        if !seen_oids.insert(oid.clone()) {
            return Err(invalid(format!(
                "duplicate X.509 extension {oid} in certificate #{}",
                chain_index + 1
            )));
        }
        if oid.starts_with("2.20.") {
            proprietary_extensions.push(summarize_extension(extension, oid));
        }
    }

    let validity = certificate.validity();
    Ok(SecImageCertificate {
        chain_index,
        offset: offset as u64,
        size: size as u64,
        subject: certificate.subject().to_string(),
        issuer: certificate.issuer().to_string(),
        serial_hex: certificate.raw_serial_as_string(),
        not_before: validity
            .not_before
            .to_rfc2822()
            .unwrap_or_else(|_| validity.not_before.to_string()),
        not_after: validity
            .not_after
            .to_rfc2822()
            .unwrap_or_else(|_| validity.not_after.to_string()),
        signature_algorithm_oid: certificate.signature_algorithm.algorithm.to_id_string(),
        proprietary_extensions,
    })
}

fn summarize_extension(extension: &X509Extension<'_>, oid: String) -> SecImageExtension {
    SecImageExtension {
        oid,
        critical: extension.critical,
        value: decode_extension_value(extension.value),
    }
}

fn decode_extension_value(raw: &[u8]) -> SecImageExtensionValue {
    let Some((tag, content)) = single_der_value(raw) else {
        return SecImageExtensionValue::Der {
            hex: hex::encode(raw),
        };
    };
    match tag {
        0x02 => decode_unsigned_integer(content)
            .map(|value| SecImageExtensionValue::Integer { value })
            .unwrap_or_else(|| SecImageExtensionValue::Der {
                hex: hex::encode(raw),
            }),
        0x04 => SecImageExtensionValue::Octets {
            hex: hex::encode(content),
            text: printable_text(content),
        },
        _ => SecImageExtensionValue::Der {
            hex: hex::encode(raw),
        },
    }
}

fn parse_leaf_metadata(certificates: &[SecImageCertificate]) -> io::Result<LeafMetadata> {
    let leaf = certificates
        .get(CERTIFICATE_COUNT - 1)
        .ok_or_else(|| invalid("secure-image leaf certificate is missing"))?;

    let image_name = required_text(leaf, IMAGE_NAME_OID)?;
    let partition_name = required_text(leaf, PARTITION_NAME_OID)?;
    validate_name(&image_name, "image")?;
    validate_name(&partition_name, "partition")?;

    for certificate in certificates {
        for extension in &certificate.proprietary_extensions {
            let expected = match extension.oid.rsplit('.').next() {
                Some("8") => &image_name,
                Some("14") => &partition_name,
                _ => continue,
            };
            let name = extension_text(extension).ok_or_else(|| {
                invalid(format!(
                    "secure-image name extension {} is not text",
                    extension.oid
                ))
            })?;
            if name != expected {
                return Err(invalid(format!(
                    "secure-image name extension {} disagrees across the certificate chain",
                    extension.oid
                )));
            }
        }
    }

    let payload_size = required_integer(leaf, PAYLOAD_SIZE_OID)?;
    if payload_size == 0 {
        return Err(invalid("secure-image payload size is zero"));
    }
    let secondary_size = optional_integer(leaf, SECONDARY_SIZE_OID)?;
    let payload_hash = required_octets(leaf, PAYLOAD_SHA256_OID)?;
    let payload_sha256: [u8; 32] = payload_hash
        .try_into()
        .map_err(|_| invalid("secure-image payload SHA-256 is not 32 bytes"))?;

    Ok(LeafMetadata {
        image_name,
        partition_name,
        payload_size,
        secondary_size,
        payload_sha256,
    })
}

fn validate_name(name: &str, field: &str) -> io::Result<()> {
    if name.len() > 64
        || name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(invalid(format!(
            "secure-image {field} name contains unsupported characters"
        )));
    }
    Ok(())
}

fn find_extension<'a>(
    certificate: &'a SecImageCertificate,
    oid: &str,
) -> Option<&'a SecImageExtension> {
    certificate
        .proprietary_extensions
        .iter()
        .find(|extension| extension.oid == oid)
}

fn required_integer(certificate: &SecImageCertificate, oid: &str) -> io::Result<u64> {
    optional_integer(certificate, oid)?.ok_or_else(|| {
        invalid(format!(
            "secure-image certificate extension {oid} is missing"
        ))
    })
}

fn optional_integer(certificate: &SecImageCertificate, oid: &str) -> io::Result<Option<u64>> {
    let Some(extension) = find_extension(certificate, oid) else {
        return Ok(None);
    };
    match extension.value {
        SecImageExtensionValue::Integer { value } => Ok(Some(value)),
        _ => Err(invalid(format!(
            "secure-image certificate extension {oid} is not an unsigned integer"
        ))),
    }
}

fn required_octets(certificate: &SecImageCertificate, oid: &str) -> io::Result<Vec<u8>> {
    let extension = find_extension(certificate, oid).ok_or_else(|| {
        invalid(format!(
            "secure-image certificate extension {oid} is missing"
        ))
    })?;
    match &extension.value {
        SecImageExtensionValue::Octets { hex, .. } => hex::decode(hex).map_err(|_| {
            invalid(format!(
                "secure-image certificate extension {oid} is invalid"
            ))
        }),
        _ => Err(invalid(format!(
            "secure-image certificate extension {oid} is not an octet string"
        ))),
    }
}

fn required_text(certificate: &SecImageCertificate, oid: &str) -> io::Result<String> {
    let extension = find_extension(certificate, oid).ok_or_else(|| {
        invalid(format!(
            "secure-image certificate extension {oid} is missing"
        ))
    })?;
    extension_text(extension).map(str::to_owned).ok_or_else(|| {
        invalid(format!(
            "secure-image certificate extension {oid} is not text"
        ))
    })
}

fn extension_text(extension: &SecImageExtension) -> Option<&str> {
    match &extension.value {
        SecImageExtensionValue::Octets {
            text: Some(text), ..
        } => Some(text),
        _ => None,
    }
}

fn single_der_value(raw: &[u8]) -> Option<(u8, &[u8])> {
    if raw.len() < 2 || raw[0] & 0x1f == 0x1f {
        return None;
    }
    let first_length = raw[1];
    let (header_size, content_size) = if first_length & 0x80 == 0 {
        (2_usize, first_length as usize)
    } else {
        let length_bytes = (first_length & 0x7f) as usize;
        if length_bytes == 0
            || length_bytes > std::mem::size_of::<usize>()
            || raw.len() < 2 + length_bytes
            || raw[2] == 0
        {
            return None;
        }
        let mut length = 0_usize;
        for byte in &raw[2..2 + length_bytes] {
            length = length.checked_mul(256)?.checked_add(*byte as usize)?;
        }
        if length < 128 {
            return None;
        }
        (2 + length_bytes, length)
    };
    let end = header_size.checked_add(content_size)?;
    (end == raw.len()).then_some((raw[0], &raw[header_size..end]))
}

fn decode_unsigned_integer(content: &[u8]) -> Option<u64> {
    if content.is_empty() || content[0] & 0x80 != 0 {
        return None;
    }
    let value = if content[0] == 0 {
        if content.len() == 1 {
            return Some(0);
        }
        if content[1] & 0x80 == 0 {
            return None;
        }
        &content[1..]
    } else {
        content
    };
    if value.len() > std::mem::size_of::<u64>() {
        return None;
    }
    Some(
        value
            .iter()
            .fold(0_u64, |result, byte| (result << 8) | u64::from(*byte)),
    )
}

fn printable_text(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || !bytes.iter().all(|byte| (0x20..=0x7e).contains(byte)) {
        return None;
    }
    std::str::from_utf8(bytes).ok().map(str::to_owned)
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
