use crate::app::HaucetApp;
use crate::pages::images::ImageKind;
use crate::pages::{ResultView, badge_text, run_button};
use crate::util::{
    human_size, message_box, open_in_file_manager, sibling_output_path, update_derived_path,
};
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RamdiskTab {
    #[default]
    Unpack,
    Repack,
    Patch,
}

#[derive(Debug)]
enum PendingOp {
    Unpack { output: String },
    Repack { output: String },
    Patch { output: String },
    Probe { image: String },
}

#[derive(Debug, Default)]
pub struct RamdiskPage {
    pub tab: RamdiskTab,
    pub unpack: UnpackState,
    pub repack: RepackState,
    pub patch: PatchState,
    pub result: Option<ResultView>,
    pending: Option<PendingOp>,
    probe_requested: bool,
}

#[derive(Debug, Default)]
pub struct UnpackState {
    pub image: String,
    pub output: String,
    pub force: bool,
    auto_output: Option<String>,
}

#[derive(Debug, Default)]
pub struct RepackState {
    pub workspace: String,
    pub original: String,
    pub output: String,
}

#[derive(Debug, Default)]
pub struct PatchState {
    pub image: String,
    pub binary: String,
    pub output: String,
    pub probe: Option<ProbeInfo>,
    pub probe_error: Option<String>,
    auto_output: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProbeInfo {
    pub patched: bool,
    pub has_init_early: bool,
    pub layout_known: bool,
    pub payload_format: String,
    pub payload_len: u64,
    pub header_size: u64,
    pub cert_original_len: u64,
}

impl RamdiskPage {
    pub fn select_unpack_image(&mut self, image: String) {
        self.unpack.image = image;
        self.update_unpack_output();
    }

