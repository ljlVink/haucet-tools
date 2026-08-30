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
