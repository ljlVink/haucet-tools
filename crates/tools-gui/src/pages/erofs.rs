use crate::app::HaucetApp;
use crate::pages::images::ImageKind;
use crate::pages::{ResultView, run_button};
use crate::util::{
    human_size, message_box, open_in_file_manager, sibling_output_path, trimmed_non_empty,
    update_derived_path,
};
use common::formats::erofs::ErofsManifest;
use eframe::egui;
use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ErofsTab {
    #[default]
    Unpack,
    Repack,
}

#[derive(Debug)]
enum PendingOp {
    Unpack { output: String },
    Repack { output: String },
}

#[derive(Debug, Default)]
pub struct ErofsPage {
    pub tab: ErofsTab,
    pub unpack: UnpackState,
    pub repack: RepackState,
    pub result: Option<ResultView>,
    pending: Option<PendingOp>,
}

#[derive(Debug, Default)]
pub struct UnpackState {
    pub image: String,
    pub output: String,
    pub force: bool,
    pub tools_dir: String,
    auto_output: Option<String>,
}

#[derive(Debug, Default)]
pub struct RepackState {
    pub workspace: String,
    pub output: String,
    pub allow_grow: bool,
    pub tools_dir: String,
    pub manifest: Option<ErofsManifest>,
    pub manifest_error: Option<String>,
    pub manifest_from: String,
    manifest_stamp: Option<ManifestStamp>,
    auto_output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ManifestStamp {
    Missing,
    Present {
        len: u64,
        modified: Option<SystemTime>,
    },
}

impl ErofsPage {
    pub fn select_unpack_image(&mut self, image: String) {
        self.unpack.image = image;
        self.update_unpack_output();
    }

    pub fn select_workspace(&mut self, workspace: String) {
        self.repack.workspace = workspace;
        self.repack.manifest = None;
        self.repack.manifest_error = None;
        self.repack.manifest_from.clear();
        self.repack.manifest_stamp = None;
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        self.poll_result(app);
        self.poll_manifest();

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("操作").weak());
            ui.selectable_value(&mut self.tab, ErofsTab::Unpack, "解包镜像");
            ui.selectable_value(&mut self.tab, ErofsTab::Repack, "重建镜像");
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
        let Some(result) = app.take_image_result(ImageKind::Erofs) else {
            return;
        };
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.apply_result(pending, result);
    }

    fn apply_result(&mut self, pending: PendingOp, result: crate::job::JobResult) {
        if !result.ok {
            self.result = Some(ResultView {
                ok: false,
                summary: result.summary,
                output: String::new(),
            });
            return;
        }
        self.result = Some(ResultView {
            ok: true,
            summary: result.summary,
            output: match pending {
                PendingOp::Unpack { output } | PendingOp::Repack { output } => output,
            },
        });
    }

    fn poll_manifest(&mut self) {
        let workspace = self.repack.workspace.trim().to_owned();
        if workspace.is_empty() {
            self.repack.manifest = None;
            self.repack.manifest_error = None;
            self.repack.manifest_from.clear();
            self.repack.manifest_stamp = None;
            return;
        }
        let stamp = manifest_stamp(Path::new(&workspace));
        if workspace == self.repack.manifest_from
            && self.repack.manifest_stamp.as_ref() == Some(&stamp)
        {
            return;
        }
        self.repack.manifest_from = workspace.clone();
        self.repack.manifest_stamp = Some(stamp);
        match common::formats::erofs::read_manifest(std::path::Path::new(&workspace)) {
            Ok(manifest) => {
                let next = default_repack_output(&workspace, &manifest.original_file_name);
                update_derived_path(&mut self.repack.output, &mut self.repack.auto_output, next);
                self.repack.manifest_error = None;
                self.repack.manifest = Some(manifest);
            }
            Err(error) => {
                self.repack.manifest = None;
                self.repack.manifest_error = Some(format!("{error:#}"));
            }
        }
    }

