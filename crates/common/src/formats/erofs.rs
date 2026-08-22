use super::hvb::{HvbFooter, HvbWrapper};
use crate::fs_util;
use crate::tools::ToolPaths;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const EROFS_MAGIC: [u8; 4] = [0xe2, 0xe1, 0xf5, 0xe0];
const EROFS_MAGIC_OFFSET: u64 = 1024;
const MANIFEST_NAME: &str = "haucet-erofs.json";
const MANIFEST_VERSION: u32 = 1;
const CERTIFICATE_NAME: &str = "hvb-certificate.bin";
const HASH_BUFFER_SIZE: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErofsManifest {
    pub version: u32,
    pub partition: String,
    pub original_file_name: String,
    pub original_size: u64,
    pub original_sha256: String,
    pub source_dir: String,
    pub config_dir: String,
    pub fs_options_file: String,
    pub extract_erofs_version: String,
    pub mkfs_erofs_version: String,
    pub hvb: Option<HvbManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HvbManifest {
    pub footer: HvbFooter,
    pub certificate_file: String,
}

pub fn is_erofs(path: &Path) -> Result<bool> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    if file.metadata()?.len() < EROFS_MAGIC_OFFSET + EROFS_MAGIC.len() as u64 {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(EROFS_MAGIC_OFFSET))?;
    let mut magic = [0_u8; 4];
    file.read_exact(&mut magic)?;
    Ok(magic == EROFS_MAGIC)
}

pub fn unpack(image: &Path, out: &Path, force: bool) -> Result<()> {
    let tools = ToolPaths::discover(None)?;
    unpack_with_tools(image, out, &tools, force)
}

pub fn unpack_with_tools(image: &Path, out: &Path, tools: &ToolPaths, force: bool) -> Result<()> {
    ensure!(
        is_erofs(image)?,
        "{} is not an EROFS image",
        image.display()
    );
    fs_util::prepare_dir(out, "EROFS workspace", force)?;

    eprintln!("extracting EROFS image {}", image.display());
    run_status(
        Command::new(&tools.extract_erofs)
            .arg("-x")
            .arg("-i")
            .arg(image)
            .arg("-o")
            .arg(out),
        "extract.erofs",
    )?;

    let extracted_source_dir = find_source_dir(out)?;
    let extracted_name = extracted_source_dir
        .file_name()
        .and_then(OsStr::to_str)
        .context("extracted EROFS root has a non-UTF-8 name")?
        .to_owned();
    let config_dir = out.join("config");

    let wrapper = HvbWrapper::read_from(image)?;
    let partition = wrapper
        .as_ref()
        .and_then(HvbWrapper::partition_name)
        .filter(|name| fs_util::is_simple_name(name))
        .unwrap_or(&extracted_name)
        .to_owned();
    let source_dir = normalize_extraction(
        out,
        &config_dir,
        extracted_source_dir,
        &extracted_name,
        &partition,
    )?;
    let fs_options = find_file_with_suffix(&config_dir, "_fs_options")?;

    let hvb = if let Some(wrapper) = wrapper {
        let certificate_path = config_dir.join(CERTIFICATE_NAME);
        fs::write(&certificate_path, &wrapper.certificate)?;
        Some(HvbManifest {
            footer: wrapper.footer,
            certificate_file: relative_string(out, &certificate_path)?,
        })
    } else {
        None
    };

    let manifest = ErofsManifest {
        version: MANIFEST_VERSION,
        partition,
        original_file_name: image
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("partition.img")
            .to_owned(),
        original_size: fs::metadata(image)?.len(),
        original_sha256: sha256_file(image)?,
        source_dir: relative_string(out, &source_dir)?,
        config_dir: relative_string(out, &config_dir)?,
        fs_options_file: relative_string(out, &fs_options)?,
        extract_erofs_version: tool_version(&tools.extract_erofs),
        mkfs_erofs_version: tool_version(&tools.mkfs_erofs),
        hvb,
    };
    write_manifest(out, &manifest)?;
    eprintln!("wrote {}", out.join(MANIFEST_NAME).display());
    Ok(())
}

pub fn repack(workspace: &Path, output: &Path, allow_grow: bool) -> Result<()> {
    let tools = ToolPaths::discover(None)?;
    repack_with_tools(workspace, output, &tools, allow_grow)
}

