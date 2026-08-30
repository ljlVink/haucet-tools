use crate::util::{human_size, kv, section};
use common::entropy::EntropySummary;
use eframe::egui;

pub(crate) fn render_summary(ui: &mut egui::Ui, summary: &EntropySummary) {
    section(ui, &tr!("entropy-title"));
    egui::Grid::new("entropy-summary-grid")
        .num_columns(2)
        .spacing([18.0, 6.0])
        .show(ui, |ui| {
            kv(ui, &tr!("file-size"), human_size(summary.size));
            kv(
                ui,
                &tr!("shannon-entropy"),
                format!("{:.6} bits/byte", summary.entropy_bits_per_byte),
            );
            kv(
                ui,
                &tr!("randomness"),
                format!("{:.2}%", summary.normalized_percent()),
            );
            kv(
                ui,
                &tr!("unique-byte-values"),
                format!("{}/256", summary.unique_bytes),
            );
        });
    ui.add_space(12.0);
    render_window_chart(ui, summary);
}

fn render_window_chart(ui: &mut egui::Ui, summary: &EntropySummary) {
    section(ui, &tr!("entropy-window-chart"));
    if summary.windows.is_empty() {
        ui.label(egui::RichText::new(tr!("no-chart-data")).weak());
        return;
    }
    ui.label(
        egui::RichText::new(tr!(
            "entropy-window-samples",
            "window" => human_size(summary.window_size),
            "count" => summary.windows.len(),
        ))
        .weak()
        .small(),
    );
    ui.add_space(4.0);

    let desired_size = egui::vec2(ui.available_width().max(180.0), 190.0);
    let (response, painter) = ui.allocate_painter(desired_size, egui::Sense::hover());
    let rect = response.rect;
    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);

    let plot = egui::Rect::from_min_max(
        rect.min + egui::vec2(30.0, 10.0),
        rect.max - egui::vec2(8.0, 24.0),
    );
    let grid_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
    for value in [0.0_f32, 2.0, 4.0, 6.0, 8.0] {
        let y = egui::lerp(plot.bottom()..=plot.top(), value / 8.0);
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            egui::Stroke::new(1.0_f32, grid_color.gamma_multiply(0.45)),
        );
        if value == 0.0 || value == 4.0 || value == 8.0 {
            painter.text(
                egui::pos2(plot.left() - 5.0, y),
                egui::Align2::RIGHT_CENTER,
                format!("{value:.0}"),
                egui::FontId::monospace(10.0),
                ui.visuals().weak_text_color(),
            );
        }
    }

    let size = summary.size.max(1) as f64;
    let points = summary
        .windows
        .iter()
        .map(|window| {
            let center = window.offset as f64 + summary.window_size as f64 / 2.0;
            let x = egui::lerp(plot.left()..=plot.right(), (center / size) as f32);
            let normalized = (window.entropy_bits_per_byte / 8.0).clamp(0.0, 1.0) as f32;
            let y = egui::lerp(plot.bottom()..=plot.top(), normalized);
            egui::pos2(x, y)
        })
        .collect::<Vec<_>>();
    let line_color = ui.visuals().selection.stroke.color;
    if points.len() == 1 {
        painter.circle_filled(points[0], 3.0, line_color);
    } else {
        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(1.8_f32, line_color),
        ));
    }
    painter.text(
        egui::pos2(plot.left(), plot.bottom() + 6.0),
        egui::Align2::LEFT_TOP,
        "0",
        egui::FontId::monospace(10.0),
        ui.visuals().weak_text_color(),
    );
    painter.text(
        egui::pos2(plot.right(), plot.bottom() + 6.0),
        egui::Align2::RIGHT_TOP,
        human_size(summary.size),
        egui::FontId::monospace(10.0),
        ui.visuals().weak_text_color(),
    );
    response.on_hover_text(tr!("entropy-chart-hint"));
}