    fn unpack_tab(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("镜像文件").strong());
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.unpack.image)
                    .hint_text("分区镜像或拖放文件到这里")
                    .desired_width(ui.available_width() - 240.0),
            );
            if response.changed() {
                self.update_unpack_output();
            }
            if ui.button("选择文件…").clicked()
                && let Some(path) = app.pick_file("选择 EROFS 镜像", &[("镜像", &["img"])])
            {
                self.select_unpack_image(path.display().to_string());
            }
        });
        let drops = app.take_drops(ui.ctx());
        if let Some(path) = drops.first() {
            self.select_unpack_image(path.display().to_string());
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("输出工作区").strong());
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
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.unpack.force, "覆盖已存在的工作区");
        });
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
            app.start_job(crate::worker::JobOp::ErofsUnpack {
                image: self.unpack.image.trim().to_owned(),
                output,
                force: self.unpack.force,
                tools_dir: trimmed_non_empty(&self.unpack.tools_dir),
            });
        }
    }

    fn repack_tab(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        ui.label(
            egui::RichText::new("把解包出来的工作区重新打包成分区镜像(保留原 HVB 证书)").weak(),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("工作区目录").strong());
            ui.add(
                egui::TextEdit::singleline(&mut self.repack.workspace)
                    .hint_text("包含 haucet-erofs.json 的目录")
                    .desired_width(ui.available_width() - 240.0),
            );
            if ui.button("选择目录…").clicked()
                && let Some(dir) = app.pick_dir("选择 EROFS 工作区")
            {
                self.select_workspace(dir.display().to_string());
            }
        });
        let drops = app.take_drops(ui.ctx());
        if let Some(path) = drops.first()
            && path.is_dir()
        {
            self.select_workspace(path.display().to_string());
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
                                    "保留(未重新签名)"
                                } else {
                                    "无"
                                },
                            );
                        });
                });
            ui.add_space(6.0);
            message_box(
                ui,
                egui::Color32::from_rgb(230, 170, 40),
                "注意: 重新打包不会重新签名 HVB 证书".to_owned(),
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
            if ui.button("选择保存位置…").clicked()
                && let Some(path) = app.pick_save("保存打包后的镜像", "new-partition.img")
            {
                self.repack.output = path.display().to_string();
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.repack.allow_grow, "允许镜像超过原始大小");
        });
        ui.add_space(8.0);
        let ready = !app.job_running()
            && !self.repack.workspace.trim().is_empty()
            && !self.repack.output.trim().is_empty()
            && self.repack.manifest.is_some();
        let output = self.repack.output.trim().to_owned();
        if run_button(ui, "重新打包", ready, Some("需要先选择有效的工作区")).clicked()
        {
            self.pending = Some(PendingOp::Repack {
                output: output.clone(),
            });
            self.result = None;
            app.start_job(crate::worker::JobOp::ErofsRepack {
                workspace: self.repack.workspace.trim().to_owned(),
                output,
                allow_grow: self.repack.allow_grow,
                tools_dir: trimmed_non_empty(&self.repack.tools_dir),
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

    fn update_unpack_output(&mut self) {
        let next = sibling_output_path(&self.unpack.image, "erofs", "-work");
        update_derived_path(&mut self.unpack.output, &mut self.unpack.auto_output, next);
    }
}

fn manifest_stamp(workspace: &Path) -> ManifestStamp {
    match std::fs::metadata(workspace.join("haucet-erofs.json")) {
        Ok(metadata) => ManifestStamp::Present {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        },
        Err(_) => ManifestStamp::Missing,
    }
}

fn default_repack_output(workspace: &str, original_file_name: &str) -> String {
    let workspace = std::path::Path::new(workspace);
    let Some(parent) = workspace
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return String::new();
    };

    let original = std::path::Path::new(original_file_name);
    let Some(file_name) = original.file_name() else {
        return String::new();
    };
    let mut patched_name = original.file_stem().unwrap_or(file_name).to_os_string();
    patched_name.push("_patched");
    if let Some(extension) = original.extension() {
        patched_name.push(".");
        patched_name.push(extension);
    }

    parent.join(patched_name).display().to_string()
}
