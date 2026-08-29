use crate::app::HaucetApp;
use crate::pages::{Page, ResultView, run_button};
use crate::util::{human_size, kv, message_box, open_in_file_manager, section};
use common::nvme::{NveBlockSummary, NveImageSummary, NveItemSummary};
use eframe::egui;
use egui_extras::{Column, TableBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ValueMode {
    #[default]
    Text,
    Hex,
}

impl ValueMode {
    fn label(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Hex => "Hex",
        }
    }

    fn spec(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Hex => "hex",
        }
    }
}

#[derive(Debug)]
pub struct NvmePage {
    image: String,
    filter: String,
    key: String,
    value: String,
    value_mode: ValueMode,
    sync_all_blocks: bool,
    auto_hash_usrkey: bool,
    summary: Option<NveImageSummary>,
    selected_slot: Option<usize>,
    result: Option<ResultView>,
    inspect_pending: bool,
    pending_edit: bool,
}

impl Default for NvmePage {
    fn default() -> Self {
        Self {
            image: String::new(),
            filter: String::new(),
            key: String::new(),
            value: String::new(),
            value_mode: ValueMode::default(),
            sync_all_blocks: true,
            auto_hash_usrkey: true,
            summary: None,
            selected_slot: None,
            result: None,
            inspect_pending: false,
            pending_edit: false,
        }
    }
}

impl NvmePage {
    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        self.poll_result(app);

