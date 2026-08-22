mod app;
mod job;
mod model;

use app::HaucetApp;
use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([960.0, 680.0]),
        ..Default::default()
    };
    eframe::run_native(
        "haucet-tools",
        options,
        Box::new(|_cc| Ok(Box::new(HaucetApp::default()))),
    )
}