pub fn repack_with_tools(
    workspace: &Path,
    output: &Path,
    tools: &ToolPaths,
    allow_grow: bool,
) -> Result<()> {
    ensure!(
        !output.exists(),
        "output already exists: {}",
        output.display()
    );
    let manifest = read_manifest(workspace)?;
    ensure!(
        manifest.version == MANIFEST_VERSION,
        "unsupported EROFS workspace version {}",
        manifest.version
    );
    let source_dir = fs_util::safe_join(workspace, &manifest.source_dir)?;
    let config_dir = fs_util::safe_join(workspace, &manifest.config_dir)?;
    let fs_options_path = fs_util::safe_join(workspace, &manifest.fs_options_file)?;
    ensure!(
        source_dir.is_dir(),
        "missing source tree: {}",
        source_dir.display()
    );
    ensure!(
        config_dir.is_dir(),
        "missing config directory: {}",
        config_dir.display()
    );

    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let raw_path = fs_util::sibling_temporary(output, "raw-erofs")?;
    let wrapped_path = fs_util::sibling_temporary(output, "wrapped")?;
    ensure!(
        !raw_path.exists(),
        "temporary file exists: {}",
        raw_path.display()
    );
    ensure!(
        !wrapped_path.exists(),
        "temporary file exists: {}",
        wrapped_path.display()
    );

    let result = (|| -> Result<()> {
        let preserved = parse_mkfs_options(&fs_options_path, &config_dir)?;
        eprintln!("rebuilding {} with mkfs.erofs", manifest.partition);
        let mut command = Command::new(&tools.mkfs_erofs);
        command
            .arg("-d1")
            .args(&preserved)
            .arg(&raw_path)
            .arg(&source_dir);
        run_status(&mut command, "mkfs.erofs")?;
        ensure!(is_erofs(&raw_path)?, "mkfs.erofs produced an invalid image");

        let raw_size = fs::metadata(&raw_path)?.len();
        if let Some(hvb) = &manifest.hvb {
            let certificate_path = fs_util::safe_join(workspace, &hvb.certificate_file)?;
            let wrapper = HvbWrapper {
                footer: hvb.footer.clone(),
                certificate: fs::read(&certificate_path).with_context(|| {
                    format!("reading HVB certificate {}", certificate_path.display())
                })?,
            };
            ensure!(
                wrapper.certificate.len() as u64 == wrapper.footer.cert_size,
                "HVB certificate size changed"
            );
            wrapper.write_repacked(&raw_path, &wrapped_path)?;
            eprintln!(
                "warning: the original HVB certificate was preserved, not cryptographically re-signed"
            );
        } else {
            ensure!(
                allow_grow || raw_size <= manifest.original_size,
                "rebuilt image is {raw_size} bytes, larger than original size {}; use --allow-grow to override",
                manifest.original_size
            );
            copy_raw_partition(
                &raw_path,
                &wrapped_path,
                if allow_grow {
                    raw_size.max(manifest.original_size)
                } else {
                    manifest.original_size
                },
            )?;
        }

        ensure!(is_erofs(&wrapped_path)?, "wrapped output is not EROFS");
        validate_with_extractor(&wrapped_path, workspace, tools)?;
        fs::rename(&wrapped_path, output)
            .with_context(|| format!("moving rebuilt image to {}", output.display()))?;
        Ok(())
    })();

    let _ = fs::remove_file(&raw_path);
    if result.is_err() {
        let _ = fs::remove_file(&wrapped_path);
    }
    result?;
    eprintln!("wrote {}", output.display());
    Ok(())
}

fn find_source_dir(workspace: &Path) -> Result<PathBuf> {
    let mut directories = Vec::new();
    for entry in fs::read_dir(workspace)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && entry.file_name() != OsStr::new("config") {
            directories.push(entry.path());
        }
    }
    ensure!(
        directories.len() == 1,
        "expected one extracted filesystem root in {}, found {}",
        workspace.display(),
        directories.len()
    );
    Ok(directories.remove(0))
}

fn find_file_with_suffix(directory: &Path, suffix: &str) -> Result<PathBuf> {
    let mut matches = Vec::new();
    for entry in fs::read_dir(directory)
        .with_context(|| format!("reading config directory {}", directory.display()))?
    {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.file_name().to_string_lossy().ends_with(suffix) {
            matches.push(entry.path());
        }
    }
    ensure!(
        matches.len() == 1,
        "expected one *{suffix} file in {}, found {}",
        directory.display(),
        matches.len()
    );
    Ok(matches.remove(0))
}

fn normalize_extraction(
    workspace: &Path,
    config_dir: &Path,
    source_dir: PathBuf,
    extracted_name: &str,
    partition: &str,
) -> Result<PathBuf> {
    if extracted_name == partition {
        return Ok(source_dir);
    }
    ensure!(
        fs_util::is_simple_name(partition),
        "unsafe partition name in HVB certificate: {partition:?}"
    );
    let normalized_source = workspace.join(partition);
    ensure!(
        !normalized_source.exists(),
        "normalized source path already exists: {}",
        normalized_source.display()
    );
    fs::rename(&source_dir, &normalized_source)?;

    for suffix in ["_fs_config", "_file_contexts", "_fs_options"] {
        let old_path = find_file_with_suffix(config_dir, suffix)?;
        let text = fs::read_to_string(&old_path)
            .with_context(|| format!("reading extracted metadata {}", old_path.display()))?;
        let new_path = config_dir.join(format!("{partition}{suffix}"));
        fs::write(&new_path, text.replace(extracted_name, partition))?;
        fs::remove_file(old_path)?;
    }
    Ok(normalized_source)
}

