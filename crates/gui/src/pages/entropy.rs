use crate::util::{human_size, kv, section};
use common::entropy::EntropySummary;
use eframe::egui;

pub(crate) fn render_summary(ui: &mut egui::Ui, summary: &EntropySummary) {
    section(ui, "信息熵");
    egui::Grid::new("entropy-summary-grid")
        .num_columns(2)
        .spacing([18.0, 6.0])
        .show(ui, |ui| {
            kv(ui, "文件大小", human_size(summary.size));
            kv(
                ui,
                "Shannon 熵",
                format!("{:.6} bits/byte", summary.entropy_bits_per_byte),
            );
            kv(
                ui,
                "接近随机程度",
                format!("{:.2}%", summary.normalized_percent()),
            );
            kv(
                ui,
                "出现过的字节值",
                format!("{}/256", summary.unique_bytes),
            );
            if let Some(byte) = &summary.most_common {
                kv(
                    ui,
                    "最常见字节",
                    format!(
                        "0x{:02X}({} 字节, {:.2}%)",
                        byte.byte,
                        byte.count,
                        byte.ratio * 100.0
                    ),
                );
            }
        });
}
