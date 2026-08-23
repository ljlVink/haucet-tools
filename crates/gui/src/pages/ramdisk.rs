use crate::app::HaucetApp;
use crate::pages::{Page, ResultView, badge_text, run_button};
use crate::util::{human_size, message_box, open_in_file_manager};
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RamdiskTab {
    #[default]
    Unpack,
    Repack,
    Patch,
}

#[derive(Debug, Default)]
pub struct RamdiskPage {
    pub tab: RamdiskTab,
    pub unpack: UnpackState,
    pub repack: RepackState,
    pub patch: PatchState,
    pub result: Option<ResultView>,
}

#[derive(Debug, Default)]
pub struct UnpackState {
    pub image: String,
    pub output: String,
    pub force: bool,
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
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProbeInfo {
    pub patched: bool,
    pub has_init_early: bool,
    pub layout_known: bool,
    pub payload_format: String,
    pub payload_len: u64,
    pub cert_original_len: u64,
}

impl RamdiskPage {
    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        self.poll_result(app);

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, RamdiskTab::Unpack, "解包");
            ui.selectable_value(&mut self.tab, RamdiskTab::Repack, "重新打包");
            ui.selectable_value(&mut self.tab, RamdiskTab::Patch, "一键打补丁");
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
        let Some(result) = app.take_result(Page::Ramdisk) else {
            return;
        };
        if let Some(payload) = result.payload
            && let Ok(probe) = serde_json::from_value::<ProbeInfo>(payload)
        {
            self.patch.probe = Some(probe);
            self.patch.probe_error = None;
            return;
        }
        if !result.ok && self.patch.probe.is_none() {
            self.patch.probe_error = Some(result.summary.clone());
        }
        self.result = Some(ResultView {
            ok: result.ok,
            summary: result.summary.clone(),
            output: match self.tab {
                RamdiskTab::Unpack => self.unpack.output.trim().to_owned(),
                RamdiskTab::Repack | RamdiskTab::Patch => {
                    let out = match self.tab {
                        RamdiskTab::Repack => &self.repack.output,
                        _ => &self.patch.output,
                    };
                    out.trim().to_owned()
                }
            },
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
            ui.add(
                egui::TextEdit::singleline(&mut self.unpack.image)
                    .hint_text("ramdisk 镜像或拖放文件到这里")
                    .desired_width(ui.available_width() - 240.0),
            );
            if ui.button("选择文件…").clicked()
                && let Some(path) = app.pick_file("选择 ramdisk 镜像", &[("镜像", &["img"])])
            {
                self.unpack.image = path.display().to_string();
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
            ui.checkbox(&mut self.unpack.force, "覆盖已存在目录");
        });
        ui.add_space(8.0);
        let ready = !app.job_running()
            && !self.unpack.image.trim().is_empty()
            && !self.unpack.output.trim().is_empty();
        if run_button(ui, "开始解包", ready, None).clicked() {
            app.start_job(crate::worker::JobOp::RamdiskUnpack {
                image: self.unpack.image.trim().to_owned(),
                output: self.unpack.output.trim().to_owned(),
                force: self.unpack.force,
            });
        }
    }

    fn repack_tab(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        ui.label(
            egui::RichText::new(
                "用解包工作区里的 ramdisk.cpio 重建镜像（需要原始镜像以保留 HVB 头）",
            )
            .weak(),
        );
        ui.add_space(6.0);
        path_edit(
            ui,
            app,
            "工作区目录",
            &mut self.repack.workspace,
            true,
            "选择目录…",
        );
        ui.add_space(6.0);
        path_edit(
            ui,
            app,
            "原始镜像",
            &mut self.repack.original,
            false,
            "选择文件…",
        );
        ui.add_space(6.0);
        path_edit(
            ui,
            app,
            "输出镜像",
            &mut self.repack.output,
            false,
            "选择保存位置…",
        );
        ui.add_space(8.0);
        let ready = !app.job_running()
            && !self.repack.workspace.trim().is_empty()
            && !self.repack.original.trim().is_empty()
            && !self.repack.output.trim().is_empty();
        if run_button(ui, "重新打包", ready, None).clicked() {
            app.start_job(crate::worker::JobOp::RamdiskRepack {
                workspace: self.repack.workspace.trim().to_owned(),
                original: self.repack.original.trim().to_owned(),
                output: self.repack.output.trim().to_owned(),
            });
        }
    }

