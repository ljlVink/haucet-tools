use crate::fs_util;
use anyhow::{Context, Result, ensure};
use ext4_extract::{ExtractReport, NodeMetadata};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const EXT4_MAGIC_OFFSET: u64 = 1024 + 56;
const EXT4_MAGIC: [u8; 2] = [0x53, 0xef];

pub fn is_ext4(path: &Path) -> Result<bool> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    if file.metadata()?.len() < EXT4_MAGIC_OFFSET + EXT4_MAGIC.len() as u64 {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(EXT4_MAGIC_OFFSET))?;
    let mut magic = [0_u8; 2];
    file.read_exact(&mut magic)?;
    Ok(magic == EXT4_MAGIC)
}

pub fn unpack(image: &Path, out: &Path, force: bool) -> Result<ExtractReport> {
    fs_util::ensure_output_does_not_contain(image, out)?;
    ensure!(
        is_ext4(image)?,
        "{} is not an ext2/ext4 image",
        image.display()
    );
    fs_util::prepare_dir_excluding(out, "ext4 output directory", force, &[image])?;

    let partition = image
        .file_stem()
        .and_then(OsStr::to_str)
        .filter(|name| fs_util::is_simple_name(name))
        .context("ext4 image filename cannot be used as a partition name")?;
    let source_dir = out.join(partition);
    let config_dir = out.join("config");
    fs::create_dir(&source_dir)
        .with_context(|| format!("creating ext4 source directory {}", source_dir.display()))?;
    fs::create_dir(&config_dir)
        .with_context(|| format!("creating ext4 config directory {}", config_dir.display()))?;

    eprintln!("extracting ext4 image {}", image.display());
    let report =
        ext4_extract::extract(image, &source_dir).context("embedded ext4 extraction failed")?;
    write_android_metadata(&config_dir, partition, &report.nodes)?;
    eprintln!(
        "extracted {} directories, {} files, and {} symlinks ({} bytes)",
        report.directories, report.files, report.symlinks, report.bytes_written
    );
    if report.special_files_skipped != 0 {
        eprintln!(
            "skipped {} ext4 special files that cannot be represented portably",
            report.special_files_skipped
        );
    }
    Ok(report)
}

fn write_android_metadata(
    config_dir: &Path,
    partition: &str,
    nodes: &[NodeMetadata],
) -> Result<()> {
    let fs_config_path = config_dir.join(format!("{partition}_fs_config"));
    fs_util::atomic_write(&fs_config_path, "fs-config", |writer| {
        for node in nodes {
            write!(
                writer,
                "{} {} {} {:04o}",
                node.path,
                node.uid,
                node.gid,
                node.mode & 0o777
            )?;
            if let Some(capabilities) = node.capabilities {
                write!(writer, " capabilities=0x{capabilities:X}")?;
            }
            writeln!(writer)?;
        }
        Ok(())
    })?;

    let file_contexts_path = config_dir.join(format!("{partition}_file_contexts"));
    fs_util::atomic_write(&file_contexts_path, "file-contexts", |writer| {
        for node in nodes {
            if let Some(context) = &node.selinux_context {
                writeln!(
                    writer,
                    "{} {context}",
                    escape_file_contexts_path(&node.path)
                )?;
            }
        }
        Ok(())
    })?;
    eprintln!(
        "wrote {} and {}",
        fs_config_path.display(),
        file_contexts_path.display()
    );
    Ok(())
}

fn escape_file_contexts_path(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for character in path.chars() {
        if matches!(character, '.' | '+' | '[' | ']' | '*') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}