use anyhow::{Context, Result, ensure};
use common::formats::update_bin::{self, UpdateLayout};
use common::formats::{cpio, erofs, header::check_fmt};
use common::tools::ToolPaths;
use common::{package, partition, ramdisk};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const WORKER_ENV: &str = "HAUCET_GUI_WORKER";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum JobOp {
    PackageInspect {
        input: String,
        layout: String,
    },
    PackageUnpack {
        input: String,
        output: String,
        partitions: Vec<String>,
        all_erofs: bool,
        layout: String,
        force: bool,
        tools_dir: Option<String>,
    },
    UpdateList {
        input: String,
        layout: String,
    },
    UpdateUnpack {
        input: String,
        output: String,
        layout: String,
        force: bool,
        selected: Vec<String>,
    },
    ErofsUnpack {
        image: String,
        output: String,
        force: bool,
        tools_dir: Option<String>,
    },
    ErofsRepack {
        workspace: String,
        output: String,
        allow_grow: bool,
        tools_dir: Option<String>,
    },
    RamdiskUnpack {
        image: String,
        output: String,
        force: bool,
    },
    RamdiskRepack {
        workspace: String,
        original: String,
        output: String,
    },
    RamdiskPatch {
        image: String,
        binary: String,
        output: String,
    },
    RamdiskProbe {
        image: String,
    },
    PartitionInfo {
        image: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSpec {
    pub op: JobOp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerResult {
    pub ok: bool,
    pub summary: String,
    pub payload: Option<serde_json::Value>,
}

pub fn is_worker_mode() -> bool {
    std::env::var_os(WORKER_ENV).is_some()
}

pub fn run_worker() -> i32 {
    let mut input = String::new();
    let read = std::io::stdin().read_to_string(&mut input);
    let result = match read {
        Ok(_) => serde_json::from_str::<JobSpec>(&input)
            .context("worker: invalid job spec on stdin")
            .and_then(|spec| execute(&spec.op)),
        Err(e) => Err(e).context("worker: reading job spec from stdin"),
    };
    let result = match result {
        Ok(result) => result,
        Err(e) => WorkerResult {
            ok: false,
            summary: format!("{e:#}"),
            payload: None,
        },
    };
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "t": "result",
            "ok": result.ok,
            "summary": result.summary,
            "payload": result.payload,
        }))
        .expect("serializing worker result")
    );
    0
}

fn execute(op: &JobOp) -> Result<WorkerResult> {
    match op {
        JobOp::PackageInspect { input, layout } => {
            let index = package::inspect(Path::new(input), parse_layout(layout)?)?;
            summary_payload(
                format!(
                    "包内共 {} 个组件（检测到 {} 布局）",
                    index.components.len(),
                    layout_label(index.layout)
                ),
                index,
            )
        }
        JobOp::PackageUnpack {
            input,
            output,
            partitions,
            all_erofs,
            layout,
            force,
            tools_dir,
        } => {
            let tools = discover_tools(tools_dir)?;
            package::unpack_full_with_tools(
                Path::new(input),
                Path::new(output),
                &tools,
                partitions,
                *all_erofs,
                parse_layout(layout)?,
                *force,
            )?;
            Ok(WorkerResult {
                ok: true,
                summary: format!("更新包已解包到 {}", output),
                payload: None,
            })
        }
        JobOp::UpdateList { input, layout } => {
            let file = fs::File::open(input).with_context(|| format!("打开 {}", input))?;
            let length = file.metadata()?.len();
            let index = update_bin::read_index(
                &mut std::io::BufReader::new(file),
                Some(length),
                parse_layout(layout)?,
            )?;
            summary_payload(
                format!(
                    "共 {} 个组件（检测到 {} 布局）",
                    index.components.len(),
                    layout_label(index.layout)
                ),
                index,
            )
        }
        JobOp::UpdateUnpack {
            input,
            output,
            layout,
            force,
            selected,
        } => {
            let count = if selected.is_empty() {
                update_bin::unpack_file(
                    Path::new(input),
                    Path::new(output),
                    parse_layout(layout)?,
                    *force,
                )?
                .len()
            } else {
                update_bin::unpack_selected_file(
                    Path::new(input),
                    Path::new(output),
                    selected,
                    parse_layout(layout)?,
                    *force,
                )?
                .len()
            };
            Ok(WorkerResult {
                ok: true,
                summary: format!("已解包 {count} 个组件到 {output}"),
                payload: Some(serde_json::json!({ "count": count })),
            })
        }
        JobOp::ErofsUnpack {
            image,
            output,
            force,
            tools_dir,
        } => {
            let tools = discover_tools(tools_dir)?;
            erofs::unpack_with_tools(Path::new(image), Path::new(output), &tools, *force)?;
            let manifest = erofs::read_manifest(Path::new(output))?;
            summary_payload(
                format!(
                    "EROFS 镜像已解包到 {}（分区 {}）",
                    output, manifest.partition
                ),
                manifest,
            )
        }
        JobOp::ErofsRepack {
            workspace,
            output,
            allow_grow,
            tools_dir,
        } => {
            let tools = discover_tools(tools_dir)?;
            erofs::repack_with_tools(Path::new(workspace), Path::new(output), &tools, *allow_grow)?;
            Ok(WorkerResult {
                ok: true,
                summary: format!("已重新打包 EROFS 镜像到 {output}"),
                payload: None,
            })
        }
        JobOp::RamdiskUnpack {
            image,
            output,
            force,
        } => {
            prepare_output_dir(Path::new(output), *force)?;
            let image = canonical_path(Path::new(image))?;
            ramdisk::unpack(&image, Path::new(output))?;
            Ok(WorkerResult {
                ok: true,
                summary: format!("Ramdisk 已解包到 {}", output),
                payload: None,
            })
        }
        JobOp::RamdiskRepack {
            workspace,
            original,
            output,
        } => {
            let workspace = canonical_path(Path::new(workspace))?;
            let original = canonical_path(Path::new(original))?;
            let output = absolute_path(Path::new(output))?;
            ramdisk::repack(&workspace, &original, &output)?;
            Ok(WorkerResult {
                ok: true,
                summary: format!("Ramdisk 镜像已生成到 {}", output.display()),
                payload: None,
            })
        }
        JobOp::RamdiskPatch {
            image,
            binary,
            output,
        } => {
            let output = absolute_path(Path::new(output))?;
            ramdisk::patch(Path::new(image), Path::new(binary), &output)?;
            Ok(WorkerResult {
                ok: true,
                summary: format!("补丁已写入 {}", output.display()),
                payload: None,
            })
        }
        JobOp::RamdiskProbe { image } => {
            let payload = probe_ramdisk(Path::new(image))?;
            let summary = match (
                payload["patched"].as_bool().unwrap_or(false),
                payload["layout_known"].as_bool().unwrap_or(false),
            ) {
                (true, _) => "该镜像已打过补丁（存在 .backup/init_early）".to_owned(),
                (false, true) => "原厂镜像, 可以打补丁".to_owned(),
                _ => "未识别的 ramdisk 布局".to_owned(),
            };
            Ok(WorkerResult {
                ok: true,
                summary,
                payload: Some(payload),
            })
        }
        JobOp::PartitionInfo { image } => {
            let summary = partition::summarize(Path::new(image))?;
            let label = match &summary {
                partition::PartitionSummary::Harmony(h) => {
                    format!("HARMONY! 分区镜像（{}）", h.cert.partition_name)
                }
                partition::PartitionSummary::Rvt(info) => {
                    format!("RVT 密钥镜像（{} 个描述符）", info.descriptors.len())
                }
                partition::PartitionSummary::HvbWrapped { .. } => "HVB 包装的分区镜像".to_owned(),
            };
            summary_payload(label, summary)
        }
    }
}

