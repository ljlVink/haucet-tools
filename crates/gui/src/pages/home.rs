use crate::app::HaucetApp;
use crate::detect::{self, FileKind};
use crate::pages::Page;
use eframe::egui;

#[derive(Debug, Default)]
pub struct HomePage {
    pub detection: Option<detect::Detection>,
}

impl HomePage {
    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        egui::ScrollArea::vertical()
            .id_salt("home-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.header(ui, app);
                ui.add_space(10.0);
                self.drop_zone(ui, app);
                ui.add_space(14.0);
                if let Some(detection) = self.detection.clone() {
                    self.detection_card(ui, app, &detection);
                }
                ui.add_space(14.0);
                self.quick_actions(ui, app);
                ui.add_space(14.0);
                self.recent_files(ui, app);
                ui.add_space(30.0);
            });
    }

    fn header(&self, ui: &mut egui::Ui, app: &HaucetApp) {
        ui.horizontal(|ui| {
            if let Some(logo) = &app.logo {
                ui.add(
                    egui::Image::new(logo)
                        .fit_to_exact_size(egui::vec2(72.0, 72.0))
                        .corner_radius(8),
                );
            }
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 72.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(
                        egui::RichText::new("欢迎使用 Haucet Tools")
                            .strong()
                            .size(22.0),
                    );
                },
            );
        });
    }

    fn drop_zone(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        let drops = app.take_drops(ui.ctx());
        if let Some(path) = drops.first() {
            self.detection = Some(detect::detect(path));
            app.settings.remember_path(path);
        }

        let height = 130.0;
        let (rect, _response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::hover(),
        );
        let hovered = ui.ctx().input(|input| {
            let has_hovered_file = input
                .raw
                .hovered_files
                .iter()
                .any(|file| file.path.is_some());
            has_hovered_file
                && input
                    .pointer
                    .hover_pos()
                    .is_none_or(|pos| rect.contains(pos))
        });
        let fill = if hovered {
            ui.visuals().selection.bg_fill.gamma_multiply(0.6)
        } else {
            ui.visuals().extreme_bg_color
        };
        ui.painter().rect(
            rect,
            egui::CornerRadius::same(10),
            fill,
            egui::Stroke::new(
                1.5_f32,
                if hovered {
                    ui.visuals().selection.stroke.color
                } else {
                    ui.visuals().widgets.noninteractive.bg_stroke.color
                },
            ),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            if hovered {
                "松开以识别文件"
            } else {
                "把文件或文件夹拖到这里"
            },
            egui::FontId::proportional(18.0),
            if hovered {
                ui.visuals().selection.stroke.color
            } else {
                ui.visuals().text_color()
            },
        );
    }

    fn detection_card(
        &mut self,
        ui: &mut egui::Ui,
        app: &mut HaucetApp,
        detection: &detect::Detection,
    ) {
        ui.label(egui::RichText::new("文件识别结果").strong().size(15.0));
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(detection.kind.label())
                            .strong()
                            .color(accent()),
                    );
                    ui.label(egui::RichText::new(&detection.path).weak().monospace());
                });
                ui.label(&detection.human);
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    for (label, page, kind) in suggested_actions(detection.kind) {
                        if ui.button(label).clicked() {
                            apply_action(app, page, kind, &detection.path);
                        }
                    }
                    if ui.button("重新识别").clicked() {
                        self.detection =
                            Some(detect::detect(std::path::Path::new(&detection.path)));
                    }
                });
            });
    }

    fn quick_actions(&self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        ui.label(egui::RichText::new("常用任务").strong().size(15.0));
        ui.add_space(4.0);
        let spacing = 12.0;
        let available_width = ui.available_width();
        let columns = if available_width >= 624.0 { 2 } else { 1 };
        let card_width = if columns == 2 {
            (available_width - spacing) / 2.0
        } else {
            available_width
        };

        egui::Grid::new("quick-actions")
            .num_columns(columns)
            .spacing([spacing, spacing])
            .show(ui, |ui| {
                quick_card(ui, "解update.zip更新包", Page::Package, card_width, app);
                if columns == 1 {
                    ui.end_row();
                }
                quick_card(ui, "给 ramdisk 补丁", Page::Ramdisk, card_width, app);
                ui.end_row();
                quick_card(ui, "解/打包 EROFS", Page::Erofs, card_width, app);
                if columns == 1 {
                    ui.end_row();
                }
                quick_card(ui, "分区信息", Page::Partition, card_width, app);
                ui.end_row();
                quick_card(ui, "Fastboot 刷机", Page::Fastboot, card_width, app);
                if columns == 1 {
                    ui.end_row();
                }
                quick_card(ui, "VCOM 刷机", Page::Vcom, card_width, app);
                ui.end_row();
            });
    }

    fn recent_files(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        if app.settings.recent.is_empty() {
            return;
        }
        ui.add_space(6.0);
        ui.label(egui::RichText::new("最近打开").strong().size(15.0));
        let recents = app.settings.recent.clone();
        for recent in recents {
            let path = std::path::PathBuf::from(&recent);
            let label = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| recent.clone());
            if ui
                .add(egui::Button::new(format!("📄 {label}")).truncate())
                .on_hover_text(&recent)
                .clicked()
            {
                let detection = detect::detect(&path);
                self.detection = Some(detection);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionKind {
    Input,
    Workspace,
}

fn suggested_actions(kind: FileKind) -> Vec<(&'static str, Page, ActionKind)> {
    match kind {
        FileKind::ZipPackage => vec![
            ("解包更新包", Page::Package, ActionKind::Input),
            ("查看包内组件", Page::Package, ActionKind::Input),
        ],
        FileKind::UpdateBin => vec![
            ("查看组件列表", Page::UpdateBin, ActionKind::Input),
            ("解包全部组件", Page::UpdateBin, ActionKind::Input),
        ],
        FileKind::Erofs => vec![
            ("解包 EROFS", Page::Erofs, ActionKind::Input),
            ("查看分区信息", Page::Partition, ActionKind::Input),
        ],
        FileKind::HarmonyFrame => vec![
            ("Ramdisk 操作", Page::Ramdisk, ActionKind::Input),
            ("查看分区信息", Page::Partition, ActionKind::Input),
        ],
        FileKind::Rvt => vec![("查看 RVT 信息", Page::Partition, ActionKind::Input)],
        FileKind::HvbWrapped => vec![("查看分区信息", Page::Partition, ActionKind::Input)],
        FileKind::Cpio => vec![("浏览 cpio 归档", Page::Cpio, ActionKind::Input)],
        FileKind::ErofsWorkspace => vec![("重新打包镜像", Page::Erofs, ActionKind::Workspace)],
        FileKind::RamdiskWorkspace => vec![("重新打包镜像", Page::Ramdisk, ActionKind::Workspace)],
        FileKind::Unknown => vec![("尝试查看分区信息", Page::Partition, ActionKind::Input)],
    }
}

fn apply_action(app: &mut HaucetApp, page: Page, kind: ActionKind, path: &str) {
    match (page, kind) {
        (Page::Package, ActionKind::Input) => {
            app.package.input = path.to_owned();
            if app.package.output.trim().is_empty() {
                app.package.output = default_output_for(&app.package.input, "package-work");
            }
        }
        (Page::UpdateBin, _) => {
            app.update_bin.input = path.to_owned();
        }
        (Page::Erofs, ActionKind::Input) => {
            app.erofs.unpack.image = path.to_owned();
            app.erofs.tab = crate::pages::erofs::ErofsTab::Unpack;
        }
        (Page::Erofs, ActionKind::Workspace) => {
            app.erofs.repack.workspace = path.to_owned();
            app.erofs.tab = crate::pages::erofs::ErofsTab::Repack;
        }
        (Page::Ramdisk, ActionKind::Input) => {
            app.ramdisk.patch.image = path.to_owned();
            app.ramdisk.tab = crate::pages::ramdisk::RamdiskTab::Patch;
            if app.ramdisk.patch.output.trim().is_empty() {
                app.ramdisk.patch.output =
                    default_output_for(&app.ramdisk.patch.image, "patched.img");
            }
        }
        (Page::Ramdisk, ActionKind::Workspace) => {
            app.ramdisk.repack.workspace = path.to_owned();
            app.ramdisk.tab = crate::pages::ramdisk::RamdiskTab::Repack;
        }
        (Page::Partition, _) => {
            app.partition.select_input(path.to_owned());
        }
        (Page::Cpio, _) => {
            app.cpio.source = crate::pages::cpio::CpioSource::File;
            app.cpio.path = path.to_owned();
        }
        _ => {}
    }
    app.nav(page);
}

fn default_output_for(input: &str, suffix: &str) -> String {
    let path = std::path::Path::new(input);
    let parent = path
        .parent()
        .map(|parent| parent.display().to_string())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_owned());
    let name = format!("{stem}-{suffix}");
    if parent.is_empty() {
        name
    } else {
        format!("{parent}/{name}")
    }
}

fn quick_card(ui: &mut egui::Ui, title: &str, page: Page, outer_width: f32, app: &mut HaucetApp) {
    let predicted_rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(outer_width, 48.0));
    let hovered = ui.input(|input| {
        input
            .pointer
            .hover_pos()
            .is_some_and(|pos| predicted_rect.contains(pos))
    });
    let visuals = ui.visuals();
    let fill = if hovered {
        visuals.widgets.hovered.bg_fill
    } else {
        visuals.extreme_bg_color
    };
    let stroke = if hovered {
        visuals.widgets.hovered.bg_stroke
    } else {
        visuals.widgets.noninteractive.bg_stroke
    };
    let response = egui::Frame::group(ui.style())
        .fill(fill)
        .stroke(stroke)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width((outer_width - 24.0).max(0.0));
            ui.label(egui::RichText::new(title).strong().size(15.0));
        })
        .response
        .interact(egui::Sense::click());
    if response.clicked() {
        app.nav(page);
    }
}

fn accent() -> egui::Color32 {
    egui::Color32::from_rgb(90, 170, 255)
}
