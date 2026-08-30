use crate::app::HaucetApp;
use crate::pages::{Page, ResultView, run_button};
use crate::util::{
    human_size, message_box, open_in_file_manager, section, sibling_output_path, trimmed_non_empty,
    update_derived_path,
};
use common::package::{PackageIndex, UpdateLayout};
use eframe::egui;
use egui_extras::{Column, TableBuilder};

#[derive(Debug)]
enum PendingOp {
    Inspect { input: String, layout: UpdateLayout },
    Unpack { output: String },
}

#[derive(Debug, Default)]
pub struct PackagePage {
    pub input: String,
    pub output: String,
    pub tools_dir: String,
    pub layout: UpdateLayout,
    pub force: bool,
    pub all_erofs: bool,
    pub custom_partitions: String,

    pub index: Option<PackageIndex>,
    pub checked: Vec<bool>,
    pub inspect_message: Option<String>,
    pub result: Option<ResultView>,
    inspect_pending: bool,
    input_initialized: bool,
    input_dirty: bool,
    pending: Option<PendingOp>,
    auto_output: Option<String>,
}

impl PackagePage {
    pub fn select_input(&mut self, input: String) {
        self.input = input;
        self.input_dirty = false;
        self.update_auto_output();
        self.queue_inspect();
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        self.poll_result(app);

        if !self.input_initialized {
            self.input_initialized = true;
            if !self.input.trim().is_empty() {
                self.queue_inspect();
            }
        }

        egui::ScrollArea::vertical()
            .id_salt("package-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let input_response = path_row(
                    ui,
                    app,
                    "更新文件",
                    &mut self.input,
                    "选择文件",
                    Some(&["zip", "bin"]),
                );
                if input_response.changed {
                    self.input_dirty = true;
                    self.invalidate_inspection();
                }
                if input_response.committed && self.input_dirty {
                    self.input_dirty = false;
                    self.update_auto_output();
                    self.queue_inspect();
                }
                ui.add_space(6.0);

                path_row(ui, app, "输出目录", &mut self.output, "选择目录", None);

                let drops = app.take_drops(ui.ctx());
                if let Some(path) = drops.first() {
                    self.select_input(path.display().to_string());
                }

                if self.inspect_pending && !app.job_running() && !self.input.trim().is_empty() {
                    self.start_inspect(app);
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let ready = !app.job_running()
                        && !self.input.trim().is_empty()
                        && !self.output.trim().is_empty();
                    if run_button(ui, "开始解包", ready, None).clicked() {
                        let partitions = self.selected_partitions();
                        let output = self.output.trim().to_owned();
                        self.pending = Some(PendingOp::Unpack {
                            output: output.clone(),
                        });
                        self.result = None;
                        app.start_job(crate::worker::JobOp::PackageUnpack {
                            input: self.input.trim().to_owned(),
                            output,
                            partitions,
                            all_erofs: self.all_erofs,
                            layout: self.layout,
                            force: self.force,
                            tools_dir: trimmed_non_empty(&self.tools_dir),
                        });
                    }
                    if app.job_running() {
                        ui.label(egui::RichText::new("任务进行中…").weak());
                    }
                });