fn probe_ramdisk(image: &Path) -> Result<serde_json::Value> {
    let frame = common::formats::harmony::HvbFrame::load(image)
        .with_context(|| format!("读取 {}", image.display()))?;
    let payload = frame.extract_image_payload();
    ensure!(!payload.is_empty(), "镜像内没有负载数据");
    let fmt = check_fmt(payload);
    let cpio_bytes = if fmt.is_compressed() {
        common::compress::decompress_vec(fmt, payload).map_err(std::io::Error::other)?
    } else {
        payload.to_vec()
    };
    let cpio = cpio::Cpio::load_from_data(&cpio_bytes)?;
    let patched = cpio.exists(".backup/init_early");
    let has_init = cpio.exists("bin/init_early") || cpio.exists("init");
    Ok(serde_json::json!({
        "patched": patched,
        "has_init_early": has_init,
        "layout_known": patched || has_init,
        "payload_format": fmt.as_str(),
        "payload_len": payload.len(),
        "cert_image_len": frame.cert.image_len,
        "cert_original_len": frame.cert.image_original_len,
    }))
}

fn summary_payload<T: Serialize>(summary: String, payload: T) -> Result<WorkerResult> {
    Ok(WorkerResult {
        ok: true,
        summary,
        payload: Some(serde_json::to_value(payload)?),
    })
}

fn parse_layout(value: &str) -> Result<UpdateLayout> {
    value.parse().map_err(|e: String| anyhow::anyhow!(e))
}

fn layout_label(layout: UpdateLayout) -> &'static str {
    match layout {
        UpdateLayout::Auto => "auto",
        UpdateLayout::L1 => "L1",
        UpdateLayout::L2 => "L2",
    }
}

fn discover_tools(tools_dir: &Option<String>) -> Result<ToolPaths> {
    let explicit = tools_dir
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from);
    ToolPaths::discover(explicit)
}

fn prepare_output_dir(output: &Path, force: bool) -> Result<()> {
    if output.exists() && fs::read_dir(output)?.next().is_some() {
        ensure!(force, "输出目录不是空的: {}", output.display());
        fs::remove_dir_all(output)
            .with_context(|| format!("删除旧输出目录 {}", output.display()))?;
    }
    fs::create_dir_all(output).with_context(|| format!("创建输出目录 {}", output.display()))
}

fn canonical_path(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("解析路径 {}", path.display()))
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}
