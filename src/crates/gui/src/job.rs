use crate::model::{LayoutChoice, Operation};
use anyhow::{Context, Result, ensure};
use common::formats::update_bin::{self, UpdateLayout};
use common::tools::ToolPaths;
use common::{formats::erofs, package, ramdisk};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

pub(crate) struct Job {
    pub(crate) operation: Operation,
    pub(crate) layout: LayoutChoice,
    pub(crate) input: String,
    pub(crate) secondary: String,
    pub(crate) output: String,
    pub(crate) tools_dir: String,
    pub(crate) partitions: String,
    pub(crate) force: bool,
    pub(crate) all_erofs: bool,
    pub(crate) allow_grow: bool,
}

pub(crate) struct JobResult {
    pub(crate) success: bool,
    pub(crate) message: String,
}

pub(crate) fn run(job: Job) -> Result<String> {
    let input = required_path(&job.input, job.operation.input_label())?;
    let layout = job.layout.to_update_layout();
    match job.operation {
        Operation::FullUnpack => {
            let output = required_output(&job)?;
            package::unpack_full_with_tools(
                &input,
                &output,
                &discover_tools(&job)?,
                &parse_partitions(&job.partitions),
                job.all_erofs,
                layout,
                job.force,
            )?;
            Ok(format!("Unpacked package into {}", output.display()))
        }
        Operation::UpdateList => format_update_list(&input, layout),
        Operation::UpdateUnpack => {
            let output = required_output(&job)?;
            let components = update_bin::unpack_file(&input, &output, layout, job.force)?;
            Ok(format!(
                "Extracted {} components into {}",
                components.len(),
                output.display()
            ))
        }
        Operation::ErofsUnpack => {
            let output = required_output(&job)?;
            erofs::unpack_with_tools(&input, &output, &discover_tools(&job)?, job.force)?;
            Ok(format!("Unpacked EROFS image into {}", output.display()))
        }
        Operation::ErofsRepack => {
            let output = required_output(&job)?;
            erofs::repack_with_tools(&input, &output, &discover_tools(&job)?, job.allow_grow)?;
            Ok(format!("Repacked EROFS image to {}", output.display()))
        }
        Operation::RamdiskUnpack => {
            let output = required_output(&job)?;
            prepare_output_dir(&output, job.force)?;
            let input = canonical_path(&input)?;
            ramdisk::unpack(&input, &output)?;
            Ok(format!("Unpacked ramdisk into {}", output.display()))
        }
        Operation::RamdiskRepack => {
            let output = required_output(&job)?;
            let original = canonical_path(&required_path(&job.secondary, "Original image")?)?;
            let output_path = absolute_output(&output)?;
            ramdisk::repack(&input, &original, &output_path)?;
            Ok(format!("Repacked ramdisk image to {}", output.display()))
        }
        Operation::RamdiskPatch => {
            let output = required_output(&job)?;
            ramdisk::patch(
                &input,
                &required_path(&job.secondary, "Replacement binary")?,
                &absolute_output(&output)?,
            )?;
            Ok(format!("Patched ramdisk image to {}", output.display()))
        }
    }
}

fn format_update_list(input: &Path, layout: UpdateLayout) -> Result<String> {
    let file =
        File::open(input).with_context(|| format!("opening update package {}", input.display()))?;
    let length = file.metadata()?.len();
    let mut reader = BufReader::new(file);
    let index = update_bin::read_index(&mut reader, Some(length), layout)?;
    let mut report = format!(
        "layout={:?} components={} data_offset={}\n",
        index.layout,
        index.components.len(),
        index.data_offset
    );
    for (number, component) in index.components.iter().enumerate() {
        report.push_str(&format!(
            "{:>3}  {:<36} type={} size={} offset={}\n",
            number + 1,
            component.output_name,
            component.component_type,
            component.size,
            component.data_offset
        ));
    }
    Ok(report)
}

fn prepare_output_dir(output: &Path, force: bool) -> Result<()> {
    if output.exists() && fs::read_dir(output)?.next().is_some() {
        ensure!(force, "output directory is not empty: {}", output.display());
        fs::remove_dir_all(output)
            .with_context(|| format!("removing old output directory {}", output.display()))?;
    }
    fs::create_dir_all(output)?;
    Ok(())
}

fn discover_tools(job: &Job) -> Result<ToolPaths> {
    ToolPaths::discover(optional_path(&job.tools_dir))
}

fn required_path(value: &str, label: &str) -> Result<PathBuf> {
    let trimmed = value.trim();
    ensure!(!trimmed.is_empty(), "{label} is required");
    Ok(PathBuf::from(trimmed))
}

fn required_output(job: &Job) -> Result<PathBuf> {
    required_path(
        &job.output,
        job.operation.output_label().unwrap_or("Output path"),
    )
}

fn optional_path(value: &str) -> Option<PathBuf> {
    (!value.trim().is_empty()).then(|| PathBuf::from(value.trim()))
}

fn parse_partitions(value: &str) -> Vec<String> {
    value
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

fn canonical_path(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("resolving {}", path.display()))
}

fn absolute_output(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_owned());
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .context("output path must include a file name")?;
    Ok(canonical_path(parent)?.join(file_name))
}