fn parse_mkfs_options(path: &Path, config_dir: &Path) -> Result<Vec<String>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading mkfs options from {}", path.display()))?;
    let line = text
        .lines()
        .find_map(|line| {
            line.split_once("mkfs.erofs options:")
                .map(|(_, value)| value.trim())
        })
        .context("fs_options does not contain a mkfs.erofs command")?;
    let mut words = shlex::split(line).context("invalid shell quoting in mkfs.erofs options")?;
    ensure!(
        words.len() >= 2,
        "mkfs.erofs options are missing output/source paths"
    );
    words.truncate(words.len() - 2);

    let mut output = Vec::new();
    let mut index = 0;
    while index < words.len() {
        let word = &words[index];
        if let Some(original) = word.strip_prefix("--fs-config-file=") {
            let basename = Path::new(original)
                .file_name()
                .context("invalid fs_config path")?;
            let path = config_dir.join(basename);
            ensure!(path.is_file(), "missing fs_config file: {}", path.display());
            output.push(format!("--fs-config-file={}", path.display()));
        } else if let Some(original) = word.strip_prefix("--file-contexts=") {
            let basename = Path::new(original)
                .file_name()
                .context("invalid file_contexts path")?;
            let path = config_dir.join(basename);
            ensure!(
                path.is_file(),
                "missing file_contexts file: {}",
                path.display()
            );
            output.push(format!("--file-contexts={}", path.display()));
        } else if option_with_inline_value(word) || flag_option(word) {
            output.push(word.clone());
        } else if matches!(word.as_str(), "-T" | "-U" | "-L") {
            let value = words
                .get(index + 1)
                .with_context(|| format!("{word} is missing its value"))?;
            output.push(word.clone());
            output.push(value.clone());
            index += 1;
        } else {
            bail!("unsupported recorded mkfs.erofs option {word:?}")
        }
        index += 1;
    }
    Ok(output)
}

fn option_with_inline_value(option: &str) -> bool {
    ["-z", "-C", "-b", "-d", "-x", "-E"]
        .iter()
        .any(|prefix| option.starts_with(prefix) && option.len() > prefix.len())
        || [
            "--mount-point=",
            "--force-uid=",
            "--force-gid=",
            "--uid-offset=",
            "--gid-offset=",
            "--max-extent-bytes=",
            "--xattr-prefix=",
            "--ovlfs-strip=",
        ]
        .iter()
        .any(|prefix| option.starts_with(prefix))
}

fn flag_option(option: &str) -> bool {
    matches!(
        option,
        "--all-root" | "--ignore-mtime" | "--preserve-mtime" | "--aufs"
    )
}

fn validate_with_extractor(image: &Path, workspace: &Path, tools: &ToolPaths) -> Result<()> {
    let validation = workspace.join(".haucet-validation");
    if validation.exists() {
        fs::remove_dir_all(&validation)?;
    }
    fs::create_dir_all(&validation)?;
    let status = Command::new(&tools.extract_erofs)
        .arg("-x")
        .arg("--only-cfg")
        .arg("-i")
        .arg(image)
        .arg("-o")
        .arg(&validation)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("running extract.erofs validation")?;
    let _ = fs::remove_dir_all(&validation);
    ensure!(status.success(), "extract.erofs rejected the rebuilt image");
    Ok(())
}

fn copy_raw_partition(source: &Path, destination: &Path, final_size: u64) -> Result<()> {
    let mut source = BufReader::new(File::open(source)?);
    let destination_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    destination_file.set_len(final_size)?;
    let mut destination = BufWriter::new(destination_file);
    io::copy(&mut source, &mut destination)?;
    destination.flush()?;
    Ok(())
}

fn run_status(command: &mut Command, name: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("running {name}"))?;
    ensure!(status.success(), "{name} exited with {status}");
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut reader = BufReader::with_capacity(HASH_BUFFER_SIZE, File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn tool_version(tool: &Path) -> String {
    Command::new(tool)
        .arg("--version")
        .output()
        .map(|output| {
            let mut bytes = output.stdout;
            bytes.extend(output.stderr);
            String::from_utf8_lossy(&bytes).trim().to_owned()
        })
        .unwrap_or_else(|error| format!("unavailable: {error}"))
}

fn write_manifest(workspace: &Path, manifest: &ErofsManifest) -> Result<()> {
    let path = workspace.join(MANIFEST_NAME);
    let json = serde_json::to_vec_pretty(manifest)?;
    fs_util::atomic_write(&path, "manifest", |writer| {
        writer.write_all(&json)?;
        Ok(())
    })
}

fn read_manifest(workspace: &Path) -> Result<ErofsManifest> {
    let path = workspace.join(MANIFEST_NAME);
    let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes).context("parsing EROFS workspace manifest")
}

fn relative_string(base: &Path, path: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(base)
        .with_context(|| format!("{} is outside {}", path.display(), base.display()))?
        .to_string_lossy()
        .into_owned())
}
