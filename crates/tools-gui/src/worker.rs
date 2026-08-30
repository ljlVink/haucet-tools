use anyhow::{Context, Result, ensure};
use common::formats::{cpio, erofs, header::check_fmt_full};
use common::package::UpdateLayout;
use common::tools::ToolPaths;
use common::{entropy, fs_util, nvme, package, partition, ramdisk};
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
        layout: UpdateLayout,
    },
    PackageUnpack {
        input: String,
        output: String,
        partitions: Vec<String>,
        all_erofs: bool,
        layout: UpdateLayout,
        force: bool,
        tools_dir: Option<String>,
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
    NvmeInspect {
        image: String,
    },
    NvmeEdit {
        image: String,
        key: String,
        value: String,
        value_format: String,
    },
    FastbootStatus {},
    FastbootReboot {},
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
            let index = package::inspect(Path::new(input), *layout)?;
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
                *layout,
                *force,
            )?;
            Ok(WorkerResult {
                ok: true,
                summary: format!("更新包已解包到 {}", output),
                payload: None,
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
            let image = fs_util::canonical_path(Path::new(image))?;
            fs_util::prepare_dir_excluding(
                Path::new(output),
                "output directory",
                *force,
                &[&image],
            )?;
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
            let workspace = fs_util::canonical_path(Path::new(workspace))?;
            let original = fs_util::canonical_path(Path::new(original))?;
            let output = fs_util::absolute_path(Path::new(output))?;
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
            let output = fs_util::absolute_path(Path::new(output))?;
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
                Some(partition::PartitionSummary::Gpt(info)) => {
                    format!(
                        "GPT 分区表({} 个表，{} 个分区)",
                        info.tables.len(),
                        info.partition_count()
                    )
                }
                Some(partition::PartitionSummary::SecImage(info)) => format!(
                    "Huawei 安全镜像({} -> {})",
                    info.image_name, info.partition_name
                ),
                Some(partition::PartitionSummary::HvbWrapped { .. }) => {
                    "HVB 包装的分区镜像".to_owned()
                }
                None => "未识别分区格式".to_owned(),
            };
            summary_payload(
                format!(
                    "{}; 信息熵 {:.6} bits/byte ({:.2}%)",
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
        JobOp::NvmeInspect { image } => {
            let summary = nvme::inspect(Path::new(image))?;
            let crc_status = if summary.crc_supported {
                format!("CRC 错误 {} 个", summary.crc_invalid)
            } else {
                "CRC 未启用".to_owned()
            };
            summary_payload(
                format!(
                    "NVE/NVME: {} 个活动副本, {} 个条目, {}",
                    summary.active_blocks, summary.valid_items, crc_status
                ),
                summary,
            )
        }
        JobOp::NvmeEdit {
            image,
            key,
            value,
            value_format,
        } => {
            let result = nvme::edit_file_in_place(Path::new(image), key, value, value_format)?;
            summary_payload(
                format!(
                    "已从 NVE 副本 {} 向副本 {} 提交 {} 个条目（代次 {}），备份已创建: {}",
                    result.source_block,
                    result.committed_block,
                    result.updated_items,
                    result.age,
                    result.backup_path
                ),
                result,
            )
        }
        JobOp::FastbootStatus {} => fastboot_status(),
        JobOp::FastbootReboot {} => fastboot_reboot(),
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

    vcom::upload(&mut device, &data, address, &mut log, &mut |sent, total| {
        if total > 0 && (sent == total || sent % (total / 10 + 1) == 0) {
            emit_log(&format!("Progress: {sent}/{total} bytes"));
        }
    })?;

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

fn fastboot_status() -> Result<WorkerResult> {
    let runtime = fastboot_runtime()?;
    runtime.block_on(async {
        use hm_fastboot::nusb::{DeviceSelectionError, NusbFastBoot, require_single_device};
        let devices: Vec<_> = hm_fastboot::nusb::devices()
            .await
            .context("枚举 USB 设备失败")?
            .collect();
        let list: Vec<_> = devices.iter().map(device_json).collect();
        let info = match require_single_device(devices.into_iter()) {
            Ok(info) => info,
            Err(DeviceSelectionError::NotFound) => {
                return Ok(WorkerResult {
                    ok: true,
                    summary: "未检测到 fastboot 设备".to_owned(),
                    payload: Some(serde_json::json!({
                        "connected": false,
                        "devices": list,
                    })),
                });
            }
            Err(DeviceSelectionError::Multiple) => {
                return Ok(WorkerResult {
                    ok: true,
                    summary: "检测到多个 fastboot 设备，已拒绝选择目标".to_owned(),
                    payload: Some(serde_json::json!({
                        "connected": false,
                        "devices": list,
                    })),
                });
            }
        };

        let mut vars = serde_json::Map::new();
        let opened = match NusbFastBoot::from_info(&info).await {
            Ok(mut fb) => {
                for var in ["product", "serialno", "version", "max-download-size"] {
                    match fb.get_var(var).await {
                        Ok(value) => {
                            vars.insert(var.to_owned(), serde_json::Value::String(value));
                        }
                        Err(error) => emit_log(&format!("getvar:{var} 失败: {error}")),
                    }
                }
                true
            }
            Err(error) => {
                emit_log(&format!("打开设备失败: {error:#}"));
                false
            }
        };

        let product = vars
            .get("product")
            .and_then(|v| v.as_str())
            .unwrap_or("未知设备")
            .to_owned();
        Ok(WorkerResult {
            ok: true,
            summary: if opened {
                format!("已连接 fastboot 设备: {product}")
            } else {
                "检测到 fastboot 设备，但无法打开".to_owned()
            },
            payload: Some(serde_json::json!({
                "connected": opened,
                "devices": list,
                "vars": vars,
            })),
        })
    })
}

fn fastboot_reboot() -> Result<WorkerResult> {
    let runtime = fastboot_runtime()?;
    runtime.block_on(async {
        use hm_fastboot::nusb::NusbFastBoot;

        let devices = hm_fastboot::nusb::devices()
            .await
            .context("枚举 USB 设备失败")?;
        let info = single_fastboot_device(devices)?;
        let mut fb = NusbFastBoot::from_info(&info)
            .await
            .context("打开 fastboot 设备失败 (可能需要管理员权限或 WinUSB 驱动)")?;
        fb.reboot().await.context("发送 fastboot 重启命令失败")?;

        Ok(WorkerResult {
            ok: true,
            summary: "已发送设备重启命令".to_owned(),
            payload: None,
        })
    })
}

fn device_json(info: &hm_fastboot::nusb::DeviceInfo) -> serde_json::Value {
    use hm_fastboot::nusb::clean_device_string;

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
        let devices = hm_fastboot::nusb::devices()
            .await
            .context("枚举 USB 设备失败")?;
        let info = single_fastboot_device(devices)?;
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

fn single_fastboot_device<T>(devices: impl Iterator<Item = T>) -> Result<T> {
    use hm_fastboot::nusb::{DeviceSelectionError, require_single_device};

    require_single_device(devices).map_err(|error| match error {
        DeviceSelectionError::NotFound => {
            anyhow::anyhow!("未检测到 fastboot 设备: 请确认设备已进入 fastboot 模式并连接 USB")
        }
        DeviceSelectionError::Multiple => {
            anyhow::anyhow!("检测到多个 fastboot 设备: 请断开其他设备后重试")
        }
    })
}

fn probe_ramdisk(image: &Path) -> Result<serde_json::Value> {
    let frame = common::formats::harmony::HvbFrame::load(image)
        .with_context(|| format!("读取 {}", image.display()))?;
    let payload = frame.extract_image_payload();
    ensure!(!payload.is_empty(), "镜像内没有负载数据");
    let fmt = check_fmt_full(payload);
    let cpio_bytes = if fmt.is_compressed() {
        common::compress::decompress_vec(fmt, payload).map_err(std::io::Error::other)?
    } else {
        payload.to_vec()
    };
    let cpio = cpio::Cpio::load_from_data(&cpio_bytes)?;
    let patch_status = ramdisk::patch_status(&cpio);
    let patched = patch_status == ramdisk::RamdiskPatchStatus::Patched;
    let has_init_early = cpio.exists("bin/init_early");
    Ok(serde_json::json!({
        "patched": patched,
        "has_init_early": has_init_early,
        "layout_known": patch_status != ramdisk::RamdiskPatchStatus::Unsupported,
        "payload_format": fmt.as_str(),
        "payload_len": payload.len(),
        "header_size": frame.harmony.hdr_size,
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
