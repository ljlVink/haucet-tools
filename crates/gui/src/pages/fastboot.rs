use crate::app::HaucetApp;
use crate::pages::{Page, ResultView, run_button};
use crate::util::{kv, message_box, section};
use eframe::egui;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FastbootDeviceInfo {
    pub bus: String,
    pub addr: u8,
    pub vid: String,
    pub pid: String,
    pub product: String,
    pub serial: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FastbootStatusPayload {
    pub connected: bool,
    #[serde(default)]
    pub devices: Vec<FastbootDeviceInfo>,
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingOp {
    Status,
    Reboot,
    Flash,
}

#[derive(Debug, Default)]
pub struct FastbootPage {
    pub status: Option<FastbootStatusPayload>,
    pub status_error: Option<String>,
    pub auto_checked: bool,
    pub image: String,
    pub target: String,
    pub result: Option<ResultView>,
    pub reboot_result: Option<ResultView>,
    pending: Option<PendingOp>,
}

impl FastbootPage {
    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        self.poll_result(app);
        if !self.auto_checked && !app.job_running() {
            self.auto_checked = true;
            self.start_status(app);
        }

        egui::ScrollArea::vertical()
            .id_salt("fastboot-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_space(6.0);
                ui.label(egui::RichText::new("HarmonyOS/Android Fastboot 刷机").weak());
                ui.add_space(6.0);

                self.status_section(ui, app);
                ui.add_space(10.0);
                self.flash_section(ui, app);
                ui.add_space(20.0);
            });
    }

    fn status_section(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        section(ui, "Fastboot 设备");
        ui.horizontal(|ui| {
            if run_button(
                ui,
                "检测设备连接",
                !app.job_running(),
                Some("重新枚举 USB 设备并查询 fastboot 变量"),
            )
            .clicked()
            {
                self.start_status(app);
            }
            if run_button(
                ui,
                "重启设备",
                !app.job_running(),
                Some("向已连接的 fastboot 设备发送 reboot 命令"),
            )
            .clicked()
            {
                self.start_reboot(app);
            }
            if app.job_running() {
                ui.add(egui::Spinner::new().size(16.0));
                let text = match self.pending {
                    Some(PendingOp::Status) => "正在检测…",
                    Some(PendingOp::Reboot) => "正在重启…",
                    Some(PendingOp::Flash) | None => "任务运行中…",
                };
                ui.label(egui::RichText::new(text).weak());
            }
        });
        ui.add_space(6.0);

        if let Some(result) = &self.reboot_result {
            let color = if result.ok {
                egui::Color32::from_rgb(90, 200, 120)
            } else {
                egui::Color32::from_rgb(230, 90, 90)
            };
            message_box(ui, color, &result.summary);
            ui.add_space(6.0);
        }

        if let Some(error) = &self.status_error {
            message_box(ui, egui::Color32::from_rgb(230, 90, 90), error);
            return;
        }
        let Some(status) = &self.status else {
            ui.label(egui::RichText::new("尚未检测, 点击上方按钮开始。").weak());
            return;
        };
        if !status.connected {
            message_box(
                ui,
                egui::Color32::from_rgb(230, 170, 40),
                "未检测到 fastboot 设备",
            );
            return;
        }

        ui.horizontal(|ui| {
            crate::pages::badge_text(ui, "已连接", egui::Color32::from_rgb(90, 200, 120));
            if let Some(device) = status.devices.first() {
                ui.label(
                    egui::RichText::new(format!(
                        "{} ({}:{})",
                        device.product, device.bus, device.addr
                    ))
                    .strong(),
                );
            }
        });
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                if let Some(device) = status.devices.first() {
                    egui::Grid::new("fastboot-device-grid")
                        .num_columns(2)
                        .spacing([18.0, 6.0])
                        .show(ui, |ui| {
                            kv(ui, "产品", &device.product);
                            kv(ui, "序列号", &device.serial);
                            kv(ui, "USB 地址", format!("{}:{}", device.bus, device.addr));
                            kv(ui, "VID:PID", format!("{}:{}", device.vid, device.pid));
                        });
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);
                }
                if status.vars.is_empty() {
                    ui.label(egui::RichText::new("设备未返回 fastboot 变量。").weak());
                } else {
                    egui::Grid::new("fastboot-vars-grid")
                        .num_columns(2)
                        .spacing([18.0, 6.0])
                        .show(ui, |ui| {
                            for (key, value) in &status.vars {
                                kv(ui, key, value);
                            }
                        });
                }
            });
    }

    fn flash_section(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        section(ui, "刷写镜像");
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("镜像文件").strong());
            let image_response = ui.add(
                egui::TextEdit::singleline(&mut self.image)
                    .hint_text("镜像文件路径或拖放文件到这里")
                    .desired_width(ui.available_width() - 170.0),
            );
            if image_response.changed()
                && let Some(target) = partition_name_from_image(Path::new(self.image.trim()))
            {
                self.target = target;
            }
            if ui.button("选择镜像…").clicked()
                && let Some(path) = app.pick_file("选择镜像文件", &[("镜像文件", &["img", "bin"])])
            {
                self.set_image(&path);
            }
        });
        let drops = app.take_drops(ui.ctx());
        if let Some(path) = drops.first() {
            self.set_image(path);
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("目标分区").strong());
            ui.add(
                egui::TextEdit::singleline(&mut self.target)
                    .hint_text("分区名, 例如 updater / ramdisk / vendor")
                    .desired_width(ui.available_width() - 170.0),
            );
        });

        ui.label(
            egui::RichText::new("⚠ 刷写会覆盖设备上的分区数据, 请确认分区名和镜像正确。")
                .color(egui::Color32::from_rgb(230, 170, 40)),
        );
        ui.add_space(6.0);
        let ready =
            !app.job_running() && !self.image.trim().is_empty() && !self.target.trim().is_empty();
        if run_button(
            ui,
            "刷写镜像",
            ready,
            Some("把镜像刷写到目标分区 (支持 raw 和 Android sparse 格式)"),
        )
        .clicked()
        {
            self.result = None;
            self.pending = Some(PendingOp::Flash);
            app.start_job(crate::worker::JobOp::FastbootFlash {
                image: self.image.trim().to_owned(),
                target: self.target.trim().to_owned(),
            });
        }
        if app.job_running() {
            ui.label(egui::RichText::new("任务运行中…").weak());
        }

        ui.add_space(10.0);
        if let Some(result) = &self.result {
            if result.ok {
                message_box(ui, egui::Color32::from_rgb(90, 200, 120), &result.summary);
            } else {
                message_box(ui, egui::Color32::from_rgb(230, 90, 90), &result.summary);
            }
        }
    }

    fn poll_result(&mut self, app: &mut HaucetApp) {
        let Some(result) = app.take_result(Page::Fastboot) else {
            return;
        };
        let op = self.pending.take().unwrap_or(PendingOp::Status);
        match op {
            PendingOp::Status => {
                self.result = None;
                if !result.ok {
                    self.status = None;
                    self.status_error = Some(result.summary);
                } else if let Some(payload) = result.payload {
                    match serde_json::from_value::<FastbootStatusPayload>(payload) {
                        Ok(status) => {
                            self.status = Some(status);
                            self.status_error = None;
                        }
                        Err(error) => {
                            self.status = None;
                            self.status_error = Some(format!("解析检测结果失败: {error}"));
                        }
                    }
                }
            }
            PendingOp::Reboot => {
                self.reboot_result = Some(ResultView {
                    ok: result.ok,
                    summary: result.summary,
                    output: String::new(),
                });
                if result.ok {
                    self.status = None;
                    self.status_error = None;
                }
            }
            PendingOp::Flash => {
                self.result = Some(ResultView {
                    ok: result.ok,
                    summary: result.summary,
                    output: String::new(),
                });
            }
        }
    }

    fn start_status(&mut self, app: &mut HaucetApp) {
        self.status_error = None;
        self.reboot_result = None;
        self.pending = Some(PendingOp::Status);
        app.start_job(crate::worker::JobOp::FastbootStatus {});
    }

    fn start_reboot(&mut self, app: &mut HaucetApp) {
        self.reboot_result = None;
        self.pending = Some(PendingOp::Reboot);
        app.start_job(crate::worker::JobOp::FastbootReboot {});
    }

    fn set_image(&mut self, path: &Path) {
        self.image = path.display().to_string();
        if let Some(target) = partition_name_from_image(path) {
            self.target = target;
        }
    }
}

fn partition_name_from_image(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?.trim();
    if file_name.eq_ignore_ascii_case("ptable") {
        return Some("ptable".to_owned());
    }

    let extension = path.extension()?.to_str()?;
    if !extension.eq_ignore_ascii_case("img") && !extension.eq_ignore_ascii_case("bin") {
        return None;
    }

    let stem = path.file_stem()?.to_str()?.trim();
    if stem.is_empty() {
        return None;
    }
    Some(stem.to_owned())
}