    fn patch_tab(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        ui.label(
            egui::RichText::new("把自制的 init_early 二进制替换进 ramdisk, 并自动腾出空间").weak(),
        );
        ui.add_space(4.0);
        message_box(
            ui,
            egui::Color32::from_rgb(230, 170, 40),
            "流程：备份原 bin/init_early 到 .backup/ → 放入新二进制 → 删除 libclang_rt 调试库腾空间 → 校验新镜像不超过 HVB 证书限制。",
        );
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("ramdisk 镜像").strong());
            ui.add(
                egui::TextEdit::singleline(&mut self.patch.image)
                    .hint_text("原始 ramdisk 镜像")
                    .desired_width(ui.available_width() - 260.0),
            );
            if ui.button("选择文件…").clicked()
                && let Some(path) = app.pick_file("选择 ramdisk 镜像", &[("镜像", &["img"])])
            {
                self.patch.image = path.display().to_string();
                self.patch.probe = None;
                self.patch.probe_error = None;
                if self.patch.output.trim().is_empty() {
                    self.patch.output = default_patched(&self.patch.image);
                }
                app.start_job(crate::worker::JobOp::RamdiskProbe {
                    image: self.patch.image.trim().to_owned(),
                });
            }
        });
        let drops = app.take_drops(ui.ctx());
        if let Some(path) = drops.first() {
            self.patch.image = path.display().to_string();
            self.patch.probe = None;
            self.patch.probe_error = None;
            if self.patch.output.trim().is_empty() {
                self.patch.output = default_patched(&self.patch.image);
            }
            app.start_job(crate::worker::JobOp::RamdiskProbe {
                image: self.patch.image.trim().to_owned(),
            });
        }

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
                            if probe.cert_original_len > probe.payload_len {
                                crate::util::kv(
                                    ui,
                                    "可用增长空间",
                                    human_size(probe.cert_original_len - probe.payload_len),
                                );
                            }
                        });
                });
        } else if let Some(error) = &self.patch.probe_error {
            ui.add_space(6.0);
            message_box(ui, egui::Color32::from_rgb(230, 90, 90), error);
        }

        ui.add_space(8.0);
        path_edit(
            ui,
            app,
            "新 init_early 二进制",
            &mut self.patch.binary,
            false,
            "选择文件…",
        );
        ui.add_space(6.0);
        path_edit(
            ui,
            app,
            "输出镜像",
            &mut self.patch.output,
            false,
            "选择保存位置…",
        );
        ui.add_space(8.0);
        let ready = !app.job_running()
            && !self.patch.image.trim().is_empty()
            && !self.patch.binary.trim().is_empty()
            && !self.patch.output.trim().is_empty();
        if run_button(ui, "一键打补丁", ready, None).clicked() {
            app.start_job(crate::worker::JobOp::RamdiskPatch {
                image: self.patch.image.trim().to_owned(),
                binary: self.patch.binary.trim().to_owned(),
                output: self.patch.output.trim().to_owned(),
            });
        }
    }

    fn handle_drops(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        let drops = app.take_drops(ui.ctx());
        if let Some(path) = drops.first() {
            self.unpack.image = path.display().to_string();
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
}

fn path_edit(
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

fn default_patched(image: &str) -> String {
    let path = std::path::Path::new(image);
    let parent = path
        .parent()
        .map(|parent| parent.display().to_string())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ramdisk".to_owned());
    let name = format!("{stem}-patched.img");
    if parent.is_empty() {
        name
    } else {
        format!("{parent}/{name}")
    }
}
