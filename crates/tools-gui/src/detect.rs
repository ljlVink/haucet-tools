use std::fs::File;
use std::io::Read;
use std::path::Path;

use common::nvme::{NVE_BLOCK_SIZE, NVE_HEADER_MAGIC, NVE_HEADER_SIZE};

const HVB_FOOTER_SIZE: usize = 104;
const HVB_FOOTER_MAGIC: &[u8; 8] = b"HVB\0\0\0\0\0";
const HARMONY_MAGIC: &[u8; 8] = b"HARMONY!";
const RVT_MAGIC: &[u8; 4] = b"rot\0";
const GPT_MAGIC: &[u8; 8] = b"EFI PART";
const EROFS_MAGIC: &[u8; 4] = &[0xe2, 0xe1, 0xf5, 0xe0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Unknown,
    ZipPackage,
    Erofs,
    HarmonyFrame,
    Rvt,
    Gpt,
    HvbWrapped,
    Nve,
    Cpio,
    ErofsWorkspace,
    RamdiskWorkspace,
}

impl FileKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "未知格式",
            Self::ZipPackage => "ZIP 更新包",
            Self::Erofs => "EROFS 分区镜像",
            Self::HarmonyFrame => "HARMONY! 镜像",
            Self::Rvt => "RVT 密钥镜像",
            Self::Gpt => "GPT 分区表镜像",
            Self::HvbWrapped => "HVB 分区镜像",
            Self::Nve => "Hisi-NV-Partition",
            Self::Cpio => "cpio 归档",
            Self::ErofsWorkspace => "EROFS 工作区",
            Self::RamdiskWorkspace => "Ramdisk 工作区",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Detection {
    pub kind: FileKind,
    pub path: String,
    pub human: String,
}

pub fn detect(path: &Path) -> Detection {
    let path_string = path.display().to_string();
    let human = if path.is_dir() {
        detect_dir(path)
    } else {
        detect_file(path)
    };
    Detection {
        kind: human.0,
        path: path_string,
        human: human.1,
    }
}

fn detect_dir(path: &Path) -> (FileKind, String) {
    let mut has_cpio = false;
    let mut is_erofs_workspace = false;
    let Ok(entries) = std::fs::read_dir(path) else {
        return (FileKind::Unknown, "无法读取目录".to_owned());
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "haucet-erofs.json" {
            is_erofs_workspace = true;
        }
        if name == "ramdisk.cpio" {
            has_cpio = true;
        }
    }
    if is_erofs_workspace {
        (
            FileKind::ErofsWorkspace,
            "这是 EROFS 解包工作区, 可以直接重新打包".to_owned(),
        )
    } else if has_cpio {
        (
            FileKind::RamdiskWorkspace,
            "这是 ramdisk 解包工作区, 可以直接重新打包镜像".to_owned(),
        )
    } else {
        (
            FileKind::Unknown,
            "目录中没有识别到 haucet 工作区".to_owned(),
        )
    }
}