                ui.add_space(4.0);
                let layout_before = self.layout;
                egui::CollapsingHeader::new("高级选项")
                    .id_salt("package-advanced")
                    .show(ui, |ui| {
                        egui::Grid::new("package-advanced-grid")
                            .num_columns(2)
                            .spacing([16.0, 8.0])
                            .show(ui, |ui| {
                                ui.label("update.bin 布局");
                                egui::ComboBox::from_id_salt("package-layout")
                                    .selected_text(layout_label(self.layout))
                                    .show_ui(ui, |ui| {
                                        for layout in
                                            [UpdateLayout::Auto, UpdateLayout::L1, UpdateLayout::L2]
                                        {
                                            ui.selectable_value(
                                                &mut self.layout,
                                                layout,
                                                layout_label(layout),
                                            );
                                        }
                                    });
                                ui.end_row();
                                ui.label("自定义分区");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.custom_partitions)
                                        .hint_text(
                                            "可选: 逗号分隔, 如 system, vendor; 留空则使用下方勾选",
                                        )
                                        .desired_width(380.0),
                                );
                                ui.end_row();
                                ui.label("选项");
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut self.force, "覆盖已存在的输出");
                                    ui.checkbox(&mut self.all_erofs, "只解包 EROFS 分区");
                                });
                                ui.end_row();
                            });
                    });

                if self.layout != layout_before && !self.input.trim().is_empty() {
                    self.queue_inspect();
                }

                ui.add_space(8.0);
                self.show_result(ui);
                if let Some(message) = &self.inspect_message {
                    message_box(ui, egui::Color32::from_rgb(90, 170, 255), message);
                    ui.add_space(6.0);
                }
                if let Some(index) = self.index.clone() {
                    self.partition_table(ui, &index);
                }
            });
    }

    fn queue_inspect(&mut self) {
        self.inspect_pending = !self.input.trim().is_empty();
        self.clear_inspection();
    }

    fn update_auto_output(&mut self) {
        let next = sibling_output_path(&self.input, "package", "-work");
        update_derived_path(&mut self.output, &mut self.auto_output, next);
    }

    fn invalidate_inspection(&mut self) {
        self.inspect_pending = false;
        self.clear_inspection();
    }

    fn clear_inspection(&mut self) {
        self.index = None;
        self.checked.clear();
        self.inspect_message = None;
        self.result = None;
    }

    fn start_inspect(&mut self, app: &mut HaucetApp) {
        let input = self.input.trim().to_owned();
        let layout = self.layout;
        self.inspect_pending = false;
        self.clear_inspection();
        self.pending = Some(PendingOp::Inspect {
            input: input.clone(),
            layout,
        });
        app.start_job(crate::worker::JobOp::PackageInspect {
            input: input.clone(),
            layout,
        });
    }

    fn poll_result(&mut self, app: &mut HaucetApp) {
        let Some(result) = app.take_result(Page::Package) else {
            return;
        };
        let Some(pending) = self.pending.take() else {
            return;
        };
        match pending {
            PendingOp::Inspect { input, layout } => {
                if input != self.input.trim() || layout != self.layout {
                    return;
                }
                if !result.ok {
                    self.inspect_message = Some(result.summary.clone());
                    self.result = Some(ResultView {
                        ok: false,
                        summary: result.summary,
                        output: String::new(),
                    });
                    return;
                }
                let index = match result
                    .payload
                    .and_then(|payload| serde_json::from_value::<PackageIndex>(payload).ok())
                {
                    Some(index) => index,
                    None => {
                        let summary = "无法解析更新包组件索引".to_owned();
                        self.inspect_message = Some(summary.clone());
                        self.result = Some(ResultView {
                            ok: false,
                            summary,
                            output: String::new(),
                        });
                        return;
                    }
                };
                let image_count = index
                    .components
                    .iter()
                    .filter(|component| component.component_type == 0)
                    .count();
                self.checked = index
                    .components
                    .iter()
                    .map(|component| component.component_type == 0)
                    .collect();
                self.inspect_message = Some(format!(
                    "包内共 {} 个组件, {} 个分区镜像",
                    index.components.len(),
                    image_count
                ));
                self.index = Some(index);
            }
            PendingOp::Unpack { output } => {
                self.result = Some(ResultView {
                    ok: result.ok,
                    summary: result.summary,
                    output: if result.ok { output } else { String::new() },
                });
            }
        }
    }

    fn selected_partitions(&self) -> Vec<String> {
        let custom = self
            .custom_partitions
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if !custom.is_empty() {
            return custom;
        }
        let Some(index) = &self.index else {
            return Vec::new();
        };
        index
            .components
            .iter()
            .zip(self.checked.iter())
            .filter(|(_, checked)| **checked)
            .map(|(component, _)| component.name.clone())
            .collect()
    }

    fn partition_table(&mut self, ui: &mut egui::Ui, index: &PackageIndex) {
        section(ui, "包内分区");
        ui.horizontal(|ui| {
            if ui.button("全选").clicked() {
                for checked in &mut self.checked {
                    *checked = true;
                }
            }
            if ui.button("全不选").clicked() {
                for checked in &mut self.checked {
                    *checked = false;
                }
            }
        });
        ui.add_space(4.0);
        let components = &index.components;
        let mut checked = self.checked.clone();
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::auto().at_least(50.0))
            .column(Column::auto().at_least(200.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::remainder().at_least(120.0))
            .header(24.0, |mut header| {
                header.col(|ui| {
                    ui.strong("解包");
                });
                header.col(|ui| {
                    ui.strong("分区");
                });
                header.col(|ui| {
                    ui.strong("类型");
                });
                header.col(|ui| {
                    ui.strong("大小");
                });
                header.col(|ui| {
                    ui.strong("数据偏移");
                });
            })
            .body(|mut body| {
                for (position, component) in components.iter().enumerate() {
                    let selectable = component.component_type == 0;
                    body.row(20.0, |mut row| {
                        row.col(|ui| {
                            if selectable {
                                ui.checkbox(&mut checked[position], "");
                            } else {
                                ui.weak("—");
                            }
                        });
                        row.col(|ui| {
                            ui.label(&component.output_name);
                        });
                        row.col(|ui| {
                            ui.label(component_type_label(component.component_type));
                        });
                        row.col(|ui| {
                            ui.label(human_size(component.size));
                        });
                        row.col(|ui| {
                            ui.label(
                                egui::RichText::new(crate::util::hex64(component.data_offset))
                                    .monospace()
                                    .weak(),
                            );
                        });
                    });
                }
            });
        self.checked = checked;
    }

    fn show_result(&self, ui: &mut egui::Ui) {
        let Some(result) = &self.result else {
            return;
        };
        ui.add_space(6.0);
        if result.ok {
            message_box(ui, egui::Color32::from_rgb(90, 200, 120), &result.summary);
            if !result.output.is_empty() && ui.button("打开输出目录").clicked() {
                open_in_file_manager(std::path::Path::new(&result.output));
            }
        } else {
            message_box(ui, egui::Color32::from_rgb(230, 90, 90), &result.summary);
        }
    }
}

