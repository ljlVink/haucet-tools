//! Lightweight file-type detection based on magic bytes, mirroring what the
//! CLI's subcommands recognize. Used by the home page's drop zone to offer
//! the right actions for any dropped file.

use std::fs::File;
use std::io::Read;
use std::path::Path;

const HVB_FOOTER_SIZE: usize = 104;
const HVB_FOOTER_MAGIC: &[u8; 8] = b"HVB\0\0\0\0\0";
const HARMONY_MAGIC: &[u8; 8] = b"HARMONY!";
const RVT_MAGIC: &[u8; 4] = b"rot\0";
const EROFS_MAGIC: &[u8; 4] = &[0xe2, 0xe1, 0xf5, 0xe0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Unknown,
    /// update_full_base.zip 之类的 ZIP 更新包
    ZipPackage,
    /// update.bin 组件包
    UpdateBin,
    /// EROFS 分区镜像
    Erofs,
    /// HARMONY! 头（ramdisk 或分区镜像）
    HarmonyFrame,
    /// RVT (rot\0) 密钥镜像
    Rvt,
    /// 仅有 HVB 尾部包装的分区镜像
    HvbWrapped,
    /// 裸 cpio 归档（ramdisk.cpio）
    Cpio,
    /// EROFS 解包工作区（含 haucet-erofs.json）
    ErofsWorkspace,
    /// Ramdisk 工作区（含 ramdisk.cpio）
    RamdiskWorkspace,
}

impl FileKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "未知格式",
            Self::ZipPackage => "ZIP 更新包",
            Self::UpdateBin => "update.bin",
            Self::Erofs => "EROFS 分区镜像",
            Self::HarmonyFrame => "HARMONY! 镜像",
            Self::Rvt => "RVT 密钥镜像",
            Self::HvbWrapped => "HVB 分区镜像",
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
            "这是 EROFS 解包工作区，可以直接重新打包".to_owned(),
        )
    } else if has_cpio {
        (
            FileKind::RamdiskWorkspace,
            "这是 ramdisk 解包工作区，可以直接重新打包镜像".to_owned(),
        )
    } else {
        (FileKind::Unknown, "目录中没有识别到 haucet 工作区".to_owned())
    }
}

fn detect_file(path: &Path) -> (FileKind, String) {
    let Ok(mut file) = File::open(path) else {
        return (FileKind::Unknown, "无法打开文件".to_owned());
    };
    let Ok(metadata) = file.metadata() else {
        return (FileKind::Unknown, "无法读取文件信息".to_owned());
    };
    let length = metadata.len();
    if length < 8 {
        return (FileKind::Unknown, "文件太小，无法识别".to_owned());
    }

    let mut head = [0_u8; 180];
    let head_len = read_at(&mut file, &mut head, 0).unwrap_or(0);

    // ZIP package
    if head_len >= 4 && &head[0..4] == b"PK\x03\x04" {
        return (
            FileKind::ZipPackage,
            "这是 ZIP 压缩包（华为 update_full_base.zip 更新包候选），可以解包分区镜像".to_owned(),
        );
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
                FileKind::UpdateBin,
                format!("这是 update.bin 组件包（{} 个分区），可以查看和解包分区", compinfo_len / 71),
            );
        }
    }

    // cpio archive
    if head_len >= 6 && (&head[0..6] == b"070701" || &head[0..6] == b"070702") {
        return (
            FileKind::Cpio,
            "这是 cpio 归档（ramdisk.cpio），可以浏览和编辑内容".to_owned(),
        );
    }

    // HARMONY! frame
    if head_len >= 8 && &head[0..8] == HARMONY_MAGIC {
        return (
            FileKind::HarmonyFrame,
            "这是 HARMONY! 包装的镜像（ramdisk 或分区），可以做 ramdisk 操作或查看分区信息".to_owned(),
        );
    }

    // RVT
    if head_len >= 4 && &head[0..4] == RVT_MAGIC {
        return (
            FileKind::Rvt,
            "这是 RVT 密钥镜像（rot\\0），包含分区公钥描述符".to_owned(),
        );
    }

    // EROFS magic at offset 1024
    let mut erofs_magic = [0_u8; 4];
    if length >= 1028
        && read_at(&mut file, &mut erofs_magic, 1024).unwrap_or(0) == 4
        && &erofs_magic == EROFS_MAGIC
    {
        return (
            FileKind::Erofs,
            "这是 EROFS 分区镜像（如 system/vendor），可以解包或查看分区信息".to_owned(),
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
            "这是带 HVB 尾部包装的分区镜像，可以查看分区信息".to_owned(),
        );
    }

    (
        FileKind::Unknown,
        "没有识别出已知格式；可以尝试“查看分区信息”或直接解包".to_owned(),
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
