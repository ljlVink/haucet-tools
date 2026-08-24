use anyhow::{Context, Result, ensure};
use common::formats::update_bin::{self, UpdateLayout};
use common::formats::{cpio, erofs, header::check_fmt};
use common::tools::ToolPaths;
use common::{entropy, package, partition, ramdisk};
use hisi_vcom::transport::{self, DeviceFilter, SerialVcomDevice};
use hisi_vcom::vcom;
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
    FileEntropy {
        file: String,
    },
    FastbootStatus {},
    FastbootFlash {
        image: String,
        target: String,
    },
    VcomStatus {},
    VcomFlash {
        port: String,
        address: u32,
        file: String,
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
                    "包内共 {} 个组件(检测到 {} 布局)",
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
                    "共 {} 个组件(检测到 {} 布局)",
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
                format!("EROFS 镜像已解包到 {}(分区 {})", output, manifest.partition),
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
                (true, _) => "该镜像已打过补丁(存在 .backup/init_early)".to_owned(),
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
            let entropy_summary = entropy::analyze_file(Path::new(image))?;
            let partition_summary = partition::summarize(Path::new(image)).ok();
            let label = match &partition_summary {
                Some(partition::PartitionSummary::Harmony(h)) => {
                    format!("HARMONY! 分区镜像({})", h.cert.partition_name)
                }
                Some(partition::PartitionSummary::Rvt(info)) => {
                    format!("RVT 密钥镜像({} 个描述符)", info.descriptors.len())
                }
                Some(partition::PartitionSummary::HvbWrapped { .. }) => {
                    "HVB 包装的分区镜像".to_owned()
                }
                None => "未识别分区格式".to_owned(),
            };
            summary_payload(
                format!(
                    "{}；信息熵 {:.6} bits/byte ({:.2}%)",
                    label,
                    entropy_summary.entropy_bits_per_byte,
                    entropy_summary.normalized_percent()
                ),
                serde_json::json!({
                    "partition": partition_summary,
                    "entropy": entropy_summary,
                }),
            )
        }
        JobOp::FileEntropy { file } => {
            let summary = entropy::analyze_file(Path::new(file))?;
            summary_payload(
                format!(
                    "信息熵 {:.6} bits/byte ({:.2}%)",
                    summary.entropy_bits_per_byte,
                    summary.normalized_percent()
                ),
                summary,
            )
        }
        JobOp::FastbootStatus {} => fastboot_status(),
        JobOp::FastbootFlash { image, target } => {
            ensure!(!target.trim().is_empty(), "分区名不能为空");
            fastboot_flash(Path::new(image), target.trim())
        }
        JobOp::VcomStatus {} => vcom_status(),
        JobOp::VcomFlash {
            port,
            address,
            file,
        } => vcom_flash(port.trim(), *address, Path::new(file)),
    }
}

fn vcom_status() -> Result<WorkerResult> {
    let ports = transport::list_serial_ports()?
        .into_iter()
        .map(|port| {
            serde_json::json!({
                "name": port.name,
                "description": port.description,
            })
        })
        .collect::<Vec<_>>();
    let filter = DeviceFilter {
        vid: Some(0x12D1),
        ..Default::default()
    };
    let usb = transport::list_candidates(&filter)?;
    let serial_count = ports.len();
    let usb_count = usb.len();

    Ok(WorkerResult {
        ok: true,
        summary: format!("VCOM serial ports: {serial_count}, USB candidates: {usb_count}"),
        payload: Some(serde_json::json!({
            "ports": ports,
            "usb": usb,
        })),
    })
}

fn vcom_flash(port: &str, address: u32, file: &Path) -> Result<WorkerResult> {
    ensure!(!port.is_empty(), "VCOM port cannot be empty");
    let data = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let mut device = SerialVcomDevice::open(port, 115200)
        .with_context(|| format!("opening VCOM port {port}"))?;
    let mut log = |message: &str| emit_log(message);

    vcom::upload(
        &mut device,
        &data,
        address,
        &mut log,
        &mut |sent, total| {
            if total > 0 && (sent == total || sent % (total / 10 + 1) == 0) {
                emit_log(&format!("Progress: {sent}/{total} bytes"));
            }
        },
    )?;

    Ok(WorkerResult {
        ok: true,
        summary: format!(
            "VCOM flash finished: {} -> {port} at 0x{address:08X}",
            file.display()
        ),
        payload: Some(serde_json::json!({
            "port": port,
            "address": format!("0x{address:08X}"),
            "bytes": data.len(),
        })),
    })
}

