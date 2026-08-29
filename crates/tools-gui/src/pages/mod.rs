pub mod cpio;
pub mod entropy;
pub mod erofs;
pub mod fastboot;
pub mod home;
pub mod images;
pub mod nvme;
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutChoice {
    #[default]
    Auto,
    L1,
    L2,
}

impl LayoutChoice {
    pub const ALL: [Self; 3] = [Self::Auto, Self::L1, Self::L2];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动检测",
            Self::L1 => "L1",
            Self::L2 => "L2",
        }
    }

    pub fn spec(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::L1 => "l1",
            Self::L2 => "l2",
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
