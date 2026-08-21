use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[allow(non_camel_case_types)]
pub enum FileFormat {
    UNKNOWN,
    GZIP,
    ZOPFLI,
    XZ,
    LZMA,
    BZIP2,
    LZ4,
    LZ4_LEGACY,
    LZ4_LG,
    LZOP,
    RAW,
}

impl FromStr for FileFormat {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "gzip" => Ok(Self::GZIP),
            "zopfli" => Ok(Self::ZOPFLI),
            "xz" => Ok(Self::XZ),
            "lzma" => Ok(Self::LZMA),
            "bzip2" => Ok(Self::BZIP2),
            "lz4" => Ok(Self::LZ4),
            "lz4_legacy" => Ok(Self::LZ4_LEGACY),
            "lz4_lg" => Ok(Self::LZ4_LG),
            "raw" => Ok(Self::RAW),
            _ => Err(()),
        }
    }
}

impl Display for FileFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FileFormat {
    pub fn as_str(&self) -> &'static str {
        match *self {
            Self::GZIP => "gzip",
            Self::ZOPFLI => "zopfli",
            Self::LZOP => "lzop",
            Self::XZ => "xz",
            Self::LZMA => "lzma",
            Self::BZIP2 => "bzip2",
            Self::LZ4 => "lz4",
            Self::LZ4_LEGACY => "lz4_legacy",
            Self::LZ4_LG => "lz4_lg",
            Self::RAW => "raw",
            Self::UNKNOWN => "unknown",
        }
    }

    pub fn ext(&self) -> &'static str {
        match *self {
            Self::GZIP | Self::ZOPFLI => "gz",
            Self::LZOP => "lzo",
            Self::XZ => "xz",
            Self::LZMA => "lzma",
            Self::BZIP2 => "bz2",
            Self::LZ4 | Self::LZ4_LEGACY | Self::LZ4_LG => "lz4",
            _ => "",
        }
    }

    pub fn is_compressed(&self) -> bool {
        matches!(
            *self,
            Self::GZIP
                | Self::ZOPFLI
                | Self::XZ
                | Self::LZMA
                | Self::BZIP2
                | Self::LZ4
                | Self::LZ4_LEGACY
                | Self::LZ4_LG
        )
    }

    pub fn formats() -> String {
        [
            Self::GZIP,
            Self::ZOPFLI,
            Self::XZ,
            Self::LZMA,
            Self::BZIP2,
            Self::LZ4,
            Self::LZ4_LEGACY,
            Self::LZ4_LG,
        ]
        .iter()
        .map(|f| f.to_string())
        .collect::<Vec<_>>()
        .join(" ")
    }
}

pub fn check_fmt(buf: &[u8]) -> FileFormat {
    if buf.len() < 16 {
        return FileFormat::UNKNOWN;
    }
    match &buf[0..4] {
        [0x1F, 0x8B, 0x08, _] => FileFormat::GZIP,
        [0xFD, 0x37, 0x7A, 0x58] => FileFormat::XZ,
        [0x42, 0x5A, 0x68, _] => FileFormat::BZIP2,
        [0x5D, 0x00, 0x00, _] => FileFormat::LZMA,
        [0x04, 0x22, 0x4D, 0x18] => FileFormat::LZ4,
        [0x02, 0x21, 0x4C, 0x18] => FileFormat::LZ4_LEGACY,
        [0x89, 0x4C, 0x5A, 0x4F] => FileFormat::LZOP,
        _ => {
            if &buf[0..6] == b"070701" || &buf[0..6] == b"070702" {
                FileFormat::RAW
            } else {
                FileFormat::UNKNOWN
            }
        }
    }
}