fn fastboot_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("创建 fastboot 异步运行时失败")
}

fn emit_log(text: &str) {
    let line = serde_json::to_string(&serde_json::json!({ "t": "log", "s": text }))
        .expect("serializing log line");
    println!("{line}");
}

fn clean_device_string(s: &str) -> Option<String> {
    hm_fastboot::nusb::clean_device_string(s)
}

fn fastboot_status() -> Result<WorkerResult> {
    let runtime = fastboot_runtime()?;
    runtime.block_on(async {
        use hm_fastboot::nusb::{DeviceInfo, NusbFastBoot};
        let devices = hm_fastboot::nusb::devices()
            .await
            .context("枚举 USB 设备失败")?;

        let mut list = Vec::new();
        let mut first: Option<DeviceInfo> = None;
        for info in devices {
            list.push(device_json(&info));
            if first.is_none() {
                first = Some(info);
            }
        }

        let Some(info) = first else {
            return Ok(WorkerResult {
                ok: true,
                summary: "未检测到 fastboot 设备".to_owned(),
                payload: Some(serde_json::json!({
                    "connected": false,
                    "devices": list,
                })),
            });
        };

        let mut vars = serde_json::Map::new();
        match NusbFastBoot::from_info(&info).await {
            Ok(mut fb) => {
                for var in ["product", "serialno", "version", "max-download-size"] {
                    match fb.get_var(var).await {
                        Ok(value) => {
                            vars.insert(var.to_owned(), serde_json::Value::String(value));
                        }
                        Err(error) => emit_log(&format!("getvar:{var} 失败: {error}")),
                    }
                }
            }
            Err(error) => emit_log(&format!("打开设备失败: {error:#}")),
        }

        let product = vars
            .get("product")
            .and_then(|v| v.as_str())
            .unwrap_or("未知设备")
            .to_owned();
        Ok(WorkerResult {
            ok: true,
            summary: format!("已连接 fastboot 设备: {product}"),
            payload: Some(serde_json::json!({
                "connected": true,
                "devices": list,
                "vars": vars,
            })),
        })
    })
}

fn device_json(info: &hm_fastboot::nusb::DeviceInfo) -> serde_json::Value {
    serde_json::json!({
        "bus": info.bus_id(),
        "addr": info.device_address(),
        "vid": format!("{:04x}", info.vendor_id()),
        "pid": format!("{:04x}", info.product_id()),
        "product": info
            .product_string()
            .map(|s| clean_device_string(s).unwrap_or_else(|| s.to_owned()))
            .unwrap_or_default(),
        "serial": info
            .serial_number()
            .map(|s| clean_device_string(s).unwrap_or_else(|| s.to_owned()))
            .unwrap_or_default(),
    })
}

fn fastboot_flash(image: &Path, target: &str) -> Result<WorkerResult> {
    let runtime = fastboot_runtime()?;
    runtime.block_on(async {
        use hm_fastboot::nusb::{FlashEvent, NusbFastBoot};
        let mut devices = hm_fastboot::nusb::devices()
            .await
            .context("枚举 USB 设备失败")?;
        let info = devices.next().ok_or_else(|| {
            anyhow::anyhow!("未检测到 fastboot 设备: 请确认设备已进入 fastboot 模式并连接 USB")
        })?;
        let mut fb = NusbFastBoot::from_info(&info)
            .await
            .context("打开 fastboot 设备失败 (可能需要管理员权限或 WinUSB 驱动)")?;

        let mut progress = |event: FlashEvent<'_>| match event {
            FlashEvent::Message(msg) => emit_log(msg),
            FlashEvent::Part { index, total } => {
                emit_log(&format!("进度: {index}/{total} 部分完成"));
            }
        };
        fb.flash_image(target, image, &mut progress)
            .await
            .with_context(|| format!("刷写 {} 到 {} 失败", image.display(), target))?;
        Ok(WorkerResult {
            ok: true,
            summary: format!("已刷写 {} 到分区 {}", image.display(), target),
            payload: None,
        })
    })
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
