use crate::app::HaucetApp;
use crate::pages::{Page, ResultView, run_button};
use crate::util::{human_size, kv, message_box, section};
use common::entropy::EntropySummary;
use eframe::egui;

#[derive(Debug, Default)]
pub struct EntropyPage {
    pub input: String,
    pub summary: Option<EntropySummary>,
    pub result: Option<ResultView>,
}

impl EntropyPage {
    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        self.poll_result(app);

        egui::ScrollArea::vertical()
            .id_salt("entropy-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "计算文件字节分布的 Shannon 信息熵",
                    )
                    .weak(),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("文件").strong());
                    ui.add(
                        egui::TextEdit::singleline(&mut self.input)
                            .hint_text("文件路径或拖放文件到这里")
                            .desired_width(ui.available_width() - 160.0),
                    );
                    if ui.button("选择文件…").clicked()
                        && let Some(path) = app.pick_file("选择文件", &[])
                    {
                        self.input = path.display().to_string();
                    }
                });

                let drops = app.take_drops(ui.ctx());
                if let Some(path) = drops.first() {
                    self.input = path.display().to_string();
                }

                ui.add_space(8.0);
                if run_button(
                    ui,
                    "计算信息熵",
                    !app.job_running() && !self.input.trim().is_empty(),
                    None,
                )
                .clicked()
                {
                    self.summary = None;
                    self.result = None;
                    app.start_job(crate::worker::JobOp::FileEntropy {
                        file: self.input.trim().to_owned(),
                    });
                }

                ui.add_space(10.0);
                if let Some(result) = &self.result
                    && !result.ok
                {
                    message_box(ui, egui::Color32::from_rgb(230, 90, 90), &result.summary);
                }
                if let Some(summary) = &self.summary {
                    render_summary(ui, summary);
                }
            });
    }

    fn poll_result(&mut self, app: &mut HaucetApp) {
        let Some(result) = app.take_result(Page::Entropy) else {
            return;
        };
        if !result.ok {
            self.summary = None;
            self.result = Some(ResultView {
                ok: false,
                summary: result.summary,
                output: String::new(),
            });
            return;
        }
        if let Some(payload) = result.payload {
            match serde_json::from_value::<EntropySummary>(payload) {
                Ok(summary) => {
                    self.summary = Some(summary);
                    self.result = None;
                }
                Err(error) => {
                    self.summary = None;
                    self.result = Some(ResultView {
                        ok: false,
                        summary: format!("解析结果失败：{error}"),
                        output: String::new(),
                    });
                }
            }
        }
    }
}

fn render_summary(ui: &mut egui::Ui, summary: &EntropySummary) {
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
                        "0x{:02X}（{} 字节, {:.2}%）",
                        byte.byte,
                        byte.count,
                        byte.ratio * 100.0
                    ),
                );
            }
        });
}
