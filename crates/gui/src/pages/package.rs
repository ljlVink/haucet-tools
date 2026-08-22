use crate::app::HaucetApp;
use crate::pages::{LayoutChoice, Page, ResultView, layout_label, run_button};
use crate::util::{human_size, message_box, open_in_file_manager, section};
use common::formats::update_bin::PackageIndex;
use eframe::egui;
use egui_extras::{Column, TableBuilder};

#[derive(Debug, Default)]
pub struct PackagePage {
    pub input: String,
    pub output: String,
    pub tools_dir: String,
    pub layout: LayoutChoice,
    pub force: bool,
    pub all_erofs: bool,
    pub custom_partitions: String,

    pub index: Option<PackageIndex>,
    pub checked: Vec<bool>,
    pub inspect_message: Option<String>,
    pub result: Option<ResultView>,
}

impl PackagePage {
    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        self.poll_result(app);

        egui::ScrollArea::vertical()
            .id_salt("package-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_space(6.0);
                path_row(
                    ui,
                    app,
                    "更新包文件",
                    &mut self.input,
                    "选择文件",
                    Some("update_full_base.zip"),
                );

                let drops = app.take_drops(ui.ctx());
                if let Some(path) = drops.first() {
                    self.input = path.display().to_string();
                    if self.output.trim().is_empty() {
                        self.output = default_output(&self.input);
                    }
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if run_button(
                        ui,
                        "读取包内容",
                        !app.job_running() && !self.input.trim().is_empty(),
                        Some("解析包内的 update.bin 组件表，无需解包"),
                    )
                    .clicked()
                    {
                        app.start_job(crate::worker::JobOp::PackageInspect {
                            input: self.input.trim().to_owned(),
                            layout: self.layout.spec().to_owned(),
                        });
                    }
                    if ui.button("打开日志面板").clicked() {
                        app.settings.show_log = true;
                    }
                });

                ui.add_space(4.0);
                egui::CollapsingHeader::new("高级选项")
                    .id_salt("package-advanced")
                    .show(ui, |ui| {
                        egui::Grid::new("package-advanced-grid")
                            .num_columns(2)
                            .spacing([16.0, 8.0])
                            .show(ui, |ui| {
                                ui.label("update.bin 布局");
                                egui::ComboBox::from_id_salt("package-layout")
                                    .selected_text(self.layout.label())
                                    .show_ui(ui, |ui| {
                                        for layout in LayoutChoice::ALL {
                                            ui.selectable_value(&mut self.layout, layout, layout.label());
                                        }
                                    });
                                ui.end_row();
                                ui.label("EROFS 工具目录");
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.tools_dir)
                                            .hint_text("留空则自动查找 bin/")
                                            .desired_width(300.0),
                                    );
                                    if ui.button("浏览…").clicked() {
                                        if let Some(dir) = app.pick_dir("选择 EROFS 工具目录") {
                                            self.tools_dir = dir.display().to_string();
                                        }
                                    }
                                });
                                ui.end_row();
                                ui.label("自定义分区");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.custom_partitions)
                                        .hint_text("可选：逗号分隔，如 system, vendor；留空则使用下方勾选")
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

                ui.add_space(8.0);
                if let Some(message) = &self.inspect_message {
                    message_box(ui, egui::Color32::from_rgb(90, 170, 255), message);
                    ui.add_space(6.0);
                }
                if let Some(index) = self.index.clone() {
                    self.partition_table(ui, &index);
                }

                ui.add_space(10.0);
                section(ui, "输出");
                path_row(
                    ui,
                    app,
                    "输出目录",
                    &mut self.output,
                    "选择目录",
                    None,
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let ready = !app.job_running()
                        && !self.input.trim().is_empty()
                        && !self.output.trim().is_empty();
                    if run_button(ui, "开始解包", ready, None).clicked() {
                        let partitions = self.selected_partitions();
                        app.start_job(crate::worker::JobOp::PackageUnpack {
                            input: self.input.trim().to_owned(),
                            output: self.output.trim().to_owned(),
                            partitions,
                            all_erofs: self.all_erofs,
                            layout: self.layout.spec().to_owned(),
                            force: self.force,
                            tools_dir: optional(&self.tools_dir),
                        });
                    }
                    if app.job_running() {
                        ui.label(egui::RichText::new("任务进行中…").weak());
                    }
                });
                ui.add_space(10.0);
                self.show_result(ui);
            });
    }

    fn poll_result(&mut self, app: &mut HaucetApp) {
        let Some(result) = app.take_result(Page::Package) else {
            return;
        };
        if !result.ok {
            // 区分“读取包内容”和“解包”两种失败
            if self.index.is_none() && self.inspect_message.is_none() {
                self.inspect_message = Some(result.summary.clone());
            }
            self.result = Some(ResultView {
                ok: false,
                summary: result.summary.clone(),
                output: String::new(),
            });
            return;
        }
        if let Some(payload) = result.payload {
            if let Ok(index) = serde_json::from_value::<PackageIndex>(payload) {
                // 读取包内容成功
                let image_count = index.components.iter().filter(|c| c.component_type == 0).count();
                self.checked = index
                    .components
                    .iter()
                    .map(|component| component.component_type == 0)
                    .collect();
                self.inspect_message = Some(format!(
                    "包内共 {} 个组件（其中 {} 个分区镜像），已自动勾选全部镜像分区；其余文件会解包到 package/ 目录。",
                    index.components.len(),
                    image_count
                ));
                self.index = Some(index);
                return;
            }
        }
        self.result = Some(ResultView {
            ok: true,
            summary: result.summary.clone(),
            output: self.output.trim().to_owned(),
        });
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
        section(ui, "包内分区（勾选要解包的镜像）");
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
            ui.label(
                egui::RichText::new(format!(
                    "布局 {} · 数据偏移 {}",
                    layout_label(&index.layout),
                    crate::util::hex64(index.data_offset)
                ))
                .weak(),
            );
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
            .header(22.0, |mut header| {
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
                    body.row(22.0, |mut row| {
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
            if !result.output.is_empty()
                && ui.button("打开输出目录").clicked()
            {
                open_in_file_manager(std::path::Path::new(&result.output));
            }
        } else {
            message_box(ui, egui::Color32::from_rgb(230, 90, 90), &result.summary);
        }
    }
}

fn path_row(
    ui: &mut egui::Ui,
    app: &mut HaucetApp,
    label: &str,
    value: &mut String,
    button: &str,
    filter: Option<&str>,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).strong());
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text("路径或拖放文件到这里")
                .desired_width(ui.available_width() - 150.0),
        );
        if ui.button(button).clicked() {
            let filters: &[(&str, &[&str])] = match filter {
                Some(ext) => &[("文件", &[ext])],
                None => &[],
            };
            let picked = if filter.is_some() {
                app.pick_file(label, filters)
            } else {
                app.pick_dir(label)
            };
            if let Some(path) = picked {
                *value = path.display().to_string();
            }
        }
    });
}

fn component_type_label(component_type: u8) -> String {
    match component_type {
        0 => "镜像".to_owned(),
        1 => "压缩包".to_owned(),
        other => format!("类型 {other}"),
    }
}

fn optional(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn default_output(input: &str) -> String {
    let path = std::path::Path::new(input);
    let parent = path
        .parent()
        .map(|parent| parent.display().to_string())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "package".to_owned());
    let name = format!("{stem}-work");
    if parent.is_empty() {
        name
    } else {
        format!("{parent}/{name}")
    }
}
