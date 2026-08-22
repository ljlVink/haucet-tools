use crate::app::HaucetApp;
use crate::pages::{Page, ResultView, run_button};
use crate::util::{human_size, message_box, open_in_file_manager};
use common::formats::erofs::ErofsManifest;
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ErofsTab {
    #[default]
    Unpack,
    Repack,
}

#[derive(Debug, Default)]
pub struct ErofsPage {
    pub tab: ErofsTab,
    pub unpack: UnpackState,
    pub repack: RepackState,
    pub result: Option<ResultView>,
}

#[derive(Debug, Default)]
pub struct UnpackState {
    pub image: String,
    pub output: String,
    pub force: bool,
    pub tools_dir: String,
}

#[derive(Debug, Default)]
pub struct RepackState {
    pub workspace: String,
    pub output: String,
    pub allow_grow: bool,
    pub tools_dir: String,
    pub manifest: Option<ErofsManifest>,
    pub manifest_error: Option<String>,
    /// 当前 manifest 来自哪个工作区路径（避免每帧重复读取）。
    pub manifest_from: String,
}

impl ErofsPage {
    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        self.poll_result(app);
        self.poll_manifest();

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, ErofsTab::Unpack, "解包");
            ui.selectable_value(&mut self.tab, ErofsTab::Repack, "重新打包");
        });
        ui.add_space(8.0);

        match self.tab {
            ErofsTab::Unpack => self.unpack_tab(ui, app),
            ErofsTab::Repack => self.repack_tab(ui, app),
        }

        ui.add_space(10.0);
        self.show_result(ui);
    }

    fn poll_result(&mut self, app: &mut HaucetApp) {
        let Some(result) = app.take_result(Page::Erofs) else {
            return;
        };
        if !result.ok {
            self.result = Some(ResultView {
                ok: false,
                summary: result.summary.clone(),
                output: String::new(),
            });
            return;
        }
        if let Some(payload) = result.payload {
            if let Ok(manifest) = serde_json::from_value::<ErofsManifest>(payload) {
                self.repack.manifest = Some(manifest);
            }
        }
        self.result = Some(ResultView {
            ok: true,
            summary: result.summary.clone(),
            output: match self.tab {
                ErofsTab::Unpack => self.unpack.output.trim().to_owned(),
                ErofsTab::Repack => self.repack.output.trim().to_owned(),
            },
        });
    }

    /// 选择工作区后，直接读 haucet-erofs.json 展示信息（小文件，同步读）。
    fn poll_manifest(&mut self) {
        let workspace = self.repack.workspace.trim().to_owned();
        if workspace.is_empty() {
            self.repack.manifest = None;
            self.repack.manifest_error = None;
            self.repack.manifest_from.clear();
            return;
        }
        if workspace == self.repack.manifest_from && self.repack.manifest.is_some() {
            return;
        }
        match common::formats::erofs::read_manifest(std::path::Path::new(&workspace)) {
            Ok(manifest) => {
                self.repack.manifest_error = None;
                self.repack.manifest_from = workspace.clone();
                self.repack.manifest = Some(manifest);
            }
            Err(error) => {
                self.repack.manifest = None;
                self.repack.manifest_from = workspace.clone();
                self.repack.manifest_error = Some(format!("{error:#}"));
            }
        }
    }

    fn unpack_tab(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        ui.label(
            egui::RichText::new("把 EROFS 分区镜像（system.img、vendor.img 等）解成可编辑的工作区")
                .weak(),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("镜像文件").strong());
            ui.add(
                egui::TextEdit::singleline(&mut self.unpack.image)
                    .hint_text("分区镜像或拖放文件到这里")
                    .desired_width(ui.available_width() - 240.0),
            );
            if ui.button("选择文件…").clicked() {
                if let Some(path) = app.pick_file("选择 EROFS 镜像", &[("镜像", &["img"])]) {
                    self.unpack.image = path.display().to_string();
                    if self.unpack.output.trim().is_empty() {
                        self.unpack.output = default_workspace(&self.unpack.image);
                    }
                }
            }
        });
        let drops = app.take_drops(ui.ctx());
        if let Some(path) = drops.first() {
            self.unpack.image = path.display().to_string();
            if self.unpack.output.trim().is_empty() {
                self.unpack.output = default_workspace(&self.unpack.image);
            }
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("输出工作区").strong());
            ui.add(
                egui::TextEdit::singleline(&mut self.unpack.output)
                    .desired_width(ui.available_width() - 240.0),
            );
            if ui.button("选择目录…").clicked() {
                if let Some(dir) = app.pick_dir("选择输出目录") {
                    self.unpack.output = dir.display().to_string();
                }
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.unpack.force, "覆盖已存在的工作区");
            ui.label("工具目录");
            ui.add(
                egui::TextEdit::singleline(&mut self.unpack.tools_dir)
                    .hint_text("留空自动查找")
                    .desired_width(180.0),
            );
            if ui.button("浏览…").clicked() {
                if let Some(dir) = app.pick_dir("选择 EROFS 工具目录") {
                    self.unpack.tools_dir = dir.display().to_string();
                }
            }
        });
        ui.add_space(8.0);
        let ready = !app.job_running()
            && !self.unpack.image.trim().is_empty()
            && !self.unpack.output.trim().is_empty();
        if run_button(ui, "开始解包", ready, None).clicked() {
            app.start_job(crate::worker::JobOp::ErofsUnpack {
                image: self.unpack.image.trim().to_owned(),
                output: self.unpack.output.trim().to_owned(),
                force: self.unpack.force,
                tools_dir: optional(&self.unpack.tools_dir),
            });
        }
    }

    fn repack_tab(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        ui.label(
            egui::RichText::new("把解包出来的工作区重新打包成分区镜像（保留原 HVB 证书）")
                .weak(),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("工作区目录").strong());
            ui.add(
                egui::TextEdit::singleline(&mut self.repack.workspace)
                    .hint_text("包含 haucet-erofs.json 的目录")
                    .desired_width(ui.available_width() - 240.0),
            );
            if ui.button("选择目录…").clicked() {
                if let Some(dir) = app.pick_dir("选择 EROFS 工作区") {
                    self.repack.workspace = dir.display().to_string();
                    self.repack.manifest = None;
                }
            }
        });
        let drops = app.take_drops(ui.ctx());
        if let Some(path) = drops.first() {
            if path.is_dir() {
                self.repack.workspace = path.display().to_string();
                self.repack.manifest = None;
            }
        }
        ui.add_space(6.0);
        if let Some(manifest) = &self.repack.manifest {
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    egui::Grid::new("erofs-manifest-grid")
                        .num_columns(2)
                        .spacing([16.0, 6.0])
                        .show(ui, |ui| {
                            crate::util::kv(ui, "分区", &manifest.partition);
                            crate::util::kv(ui, "原始文件", &manifest.original_file_name);
                            crate::util::kv(ui, "原始大小", human_size(manifest.original_size));
                            crate::util::kv(ui, "原始 SHA256", &manifest.original_sha256);
                            crate::util::kv(
                                ui,
                                "HVB 证书",
                                if manifest.hvb.is_some() {
                                    "保留（未重新签名）"
                                } else {
                                    "无"
                                },
                            );
                            crate::util::kv(ui, "extract.erofs", &manifest.extract_erofs_version);
                            crate::util::kv(ui, "mkfs.erofs", &manifest.mkfs_erofs_version);
                        });
                });
            ui.add_space(6.0);
            message_box(
                ui,
                egui::Color32::from_rgb(230, 170, 40),
                "注意：重新打包不会重新签名 HVB 证书。设备安全启动（secure boot）可能拒绝新镜像，"
                    .to_owned() + "即使文件结构完全合法。",
            );
        } else if let Some(error) = &self.repack.manifest_error {
            message_box(ui, egui::Color32::from_rgb(230, 90, 90), error);
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("输出镜像").strong());
            ui.add(
                egui::TextEdit::singleline(&mut self.repack.output)
                    .hint_text("例如 new-system.img")
                    .desired_width(ui.available_width() - 260.0),
            );
            if ui.button("选择保存位置…").clicked() {
                if let Some(path) = app.pick_save("保存打包后的镜像", "new-partition.img") {
                    self.repack.output = path.display().to_string();
                }
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.repack.allow_grow, "允许镜像超过原始大小");
            ui.label("工具目录");
            ui.add(
                egui::TextEdit::singleline(&mut self.repack.tools_dir)
                    .hint_text("留空自动查找")
                    .desired_width(180.0),
            );
            if ui.button("浏览…").clicked() {
                if let Some(dir) = app.pick_dir("选择 EROFS 工具目录") {
                    self.repack.tools_dir = dir.display().to_string();
                }
            }
        });
        ui.add_space(8.0);
        let ready = !app.job_running()
            && !self.repack.workspace.trim().is_empty()
            && !self.repack.output.trim().is_empty()
            && self.repack.manifest.is_some();
        if run_button(ui, "重新打包", ready, Some("需要先选择有效的工作区")).clicked() {
            app.start_job(crate::worker::JobOp::ErofsRepack {
                workspace: self.repack.workspace.trim().to_owned(),
                output: self.repack.output.trim().to_owned(),
                allow_grow: self.repack.allow_grow,
                tools_dir: optional(&self.repack.tools_dir),
            });
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

fn optional(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn default_workspace(image: &str) -> String {
    let path = std::path::Path::new(image);
    let parent = path
        .parent()
        .map(|parent| parent.display().to_string())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "erofs".to_owned());
    let name = format!("{stem}-work");
    if parent.is_empty() {
        name
    } else {
        format!("{parent}/{name}")
    }
}