    pub fn select_patch_image(&mut self, image: String) {
        self.patch.image = image;
        self.patch.probe = None;
        self.patch.probe_error = None;
        self.update_patch_output();
        self.probe_requested = !self.patch.image.trim().is_empty();
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        self.poll_result(app);

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("操作").weak());
            ui.selectable_value(&mut self.tab, RamdiskTab::Unpack, "解包镜像");
            ui.selectable_value(&mut self.tab, RamdiskTab::Repack, "重建镜像");
            ui.selectable_value(&mut self.tab, RamdiskTab::Patch, "Patch init_early");
        });
        ui.add_space(8.0);

        match self.tab {
            RamdiskTab::Unpack => self.unpack_tab(ui, app),
            RamdiskTab::Repack => self.repack_tab(ui, app),
            RamdiskTab::Patch => self.patch_tab(ui, app),
        }

        ui.add_space(10.0);
        self.show_result(ui);
    }

    fn poll_result(&mut self, app: &mut HaucetApp) {
        let Some(result) = app.take_image_result(ImageKind::Ramdisk) else {
            return;
        };
        let Some(pending) = self.pending.take() else {
            return;
        };
        let output = match pending {
            PendingOp::Probe { image } => {
                if image != self.patch.image.trim() {
                    return;
                }
                if !result.ok {
                    self.patch.probe = None;
                    self.patch.probe_error = Some(result.summary);
                } else {
                    match result
                        .payload
                        .and_then(|payload| serde_json::from_value::<ProbeInfo>(payload).ok())
                    {
                        Some(probe) => {
                            self.patch.probe = Some(probe);
                            self.patch.probe_error = None;
                        }
                        None => {
                            self.patch.probe = None;
                            self.patch.probe_error = Some("无法解析 ramdisk 检测结果".to_owned());
                        }
                    }
                }
                return;
            }
            PendingOp::Unpack { output }
            | PendingOp::Repack { output }
            | PendingOp::Patch { output } => output,
        };
        self.result = Some(ResultView {
            ok: result.ok,
            summary: result.summary,
            output: if result.ok { output } else { String::new() },
        });
    }

    fn unpack_tab(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        ui.label(
            egui::RichText::new(
                "把 HARMONY! ramdisk 镜像解成 ramdisk.bin / ramdisk.cpio / header.json 工作区",
            )
            .weak(),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("镜像文件").strong());
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.unpack.image)
                    .hint_text("ramdisk 镜像或拖放文件到这里")
                    .desired_width(ui.available_width() - 240.0),
            );
            if response.changed() {
                self.update_unpack_output();
            }
            if ui.button("选择文件…").clicked()
                && let Some(path) = app.pick_file("选择 ramdisk 镜像", &[("镜像", &["img"])])
            {
                self.select_unpack_image(path.display().to_string());
            }
        });
        self.handle_drops(ui, app);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("输出目录").strong());
            ui.add(
                egui::TextEdit::singleline(&mut self.unpack.output)
                    .desired_width(ui.available_width() - 240.0),
            );
            if ui.button("选择目录…").clicked()
                && let Some(dir) = app.pick_dir("选择输出目录")
            {
                self.unpack.output = dir.display().to_string();
            }
        });
        ui.add_space(6.0);
        ui.checkbox(&mut self.unpack.force, "覆盖已存在目录");
        ui.add_space(8.0);
        let ready = !app.job_running()
            && !self.unpack.image.trim().is_empty()
            && !self.unpack.output.trim().is_empty();
        let output = self.unpack.output.trim().to_owned();
        if run_button(ui, "开始解包", ready, None).clicked() {
            self.pending = Some(PendingOp::Unpack {
                output: output.clone(),
            });
            self.result = None;
            app.start_job(crate::worker::JobOp::RamdiskUnpack {
                image: self.unpack.image.trim().to_owned(),
                output,
                force: self.unpack.force,
            });
        }
    }

    fn repack_tab(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        ui.label(
            egui::RichText::new(
                "用解包工作区里的 ramdisk.cpio 重建镜像(需要原始镜像以保留 HVB 头)",
            )
            .weak(),
        );
        ui.add_space(6.0);
        input_path_edit(
            ui,
            app,
            "工作区目录",
            &mut self.repack.workspace,
            true,
            "选择目录…",
        );
        ui.add_space(6.0);
        input_path_edit(
            ui,
            app,
            "原始镜像",
            &mut self.repack.original,
            false,
            "选择文件…",
        );
        ui.add_space(6.0);
        save_path_edit(
            ui,
            app,
            "输出镜像",
            &mut self.repack.output,
            "选择保存位置…",
            "ramdisk-repacked.img",
        );
        ui.add_space(8.0);
        let ready = !app.job_running()
            && !self.repack.workspace.trim().is_empty()
            && !self.repack.original.trim().is_empty()
            && !self.repack.output.trim().is_empty();
        let output = self.repack.output.trim().to_owned();
        if run_button(ui, "重新打包", ready, None).clicked() {
            self.pending = Some(PendingOp::Repack {
                output: output.clone(),
            });
            self.result = None;
            app.start_job(crate::worker::JobOp::RamdiskRepack {
                workspace: self.repack.workspace.trim().to_owned(),
                original: self.repack.original.trim().to_owned(),
                output,
            });
        }
    }

    fn patch_tab(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        ui.label(
            egui::RichText::new("把自制的 init_early 二进制替换进 ramdisk, 并自动腾出空间").weak(),
        );
        ui.add_space(4.0);

        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("ramdisk 镜像").strong());
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.patch.image)
                    .hint_text("原始 ramdisk 镜像")
                    .desired_width(ui.available_width() - 260.0),
            );
            if response.changed() {
                self.patch.probe = None;
                self.patch.probe_error = None;
                self.update_patch_output();
                self.probe_requested = std::path::Path::new(self.patch.image.trim()).is_file();
            }
            if ui.button("选择文件…").clicked()
                && let Some(path) = app.pick_file("选择 ramdisk 镜像", &[("镜像", &["img"])])
            {
                self.select_patch_image(path.display().to_string());
            }
        });
        let drops = app.take_drops(ui.ctx());
        if let Some(path) = drops.first() {
            self.select_patch_image(path.display().to_string());
        }
        self.start_probe(app);

        if let Some(probe) = &self.patch.probe {
            ui.add_space(6.0);
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("补丁状态").strong());
                        if probe.patched {
                            badge_text(ui, "已打过补丁", egui::Color32::from_rgb(230, 170, 40));
                            ui.label(
                                egui::RichText::new("再次打补丁会失败, 请使用原厂镜像").weak(),
                            );
                        } else if probe.layout_known {
                            badge_text(
                                ui,
                                "原厂镜像, 可以打补丁",
                                egui::Color32::from_rgb(90, 200, 120),
                            );
                        } else {
                            badge_text(ui, "未识别的布局", egui::Color32::from_rgb(230, 90, 90));
                        }
                    });
                    egui::Grid::new("probe-grid")
                        .num_columns(2)
                        .spacing([16.0, 6.0])
                        .show(ui, |ui| {
                            crate::util::kv(ui, "负载压缩格式", &probe.payload_format);
                            crate::util::kv(
                                ui,
                                "bin/init_early",
                                if probe.has_init_early {
                                    "存在"
                                } else {
                                    "不存在"
                                },
                            );
                            crate::util::kv(ui, "负载大小", human_size(probe.payload_len));
                            crate::util::kv(
                                ui,
                                "证书允许的最大镜像",
                                human_size(probe.cert_original_len),
                            );
                            let growth = payload_growth_space(
                                probe.cert_original_len,
                                probe.header_size,
                                probe.payload_len,
                            );
                            if growth > 0 {
                                crate::util::kv(ui, "可用增长空间", human_size(growth));
                            }
                        });
                });
        } else if let Some(error) = &self.patch.probe_error {
            ui.add_space(6.0);
            message_box(ui, egui::Color32::from_rgb(230, 90, 90), error);
        }

        ui.add_space(8.0);
        input_path_edit(
            ui,
            app,
            "新 init_early 二进制",
            &mut self.patch.binary,
            false,
            "选择文件…",
        );
        ui.add_space(6.0);
        save_path_edit(
            ui,
            app,
            "输出镜像",
            &mut self.patch.output,
            "选择保存位置…",
            "ramdisk-patched.img",
        );
        ui.add_space(8.0);
        let patchable = self
            .patch
            .probe
            .as_ref()
            .is_some_and(|probe| !probe.patched && probe.layout_known && probe.has_init_early);
        let ready = !app.job_running()
            && !self.patch.image.trim().is_empty()
            && !self.patch.binary.trim().is_empty()
            && !self.patch.output.trim().is_empty()
            && patchable;
        let output = self.patch.output.trim().to_owned();
        if run_button(
            ui,
            "Patch init_early",
            ready,
            Some("需要先完成检测，并确认镜像包含未修改的 bin/init_early"),
        )
        .clicked()
        {
            self.pending = Some(PendingOp::Patch {
                output: output.clone(),
            });
            self.result = None;
            app.start_job(crate::worker::JobOp::RamdiskPatch {
                image: self.patch.image.trim().to_owned(),
                binary: self.patch.binary.trim().to_owned(),
                output,
            });
        }
    }

    fn start_probe(&mut self, app: &mut HaucetApp) {
        if !self.probe_requested || app.job_running() {
            return;
        }
        let image = self.patch.image.trim().to_owned();
        self.probe_requested = false;
        self.pending = Some(PendingOp::Probe {
            image: image.clone(),
        });
        app.start_job(crate::worker::JobOp::RamdiskProbe { image });
    }

    fn handle_drops(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        let drops = app.take_drops(ui.ctx());
        if let Some(path) = drops.first() {
            self.select_unpack_image(path.display().to_string());
        }
    }

    fn show_result(&self, ui: &mut egui::Ui) {
        let Some(result) = &self.result else {
            return;
        };
        ui.add_space(6.0);
        if result.ok {
            message_box(ui, egui::Color32::from_rgb(90, 200, 120), &result.summary);
            if !result.output.is_empty() && ui.button("打开输出位置").clicked() {
                open_in_file_manager(std::path::Path::new(&result.output));
            }
        } else {
            message_box(ui, egui::Color32::from_rgb(230, 90, 90), &result.summary);
        }
    }

    fn update_unpack_output(&mut self) {
        let next = sibling_output_path(&self.unpack.image, "ramdisk", "-work");
        update_derived_path(&mut self.unpack.output, &mut self.unpack.auto_output, next);
    }

    fn update_patch_output(&mut self) {
        let next = sibling_output_path(&self.patch.image, "ramdisk", "-patched.img");
        update_derived_path(&mut self.patch.output, &mut self.patch.auto_output, next);
    }
}