        egui::ScrollArea::vertical()
            .id_salt("nvme-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_space(4.0);
                ui.label(egui::RichText::new("NVMe / NVE 编辑器").strong().size(22.0));
                ui.label(
                    egui::RichText::new("查看 HiSilicon NVE 条目")
                        .weak(),
                );
                ui.add_space(14.0);

                self.image_row(ui, app);
                if let Some(path) = app.take_drops(ui.ctx()).first().cloned() {
                    self.set_image(path.display().to_string());
                }

                if self.inspect_pending && !app.job_running() && !self.image.trim().is_empty() {
                    self.inspect_pending = false;
                    app.start_job(crate::worker::JobOp::NvmeInspect {
                        image: self.image.trim().to_owned(),
                    });
                }

                if app.job_running() {
                    ui.add(egui::Spinner::new().size(16.0));
                }

                if let Some(summary) = self.summary.clone() {
                    ui.add_space(8.0);
                    self.render_summary(ui, &summary);
                    ui.add_space(8.0);
                    self.render_editor(ui, app);
                    ui.add_space(8.0);
                    self.render_items(ui, &summary);
                    ui.add_space(8.0);
                    self.render_blocks(ui, &summary.blocks);
                }

                self.render_result(ui);
                ui.add_space(20.0);
            });
    }

    fn image_row(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Image").strong());
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.image)
                    .hint_text("nvme.img or a raw NVME dump")
                    .desired_width(ui.available_width() - 220.0),
            );
            if response.changed() {
                self.summary = None;
                self.selected_slot = None;
                self.result = None;
                if std::path::Path::new(self.image.trim()).is_file() {
                    self.inspect_pending = true;
                }
            }
            if ui.button("Choose file").clicked()
                && let Some(path) = app.pick_file("Choose NVE image", &[])
            {
                self.set_image(path.display().to_string());
            }
        });
    }

    fn render_summary(&self, ui: &mut egui::Ui, summary: &NveImageSummary) {
        section(ui, "Image summary");
        egui::Grid::new("nvme-summary-grid")
            .num_columns(2)
            .spacing([18.0, 6.0])
            .show(ui, |ui| {
                kv(ui, "Size", human_size(summary.file_size));
                kv(
                    ui,
                    "Blocks",
                    format!(
                        "{} total / {} active",
                        summary.total_blocks, summary.active_blocks
                    ),
                );
                kv(
                    ui,
                    "Partition",
                    if summary.partition_name.is_empty() {
                        "Unknown".to_owned()
                    } else {
                        summary.partition_name.clone()
                    },
                );
                kv(ui, "Version", summary.version.to_string());
                kv(ui, "Entries", summary.valid_items.to_string());
                kv(
                    ui,
                    "CRC32C",
                    format!(
                        "{} valid / {} invalid",
                        summary.crc_valid, summary.crc_invalid
                    ),
                );
            });
        if summary.crc_invalid != 0 {
            message_box(
                ui,
                egui::Color32::from_rgb(230, 170, 40),
                "Some entries have invalid CRC32C. Editing an entry recalculates its CRC in every selected copy.",
            );
        }
    }

    fn render_editor(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        section(ui, "Edit entry");
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Key").strong());
            ui.add(
                egui::TextEdit::singleline(&mut self.key)
                    .hint_text("SN, IMEI, MACADDR, FBLOCK...")
                    .desired_width(180.0),
            );
            ui.label(egui::RichText::new("Value mode").weak());
            egui::ComboBox::from_id_salt("nvme-value-mode")
                .selected_text(self.value_mode.label())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.value_mode, ValueMode::Text, "Text");
                    ui.selectable_value(&mut self.value_mode, ValueMode::Hex, "Hex");
                });
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Value").strong());
            ui.add(
                egui::TextEdit::singleline(&mut self.value)
                    .hint_text(if self.value_mode == ValueMode::Hex {
                        "hex bytes, for example 0001ff"
                    } else {
                        "text value; FBLOCK accepts 0 or 1"
                    })
                    .desired_width(ui.available_width() - 90.0),
            );
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.sync_all_blocks, "Update all active copies");
            ui.checkbox(&mut self.auto_hash_usrkey, "Auto SHA-256 USRKEY");
        });
        ui.label(
            egui::RichText::new(
                "Writing replaces the source image in place. A timestamped backup is created first.",
            )
            .weak(),
        );
        ui.add_space(4.0);
        let ready = !app.job_running()
            && !self.image.trim().is_empty()
            && !self.key.trim().is_empty()
            && self.summary.is_some();
        if run_button(
            ui,
            "Write source (backup)",
            ready,
            Some("Create a backup, update the selected entry, and recalculate CRC32C"),
        )
        .clicked()
        {
            self.result = None;
            self.pending_edit = true;
            app.start_job(crate::worker::JobOp::NvmeEdit {
                image: self.image.trim().to_owned(),
                key: self.key.trim().to_owned(),
                value: self.value.clone(),
                value_format: self.value_mode.spec().to_owned(),
                sync_all_blocks: self.sync_all_blocks,
            });
        }
    }

    fn render_items(&mut self, ui: &mut egui::Ui, summary: &NveImageSummary) {
        section(ui, "Entries");
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Filter").weak());
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("name, number, value, or hex")
                    .desired_width(300.0),
            );
            ui.label(
                egui::RichText::new(format!("{} entries", summary.items.len()))
                    .weak()
                    .small(),
            );
        });
        ui.add_space(4.0);

        let filter = self.filter.trim().to_ascii_lowercase();
        let items = summary
            .items
            .iter()
            .filter(|item| {
                filter.is_empty()
                    || item.name.to_ascii_lowercase().contains(&filter)
                    || item.number.to_string().contains(&filter)
                    || item.value_text.to_ascii_lowercase().contains(&filter)
                    || item.value_hex.to_ascii_lowercase().contains(&filter)
            })
            .cloned()
            .collect::<Vec<NveItemSummary>>();
        let selected_slot = self.selected_slot;
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::auto().at_least(62.0))
            .column(Column::auto().at_least(120.0))
            .column(Column::auto().at_least(82.0))
            .column(Column::remainder().at_least(180.0))
            .column(Column::auto().at_least(80.0))
            .header(26.0, |mut header| {
                header.col(|ui| {
                    ui.strong("Number");
                });
                header.col(|ui| {
                    ui.strong("Name");
                });
                header.col(|ui| {
                    ui.strong("Size");
                });
                header.col(|ui| {
                    ui.strong("Value");
                });
                header.col(|ui| {
                    ui.strong("CRC");
                });
            })
            .body(|mut body| {
                for item in items {
                    body.row(26.0, |mut row| {
                        let selected = selected_slot == Some(item.slot);
                        row.col(|ui| {
                            if ui
                                .selectable_label(selected, item.number.to_string())
                                .clicked()
                            {
                                self.select_item(&item);
                            }
                        });
                        row.col(|ui| {
                            if ui.selectable_label(selected, &item.name).clicked() {
                                self.select_item(&item);
                            }
                        });
                        row.col(|ui| {
                            ui.label(item.valid_size.to_string());
                        });
                        row.col(|ui| {
                            let display = if item.value_text.is_empty() {
                                format!("0x{}", item.value_hex)
                            } else {
                                item.value_text.clone()
                            };
                            ui.label(display).on_hover_text(format!(
                                "hex: {}\n{}",
                                item.value_hex, item.description
                            ));
                        });
                        row.col(|ui| {
                            let (label, color) = if item.crc_valid {
                                ("OK", egui::Color32::from_rgb(90, 200, 120))
                            } else {
                                ("INVALID", egui::Color32::from_rgb(230, 90, 90))
                            };
                            ui.label(egui::RichText::new(label).color(color));
                        });
                    });
                }
            });
    }

    fn render_blocks(&self, ui: &mut egui::Ui, blocks: &[NveBlockSummary]) {
        egui::CollapsingHeader::new("Active copies")
            .id_salt("nvme-blocks")
            .default_open(false)
            .show(ui, |ui| {
                TableBuilder::new(ui)
                    .striped(true)
                    .column(Column::auto().at_least(80.0))
                    .column(Column::auto().at_least(110.0))
                    .column(Column::auto().at_least(90.0))
                    .column(Column::auto().at_least(90.0))
                    .column(Column::remainder().at_least(120.0))
                    .header(24.0, |mut header| {
                        header.col(|ui| {
                            ui.strong("Block");
                        });
                        header.col(|ui| {
                            ui.strong("Offset");
                        });
                        header.col(|ui| {
                            ui.strong("Age");
                        });
                        header.col(|ui| {
                            ui.strong("Entries");
                        });
                        header.col(|ui| {
                            ui.strong("CRC32C");
                        });
                    })
                    .body(|mut body| {
                        for block in blocks {
                            body.row(24.0, |mut row| {
                                row.col(|ui| {
                                    ui.label(block.block_index.to_string());
                                });
                                row.col(|ui| {
                                    ui.label(crate::util::hex64(block.offset));
                                });
                                row.col(|ui| {
                                    ui.label(block.age.to_string());
                                });
                                row.col(|ui| {
                                    ui.label(block.valid_items.to_string());
                                });
                                row.col(|ui| {
                                    ui.label(format!(
                                        "{} valid / {} invalid",
                                        block.crc_valid, block.crc_invalid
                                    ));
                                });
                            });
                        }
                    });
            });
    }

    fn render_result(&self, ui: &mut egui::Ui) {
        let Some(result) = &self.result else {
            return;
        };
        ui.add_space(6.0);
        let color = if result.ok {
            egui::Color32::from_rgb(90, 200, 120)
        } else {
            egui::Color32::from_rgb(230, 90, 90)
        };
        message_box(ui, color, &result.summary);
        if result.ok && !result.output.is_empty() {
            if ui.button("Open backup location").clicked() {
                open_in_file_manager(std::path::Path::new(&result.output));
            }
        }
    }

    fn poll_result(&mut self, app: &mut HaucetApp) {
        let Some(result) = app.take_result(Page::Nvme) else {
            return;
        };
        if !result.ok {
            self.result = Some(ResultView {
                ok: false,
                summary: result.summary,
                output: String::new(),
            });
            self.pending_edit = false;
            return;
        }

        if self.pending_edit {
            self.pending_edit = false;
            let edit = result.payload.and_then(|payload| {
                serde_json::from_value::<common::nvme::NveEditResult>(payload).ok()
            });
            self.result = Some(ResultView {
                ok: true,
                summary: result.summary,
                output: edit.map(|value| value.backup_path).unwrap_or_default(),
            });
            self.inspect_pending = true;
        } else if let Some(payload) = result.payload {
            match serde_json::from_value::<NveImageSummary>(payload) {
                Ok(summary) => {
                    if self.key.trim().is_empty() {
                        if let Some(item) = summary.items.first() {
                            self.select_item(item);
                        }
                    } else if let Some(item) = summary
                        .items
                        .iter()
                        .find(|item| item.name.eq_ignore_ascii_case(self.key.trim()))
                    {
                        self.select_item(item);
                    }
                    self.summary = Some(summary);
                    if self
                        .result
                        .as_ref()
                        .is_none_or(|result| !result.ok || result.output.is_empty())
                    {
                        self.result = None;
                    }
                }
                Err(error) => {
                    self.result = Some(ResultView {
                        ok: false,
                        summary: format!("Unable to parse NVE result: {error}"),
                        output: String::new(),
                    });
                }
            }
        }
    }

    fn set_image(&mut self, image: String) {
        self.image = image;
        self.summary = None;
        self.selected_slot = None;
        self.key.clear();
        self.value.clear();
        self.result = None;
        self.inspect_pending = true;
    }

    pub fn select_input(&mut self, image: String) {
        self.set_image(image);
    }

    fn select_item(&mut self, item: &NveItemSummary) {
        self.selected_slot = Some(item.slot);
        self.key = item.name.clone();
        self.value = match self.value_mode {
            ValueMode::Text if !item.value_text.is_empty() => item.value_text.clone(),
            _ => item.value_hex.clone(),
        };
    }
}
