use crate::app::HaucetApp;
use crate::pages::{Page, ResultView, run_button};
use crate::util::{message_box, open_in_file_manager};
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
    fn spec(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Hex => "hex",
        }
    }
}

#[derive(Debug, Default)]
pub struct NvmePage {
    image: String,
    filter: String,
    key: String,
    value: String,
    value_mode: ValueMode,
    summary: Option<NveImageSummary>,
    selected_slot: Option<usize>,
    result: Option<ResultView>,
    inspect_pending: bool,
    pending_edit: bool,
}

impl NvmePage {
    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        self.poll_result(app);

        egui::ScrollArea::vertical()
            .id_salt("nvme-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("NVMe / NVE 编辑器").strong().size(22.0));
                        ui.label(egui::RichText::new("查看与修改 HiSilicon NVE 条目").weak());
                    });
                    if app.job_running() {
                        ui.add_space(8.0);
                        ui.add(egui::Spinner::new().size(16.0));
                    }
                });
                ui.add_space(12.0);

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

                if let Some(summary) = self.summary.clone() {
                    ui.add_space(12.0);
                    self.render_summary(ui, &summary);
                    ui.add_space(12.0);
                    self.render_editor(ui, app, &summary);
                    self.render_result(ui);
                    ui.add_space(12.0);
                    self.render_items(ui, &summary);
                    ui.add_space(10.0);
                    self.render_blocks(ui, &summary.blocks);
                }

                ui.add_space(20.0);
            });
    }

    fn image_row(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("镜像文件").strong());
            let field_width = (ui.available_width() - 112.0).max(120.0);
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.image)
                    .hint_text("选择 NVME 分区镜像")
                    .font(egui::TextStyle::Monospace)
                    .desired_width(field_width),
            );
            if response.changed() {
                self.summary = None;
                self.selected_slot = None;
                self.result = None;
                if std::path::Path::new(self.image.trim()).is_file() {
                    self.inspect_pending = true;
                }
            }
            if ui.button("选择文件").clicked()
                && let Some(path) = app.pick_file("选择 NVE 镜像", &[])
            {
                self.set_image(path.display().to_string());
            }
        });
    }

    fn render_summary(&self, ui: &mut egui::Ui, summary: &NveImageSummary) {
        let blocks = format!("{} / {}", summary.active_blocks, summary.total_blocks);
        let entries = summary.valid_items.to_string();
        let version = format!("版本 {}", summary.version);
        let (crc_value, crc_detail, crc_color) = if !summary.crc_supported {
            (
                "未启用".to_owned(),
                "检测到的副本未声明 CRC32C".to_owned(),
                None,
            )
        } else if summary.crc_invalid == 0 {
            (
                "全部通过".to_owned(),
                format!("{} 项已校验", summary.crc_valid),
                Some(egui::Color32::from_rgb(95, 190, 125)),
            )
        } else {
            (
                format!("{} 项异常", summary.crc_invalid),
                format!("{} 项通过", summary.crc_valid),
                Some(egui::Color32::from_rgb(225, 155, 60)),
            )
        };

        ui.label(egui::RichText::new("镜像概览").strong().size(16.0));
        ui.add_space(5.0);
        if ui.available_width() >= 760.0 {
            ui.columns(3, |columns| {
                summary_stat(&mut columns[0], "活动块", &blocks, "活动 / 总数", None);
                summary_stat(&mut columns[1], "有效条目", &entries, &version, None);
                summary_stat(
                    &mut columns[2],
                    "CRC32C",
                    &crc_value,
                    &crc_detail,
                    crc_color,
                );
            });
        } else {
            ui.columns(2, |columns| {
                summary_stat(&mut columns[0], "活动块", &blocks, "活动 / 总数", None);
                summary_stat(&mut columns[1], "有效条目", &entries, &version, None);
            });
            ui.add_space(6.0);
            summary_stat(ui, "CRC32C", &crc_value, &crc_detail, crc_color);
        }
        if summary.crc_invalid != 0 {
            ui.add_space(8.0);
            message_box(
                ui,
                egui::Color32::from_rgb(225, 155, 60),
                "镜像中存在 CRC32C 异常副本；异常副本不会作为当前数据或写入来源。",
            );
        }
    }

    fn render_editor(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp, summary: &NveImageSummary) {
        let selected = self
            .selected_slot
            .and_then(|slot| {
                summary.items.iter().find(|item| {
                    item.slot == slot && item.name.eq_ignore_ascii_case(self.key.trim())
                })
            })
            .cloned();
        let fill = ui.visuals().faint_bg_color;
        egui::Frame::group(ui.style())
            .fill(fill)
            .corner_radius(6)
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("编辑条目").strong().size(16.0));
                    if let Some(item) = &selected {
                        ui.label(
                            egui::RichText::new(format!("#{}", item.number))
                                .small()
                                .weak(),
                        );
                        if item.kernel_protected {
                            ui.label(
                                egui::RichText::new("内核保护")
                                    .small()
                                    .strong()
                                    .color(egui::Color32::from_rgb(225, 155, 60)),
                            );
                        }
                    }
                });
                ui.add_space(8.0);

                let previous_mode = self.value_mode;
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("条目名称").strong());
                    ui.add(
                        egui::TextEdit::singleline(&mut self.key)
                            .hint_text("例如 SN、IMEI、FBLOCK")
                            .font(egui::TextStyle::Monospace)
                            .desired_width(190.0),
                    );
                    ui.add_space(12.0);
                    ui.label(egui::RichText::new("值格式").strong());
                    ui.selectable_value(&mut self.value_mode, ValueMode::Text, "文本");
                    ui.selectable_value(&mut self.value_mode, ValueMode::Hex, "十六进制");
                });
                if previous_mode != self.value_mode
                    && let Some(item) = &selected
                {
                    self.value = match self.value_mode {
                        ValueMode::Text if !item.value_text.is_empty() => item.value_text.clone(),
                        _ => item.value_hex.clone(),
                    };
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("条目值").strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("{} 个字符", self.value.chars().count()))
                                .small()
                                .weak(),
                        );
                    });
                });
                let hint = if self.value_mode == ValueMode::Hex {
                    "输入十六进制字节，例如 0001ff"
                } else {
                    "输入文本内容"
                };
                ui.add(
                    egui::TextEdit::multiline(&mut self.value)
                        .hint_text(hint)
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(2)
                        .desired_width(ui.available_width()),
                );

                ui.add_space(6.0);
                let ready = !app.job_running()
                    && !self.image.trim().is_empty()
                    && !self.key.trim().is_empty()
                    && self.summary.is_some();
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("写入下一代副本前会自动创建带时间戳的备份")
                            .small()
                            .weak(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if run_button(
                            ui,
                            "写入并备份",
                            ready,
                            Some("修改源镜像中的条目，并按头部声明规则更新校验"),
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
                            });
                        }
                    });
                });
            });
    }

    fn render_items(&mut self, ui: &mut egui::Ui, summary: &NveImageSummary) {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("NVE 条目").strong().size(16.0));
            ui.add_space(10.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("搜索名称、编号或值")
                    .desired_width(280.0),
            );
            if !self.filter.is_empty() && ui.button("×").on_hover_text("清除搜索").clicked() {
                self.filter.clear();
            }
        });
        ui.add_space(5.0);

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
        ui.label(
            egui::RichText::new(format!("显示 {} / {} 项", items.len(), summary.items.len()))
                .small()
                .weak(),
        );
        ui.add_space(4.0);

        let selected_slot = self.selected_slot;
        let table_height = (ui.clip_rect().height() * 0.42).clamp(260.0, 420.0);
        TableBuilder::new(ui)
            .id_salt("nvme-items-table")
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .min_scrolled_height(table_height)
            .max_scroll_height(table_height)
            .auto_shrink([false, false])
            .column(Column::exact(72.0))
            .column(Column::initial(150.0).at_least(110.0).clip(true))
            .column(Column::exact(68.0))
            .column(Column::remainder().at_least(180.0).clip(true))
            .column(Column::exact(82.0))
            .header(30.0, |mut header| {
                header.col(|ui| {
                    ui.strong("编号");
                });
                header.col(|ui| {
                    ui.strong("名称");
                });
                header.col(|ui| {
                    ui.strong("大小");
                });
                header.col(|ui| {
                    ui.strong("值");
                });
                header.col(|ui| {
                    ui.strong("CRC");
                });
            })
            .body(|mut body| {
                if items.is_empty() {
                    body.row(44.0, |mut row| {
                        row.col(|_| {});
                        row.col(|_| {});
                        row.col(|_| {});
                        row.col(|ui| {
                            ui.label(egui::RichText::new("没有匹配的条目").weak());
                        });
                        row.col(|_| {});
                    });
                    return;
                }
                body.rows(30.0, items.len(), |mut row| {
                    let item = &items[row.index()];
                    let selected = selected_slot == Some(item.slot);
                    row.set_selected(selected);
                    row.col(|ui| {
                        ui.label(egui::RichText::new(item.number.to_string()).monospace());
                    });
                    row.col(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&item.name).strong().monospace());
                            if item.kernel_protected {
                                ui.label(
                                    egui::RichText::new("保护")
                                        .small()
                                        .color(egui::Color32::from_rgb(225, 155, 60)),
                                );
                            }
                        });
                    });
                    row.col(|ui| {
                        ui.label(format!("{} B", item.valid_size));
                    });
                    row.col(|ui| {
                        let display = if item.value_text.is_empty() {
                            format!("0x{}", item.value_hex)
                        } else {
                            item.value_text.clone()
                        };
                        let response = ui
                            .add(
                                egui::Label::new(egui::RichText::new(&display).monospace())
                                    .truncate()
                                    .sense(egui::Sense::click()),
                            )
                            .on_hover_ui(|ui| {
                                ui.set_max_width(520.0);
                                ui.label(egui::RichText::new("十六进制值").small().weak());
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&item.value_hex).monospace(),
                                    )
                                    .wrap(),
                                );
                                ui.label(
                                    egui::RichText::new("双击可复制当前显示值").small().weak(),
                                );
                            });
                        if response.double_clicked() {
                            ui.ctx().copy_text(display);
                        }
                    });
                    row.col(|ui| {
                        if !item.crc_supported {
                            ui.label(egui::RichText::new("未启用").weak());
                        } else if item.crc_valid {
                            ui.label(
                                egui::RichText::new("通过")
                                    .strong()
                                    .color(egui::Color32::from_rgb(95, 190, 125)),
                            );
                        } else {
                            ui.label(
                                egui::RichText::new("异常")
                                    .strong()
                                    .color(egui::Color32::from_rgb(225, 90, 90)),
                            );
                        }
                    });
                    let response = row
                        .response()
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if response.clicked() {
                        self.select_item(item);
                    }
                });
            });
    }

    fn render_blocks(&self, ui: &mut egui::Ui, blocks: &[NveBlockSummary]) {
        egui::CollapsingHeader::new(format!("检测到的副本（{}）", blocks.len()))
            .id_salt("nvme-blocks")
            .default_open(false)
            .show(ui, |ui| {
                TableBuilder::new(ui)
                    .id_salt("nvme-blocks-table")
                    .striped(true)
                    .max_scroll_height(220.0)
                    .column(Column::auto().at_least(80.0))
                    .column(Column::auto().at_least(110.0))
                    .column(Column::auto().at_least(90.0))
                    .column(Column::auto().at_least(90.0))
                    .column(Column::remainder().at_least(120.0))
                    .header(24.0, |mut header| {
                        header.col(|ui| {
                            ui.strong("块");
                        });
                        header.col(|ui| {
                            ui.strong("偏移");
                        });
                        header.col(|ui| {
                            ui.strong("代次");
                        });
                        header.col(|ui| {
                            ui.strong("条目");
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
                                    if block.crc_supported {
                                        ui.label(format!(
                                            "{} 通过 / {} 异常",
                                            block.crc_valid, block.crc_invalid
                                        ));
                                    } else {
                                        ui.label("未启用");
                                    }
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
        if result.ok && !result.output.is_empty() && ui.button("打开备份位置").clicked() {
            open_in_file_manager(std::path::Path::new(&result.output));
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
                        summary: format!("无法解析 NVE 读取结果: {error}"),
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
        if item.value_text.is_empty() {
            self.value_mode = ValueMode::Hex;
        }
        self.value = match self.value_mode {
            ValueMode::Text if !item.value_text.is_empty() => item.value_text.clone(),
            _ => item.value_hex.clone(),
        };
    }
}

fn summary_stat(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    detail: &str,
    color: Option<egui::Color32>,
) {
    let fill = ui.visuals().faint_bg_color;
    egui::Frame::group(ui.style())
        .fill(fill)
        .corner_radius(6)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_min_height(66.0);
            ui.label(egui::RichText::new(label).small().weak());
            let mut value_text = egui::RichText::new(value).strong().size(19.0);
            if let Some(color) = color {
                value_text = value_text.color(color);
            }
            ui.add(egui::Label::new(value_text).truncate());
            ui.label(egui::RichText::new(detail).small().weak());
        });
}
