pub mod cpio;
pub mod entropy;
pub mod erofs;
pub mod fastboot;
pub mod home;
pub mod images;
pub mod nvme;
pub mod oeminfo;
pub mod package;
pub mod partition;
pub mod ramdisk;
pub mod vcom;

use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Package,
    Images,
    Fastboot,
    Vcom,
    Cpio,
    Nvme,
    OemInfo,
}

impl Page {
    pub fn title(self) -> &'static str {
        match self {
            Self::Home => "快速开始",
            Self::Package => "更新包解包",
            Self::Images => "镜像工作区",
            Self::Fastboot => "Fastboot 刷机",
            Self::Vcom => "VCOM 刷机",
            Self::Cpio => "Cpio 浏览器",
            Self::Nvme => "NVMe / NVE",
            Self::OemInfo => "OEMINFO 浏览器",
        }
    }

    pub fn header(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Home => None,
            Self::Package => Some((
                "更新包解包",
                "读取更新包内容，选择需要的分区并解包到工作目录。",
            )),
            Self::Images => Some((
                "镜像工作区",
                "在同一处识别镜像，并完成分区镜像与启动镜像的解包、修改和重建。",
            )),
            Self::Fastboot => Some(("Fastboot 刷机", "HarmonyOS/Android Fastboot 刷机")),
            Self::Vcom => Some(("VCOM 刷机", "HiSilicon VCOM 刷机")),
            Self::Cpio => Some((
                "Cpio 浏览器",
                "浏览、提取和编辑 CPIO 文件、Ramdisk 镜像及解包工作区。",
            )),
            Self::Nvme => Some(("NVMe / NVE 编辑器", "查看与修改 HiSilicon NVE 条目")),
            Self::OemInfo => Some(("OEMINFO 浏览器", "查看 OEMINFO 数据块、A/B 副本与载荷类型")),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ResultView {
    pub ok: bool,
    pub summary: String,
    pub output: String,
}

pub fn badge_text(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    ui.label(egui::RichText::new(text).color(color).strong());
}

pub(crate) fn page_header(ui: &mut egui::Ui, title: &str, description: &str, busy: bool) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(title).strong().size(22.0));
            ui.label(egui::RichText::new(description).weak());
        });
        if busy {
            ui.add_space(8.0);
            ui.add(egui::Spinner::new().size(16.0));
        }
    });
}

pub(crate) fn run_button(
    ui: &mut egui::Ui,
    text: &str,
    enabled: bool,
    hint: Option<&str>,
) -> egui::Response {
    let response = ui
        .allocate_ui_with_layout(
            egui::vec2(140.0, 32.0),
            egui::Layout::centered_and_justified(egui::Direction::TopDown),
            |ui| {
                ui.add_enabled(
                    enabled,
                    egui::Button::new(egui::RichText::new(text).strong())
                        .min_size(egui::vec2(140.0, 32.0)),
                )
            },
        )
        .inner;
    match hint {
        Some(hint) => response.on_hover_text(hint),
        None => response,
    }
}
