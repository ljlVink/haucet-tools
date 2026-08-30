use crate::app::HaucetApp;
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageKind {
    #[default]
    Erofs,
    Ramdisk,
    Partition,
}

#[derive(Debug, Default)]
pub struct ImagesPage {
    pub kind: ImageKind,
    pub erofs: super::erofs::ErofsPage,
    pub ramdisk: super::ramdisk::RamdiskPage,
    pub partition: super::partition::PartitionPage,
}

impl ImagesPage {
    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        egui::ScrollArea::vertical()
            .id_salt("images-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.columns(3, |columns| {
                    if kind_card(
                        &mut columns[0],
                        self.kind == ImageKind::Erofs,
                        &tr!("images-erofs-title"),
                        &tr!("images-erofs-description"),
                    )
                    .clicked()
                    {
                        self.kind = ImageKind::Erofs;
                    }
                    if kind_card(
                        &mut columns[1],
                        self.kind == ImageKind::Ramdisk,
                        &tr!("images-ramdisk-title"),
                        &tr!("images-ramdisk-description"),
                    )
                    .clicked()
                    {
                        self.kind = ImageKind::Ramdisk;
                    }
                    if kind_card(
                        &mut columns[2],
                        self.kind == ImageKind::Partition,
                        &tr!("images-partition-title"),
                        &tr!("images-partition-description"),
                    )
                    .clicked()
                    {
                        self.kind = ImageKind::Partition;
                    }
                });

                ui.add_space(12.0);
                ui.separator();
                match self.kind {
                    ImageKind::Erofs => self.erofs.ui(ui, app),
                    ImageKind::Ramdisk => self.ramdisk.ui(ui, app),
                    ImageKind::Partition => self.partition.ui(ui, app),
                }
                ui.add_space(24.0);
            });
    }
}

fn kind_card(ui: &mut egui::Ui, selected: bool, title: &str, description: &str) -> egui::Response {
    let visuals = ui.visuals();
    let fill = if selected {
        visuals.selection.bg_fill.gamma_multiply(0.45)
    } else {
        visuals.extreme_bg_color
    };
    let stroke = if selected {
        egui::Stroke::new(1.5_f32, visuals.selection.stroke.color)
    } else {
        visuals.widgets.noninteractive.bg_stroke
    };

    egui::Frame::group(ui.style())
        .fill(fill)
        .stroke(stroke)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(48.0);
            ui.label(egui::RichText::new(title).strong().size(16.0));
            ui.label(egui::RichText::new(description).weak().size(13.0));
        })
        .response
        .interact(egui::Sense::click())
}