//todo use local funcs, rather than judge itself.
fn detect_file(path: &Path) -> (FileKind, String) {
    let Ok(mut file) = File::open(path) else {
        return (FileKind::Unknown, "无法打开文件".to_owned());
    };
    let Ok(metadata) = file.metadata() else {
        return (FileKind::Unknown, "无法读取文件信息".to_owned());
    };
    let length = metadata.len();
    if length < 8 {
        return (FileKind::Unknown, "文件太小, 无法识别".to_owned());
    }

    let mut head = [0_u8; 180];
    let head_len = read_at(&mut file, &mut head, 0).unwrap_or(0);

    if length >= NVE_BLOCK_SIZE as u64 && length % NVE_BLOCK_SIZE as u64 == 0 {
        let mut nve_magic = [0_u8; NVE_HEADER_MAGIC.len()];
        let header_offset = (NVE_BLOCK_SIZE - NVE_HEADER_SIZE) as u64;
        let mut block_offset = 0_u64;
        while block_offset < length {
            if read_at(&mut file, &mut nve_magic, block_offset + header_offset).unwrap_or(0)
                == nve_magic.len()
                && nve_magic == NVE_HEADER_MAGIC
            {
                return (
                    FileKind::Nve,
                    "Hisi-NV-Partition NVE 镜像, 可打开 NVE 编辑器".to_owned(),
                );
            }
            block_offset += NVE_BLOCK_SIZE as u64;
        }
    }

    // ZIP package
    if head_len >= 4 && &head[0..4] == b"PK\x03\x04" {
        return (FileKind::ZipPackage, "ZIP 压缩包".to_owned());
    }

    // update.bin: TLV type 0x01 (L2) or 0x11 (L1) + sane component table size
    if head_len >= 180 {
        let tlv_type = u16::from_be_bytes([head[0], head[1]]);
        let compinfo_len = u16::from_be_bytes([head[178], head[179]]) as usize;
        if (tlv_type == 0x01 || tlv_type == 0x11)
            && compinfo_len > 0
            && compinfo_len <= 8 * 1024 * 1024
            && length as usize >= 180 + compinfo_len + 16
        {
            return (
                FileKind::ZipPackage,
                format!(
                    "update.bin 组件包({} 个分区), 可以查看和解包分区",
                    compinfo_len / 71
                ),
            );
        }
    }

    // cpio archive
    if head_len >= 6 && (&head[0..6] == b"070701" || &head[0..6] == b"070702") {
        return (FileKind::Cpio, "cpio 归档, 可浏览和编辑内容".to_owned());
    }

    // HARMONY! frame
    if head_len >= 8 && &head[0..8] == HARMONY_MAGIC {
        return (
            FileKind::HarmonyFrame,
            "HARMONY! 包装的镜像, 可进行 ramdisk 操作".to_owned(),
        );
    }

    // RVT
    if head_len >= 4 && &head[0..4] == RVT_MAGIC {
        return (FileKind::Rvt, "RVT 密钥镜像, 包含分区公钥描述符".to_owned());
    }

    // GPT header is at LBA 1 (offset 512) for a standard 512-byte logical block.
    let mut gpt_magic = [0_u8; 8];
    if length >= 520
        && read_at(&mut file, &mut gpt_magic, 512).unwrap_or(0) == GPT_MAGIC.len()
        && &gpt_magic == GPT_MAGIC
    {
        return (FileKind::Gpt, "GPT 分区表镜像".to_owned());
    }

    // EROFS magic at offset 1024
    let mut erofs_magic = [0_u8; 4];
    if length >= 1028
        && read_at(&mut file, &mut erofs_magic, 1024).unwrap_or(0) == 4
        && &erofs_magic == EROFS_MAGIC
    {
        return (
            FileKind::Erofs,
            "EROFS 分区镜像, 可解包或查看分区信息".to_owned(),
        );
    }

    // HVB footer at the tail
    let mut footer = [0_u8; HVB_FOOTER_SIZE];
    if length >= HVB_FOOTER_SIZE as u64
        && read_at(&mut file, &mut footer, length - HVB_FOOTER_SIZE as u64).unwrap_or(0)
            == HVB_FOOTER_SIZE
        && &footer[0..8] == HVB_FOOTER_MAGIC
    {
        return (
            FileKind::HvbWrapped,
            "HVB 尾部包装的分区镜像, 可查看分区信息".to_owned(),
        );
    }

    (
        FileKind::Unknown,
        "没有识别出已知格式; 可以尝试\"查看分区信息\"或直接解包".to_owned(),
    )
}

fn read_at(file: &mut File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(offset))?;
    let mut read = 0;
    while read < buf.len() {
        let n = file.read(&mut buf[read..])?;
        if n == 0 {
            break;
        }
        read += n;
    }
    Ok(read)
}
