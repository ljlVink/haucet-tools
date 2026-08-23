pub mod cpio;
pub mod erofs;
pub mod home;
pub mod package;
pub mod partition;
pub mod ramdisk;
pub mod update_bin;

use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Package,
    UpdateBin,
    Erofs,
    Ramdisk,
    Partition,
    Cpio,
}

impl Page {
    pub const ALL: [Self; 7] = [
        Self::Home,
        Self::Package,
        Self::UpdateBin,
        Self::Erofs,
        Self::Ramdisk,
        Self::Partition,
        Self::Cpio,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Home => "快速开始",
            Self::Package => "更新包解包",
            Self::UpdateBin => "update.bin",
            Self::Erofs => "EROFS 镜像",
            Self::Ramdisk => "Ramdisk",
            Self::Partition => "分区信息",
            Self::Cpio => "Cpio 浏览器",
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

pub fn layout_label(layout: &common::formats::update_bin::UpdateLayout) -> &'static str {
    match layout {
        common::formats::update_bin::UpdateLayout::Auto => "自动检测",
        common::formats::update_bin::UpdateLayout::L1 => "L1",
        common::formats::update_bin::UpdateLayout::L2 => "L2",
    }
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
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(text).strong()).min_size(egui::vec2(140.0, 32.0)),
    );
    match hint {
        Some(hint) => response.on_hover_text(hint),
        None => response,
    }
}
