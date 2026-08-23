use std::{fmt::Display, num::ParseIntError};
use thiserror::Error;
use tracing::trace;

fn bytes_slice_null(bytes: &[u8]) -> &[u8] {
    match bytes.iter().position(|&b| b == 0x00) {
        Some(pos) => &bytes[..pos],
        None => bytes,
    }
}

pub fn parse_u32(s: &str) -> Result<u32, ParseIntError> {
    if s.starts_with("0x") {
        parse_u32_hex(s)
    } else {
        s.parse()
    }
}

pub fn parse_u32_hex(hex: &str) -> Result<u32, ParseIntError> {
    let hex = hex.strip_prefix("0x").unwrap_or("invalid");
    u32::from_str_radix(hex, 16)
}

pub fn parse_u64_hex(hex: &str) -> Result<u64, ParseIntError> {
    let hex = hex.strip_prefix("0x").unwrap_or("invalid");
    u64::from_str_radix(hex, 16)
}

#[derive(Debug)]
pub enum FastBootCommand<S> {
    GetVar(S),
    Download(u32),
    Verify(u32),
    Flash(S),
    Erase(S),
    Boot,
    Continue,
    Reboot,
    RebootBootloader,
    RebootTo(S),
    Powerdown,
    Oem(S),
}

impl<S: Display> Display for FastBootCommand<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FastBootCommand::GetVar(var) => write!(f, "getvar:{var}"),
            FastBootCommand::Download(size) => write!(f, "download:{size:08x}"),
            FastBootCommand::Verify(part) => write!(f, "verity:{part}"),
            FastBootCommand::Flash(part) => write!(f, "flash:{part}"),
            FastBootCommand::Erase(part) => write!(f, "erase:{part}"),
            FastBootCommand::Boot => write!(f, "boot"),
            FastBootCommand::Continue => write!(f, "continue"),
            FastBootCommand::Reboot => write!(f, "reboot"),
            FastBootCommand::RebootBootloader => write!(f, "reboot-bootloader"),
            FastBootCommand::RebootTo(mode) => write!(f, "reboot-{mode}"),
            FastBootCommand::Powerdown => write!(f, "powerdown"),
            FastBootCommand::Oem(cmd) => write!(f, "oem {cmd}"),
        }
    }
}

#[derive(Error, Debug, PartialEq, Eq)]
pub enum FastBootResponseParseError {
    #[error("Unknown response type")]
    UnknownReply,
    #[error("Couldn't parse response type")]
    ParseType,
    #[error("Couldn't parse response payload")]
    ParsePayload,
    #[error("Couldn't parse DATA length")]
    DataLength,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FastBootResponse {
    Okay(String),
    Info(String),
    Text(String),
    Fail(String),
    Data(u32),
}

impl<'a> FastBootResponse {
    fn from_parts(resp: &str, data: &'a str) -> Result<Self, FastBootResponseParseError> {
        trace!("Parsing Response: {} {}", resp, data);
        match resp {
            "OKAY" => Ok(Self::Okay(data.into())),
            "INFO" => Ok(Self::Info(data.into())),
            "TEXT" => Ok(Self::Text(data.into())),
            "FAIL" => Ok(Self::Fail(data.into())),
            "DATA" => {
                let offset = u32::from_str_radix(data, 16)
                    .or(Err(FastBootResponseParseError::DataLength))?;
                Ok(Self::Data(offset))
            }
            _ => Err(FastBootResponseParseError::UnknownReply),
        }
    }

    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self, FastBootResponseParseError> {
        if bytes.len() < 4 {
            Err(FastBootResponseParseError::UnknownReply)
        } else {
            let resp =
                std::str::from_utf8(&bytes[0..4]).or(Err(FastBootResponseParseError::ParseType))?;
            let data = std::str::from_utf8(bytes_slice_null(&bytes[4..]))
                .or(Err(FastBootResponseParseError::ParsePayload))?;

            Self::from_parts(resp, data)
        }
    }
}