fn payload_growth_space(cert_limit: u64, header_size: u64, payload_size: u64) -> u64 {
    cert_limit
        .checked_sub(header_size)
        .and_then(|limit| limit.checked_sub(payload_size))
        .unwrap_or(0)
}

fn input_path_edit(
    ui: &mut egui::Ui,
    app: &mut HaucetApp,
    label: &str,
    value: &mut String,
    is_dir: bool,
    button: &str,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).strong());
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(if is_dir {
                    "目录路径"
                } else {
                    "文件路径"
                })
                .desired_width(ui.available_width() - 220.0),
        );
        if ui.button(button).clicked() {
            let picked = if is_dir {
                app.pick_dir(label)
            } else {
                app.pick_file(label, &[])
            };
            if let Some(path) = picked {
                *value = path.display().to_string();
            }
        }
    });
}

fn save_path_edit(
    ui: &mut egui::Ui,
    app: &mut HaucetApp,
    label: &str,
    value: &mut String,
    button: &str,
    fallback_name: &str,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).strong());
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text("文件路径")
                .desired_width(ui.available_width() - 220.0),
        );
        if ui.button(button).clicked() {
            let suggested = std::path::Path::new(value.trim())
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| fallback_name.to_owned());
            if let Some(path) = app.pick_save(label, &suggested) {
                *value = path.display().to_string();
            }
        }
    });
}