fn layout_label(layout: UpdateLayout) -> &'static str {
    match layout {
        UpdateLayout::Auto => "自动检测",
        UpdateLayout::L1 => "L1",
        UpdateLayout::L2 => "L2",
    }
}

#[derive(Debug, Default)]
struct PathRowResponse {
    changed: bool,
    committed: bool,
}

fn path_row(
    ui: &mut egui::Ui,
    app: &mut HaucetApp,
    label: &str,
    value: &mut String,
    button: &str,
    filter: Option<&[&str]>,
) -> PathRowResponse {
    let mut result = PathRowResponse::default();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).strong());
        let response = ui.add(
            egui::TextEdit::singleline(value)
                .hint_text("路径或拖放文件到这里")
                .desired_width(ui.available_width() - 150.0),
        );
        result.changed |= response.changed();
        result.committed |= response.lost_focus();
        if ui.button(button).clicked() {
            let filters: &[(&str, &[&str])] = match filter {
                Some(extensions) => &[("更新包", extensions)],
                None => &[],
            };
            let picked = if filter.is_some() {
                app.pick_file(label, filters)
            } else {
                app.pick_dir(label)
            };
            if let Some(path) = picked {
                *value = path.display().to_string();
                result.changed = true;
                result.committed = true;
            }
        }
    });
    result
}

fn component_type_label(component_type: u8) -> String {
    match component_type {
        0 => "镜像".to_owned(),
        1 => "压缩包".to_owned(),
        other => format!("类型 {other}"),
    }
}
