use crate::app::HaucetApp;
use crate::pages::{LayoutChoice, Page, ResultView, layout_label, run_button};
use crate::util::{human_size, message_box, open_in_file_manager, section};
use common::package::PackageIndex;
use eframe::egui;
use egui_extras::{Column, TableBuilder};

#[derive(Debug, Default)]
pub struct UpdateBinPage {
    pub input: String,
    pub output: String,
    pub layout: LayoutChoice,
    pub force: bool,

    pub index: Option<PackageIndex>,
    pub checked: Vec<bool>,
    pub message: Option<String>,
    pub result: Option<ResultView>,
}

impl UpdateBinPage {
    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        self.poll_result(app);

        egui::ScrollArea::vertical()
            .id_salt("updatebin-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("update.bin 文件").strong());
                    ui.add(
                        egui::TextEdit::singleline(&mut self.input)
                            .hint_text("update.bin 或拖放文件到这里")
                            .desired_width(ui.available_width() - 320.0),
                    );
                    if ui.button("选择文件…").clicked()
                        && let Some(path) =
                            app.pick_file("选择 update.bin", &[("update.bin", &["bin"])])
                    {
                        self.input = path.display().to_string();
                        self.output = default_output(&self.input);
                    }
                });
                let drops = app.take_drops(ui.ctx());
                if let Some(path) = drops.first() {
                    self.input = path.display().to_string();
                    self.output = default_output(&self.input);
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if run_button(
                        ui,
                        "读取组件索引",
                        !app.job_running() && !self.input.trim().is_empty(),
                        Some("解析头部与组件表"),
                    )
                    .clicked()
                    {
                        app.start_job(crate::worker::JobOp::UpdateList {
                            input: self.input.trim().to_owned(),
                            layout: self.layout.spec().to_owned(),
                        });
                    }
                    ui.label("布局");
                    egui::ComboBox::from_id_salt("updatebin-layout")
                        .selected_text(self.layout.label())
                        .show_ui(ui, |ui| {
                            for layout in LayoutChoice::ALL {
                                ui.selectable_value(&mut self.layout, layout, layout.label());
                            }
                        });
                });

                ui.add_space(8.0);
                if let Some(message) = &self.message {
                    message_box(ui, egui::Color32::from_rgb(90, 170, 255), message);
                    ui.add_space(6.0);
                }
                if let Some(index) = self.index.clone() {
                    self.component_table(ui, &index);
                }

                ui.add_space(10.0);
                section(ui, "解包");
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("输出目录").strong());
                    ui.add(
                        egui::TextEdit::singleline(&mut self.output)
                            .desired_width(ui.available_width() - 260.0),
                    );
                    if ui.button("选择目录…").clicked()
                        && let Some(dir) = app.pick_dir("选择输出目录")
                    {
                        self.output = dir.display().to_string();
                    }
                    ui.checkbox(&mut self.force, "覆盖已存在文件");
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let ready = !app.job_running()
                        && !self.input.trim().is_empty()
                        && !self.output.trim().is_empty();
                    if run_button(ui, "解包勾选组件", ready, None).clicked() {
                        let selected = self.selected_names();
                        app.start_job(crate::worker::JobOp::UpdateUnpack {
                            input: self.input.trim().to_owned(),
                            output: self.output.trim().to_owned(),
                            layout: self.layout.spec().to_owned(),
                            force: self.force,
                            selected,
                        });
                    }
                    if run_button(
                        ui,
                        "解包全部组件",
                        ready,
                        Some("包括 VERSION.mbn、BOARD.list 等非镜像组件"),
                    )
                    .clicked()
                    {
                        app.start_job(crate::worker::JobOp::UpdateUnpack {
                            input: self.input.trim().to_owned(),
                            output: self.output.trim().to_owned(),
                            layout: self.layout.spec().to_owned(),
                            force: self.force,
                            selected: Vec::new(),
                        });
                    }
                });
                ui.add_space(10.0);
                self.show_result(ui);
            });
    }

    fn poll_result(&mut self, app: &mut HaucetApp) {
        let Some(result) = app.take_result(Page::UpdateBin) else {
            return;
        };
        if !result.ok {
            self.result = Some(ResultView {
                ok: false,
                summary: result.summary.clone(),
                output: String::new(),
            });
            return;
        }
        if let Some(payload) = result.payload
            && let Ok(index) = serde_json::from_value::<PackageIndex>(payload)
        {
            self.checked = vec![true; index.components.len()];
            self.message = Some(format!(
                "共 {} 个组件(检测到 {} 布局)。双击组件名可复制。",
                index.components.len(),
                layout_label(&index.layout)
            ));
            self.index = Some(index);
            return;
        }
        self.result = Some(ResultView {
            ok: true,
            summary: result.summary.clone(),
            output: self.output.trim().to_owned(),
        });
    }

    fn selected_names(&self) -> Vec<String> {
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

    fn component_table(&mut self, ui: &mut egui::Ui, index: &PackageIndex) {
        section(ui, "组件列表");
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
            let mut list_text = format!(
                "layout={:?} components={} data_offset={}\n",
                index.layout,
                index.components.len(),
                index.data_offset
            );
            for (number, component) in index.components.iter().enumerate() {
                list_text.push_str(&format!(
                    "{:>3}  {:<36} type={} size={} offset={}\n",
                    number + 1,
                    component.output_name,
                    component.component_type,
                    component.size,
                    component.data_offset
                ));
            }
            let copy = list_text.clone();
            if ui.button("复制清单").clicked() {
                ui.ctx().copy_text(copy);
            }
            ui.label(
                egui::RichText::new("提示: 点击行可选中/取消, 勾选决定\"解包勾选组件\"的内容").weak(),
            );
        });
        ui.add_space(4.0);

        let components = &index.components;
        let mut checked = self.checked.clone();
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::auto().at_least(46.0))
            .column(Column::auto().at_least(180.0))
            .column(Column::auto().at_least(80.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::remainder().at_least(120.0))
            .header(24.0, |mut header| {
                header.col(|ui| {
                    ui.strong("解包");
                });
                header.col(|ui| {
                    ui.strong("名称");
                });
                header.col(|ui| {
                    ui.strong("类型");
                });
                header.col(|ui| {
                    ui.strong("大小");
                });
                header.col(|ui| {
                    ui.strong("偏移");
                });
            })
            .body(|mut body| {
                for position in 0..components.len() {
                    let component = &components[position];
                    body.row(24.0, |mut row| {
                        row.col(|ui| {
                            if ui.checkbox(&mut checked[position], "").changed() {
                                let _ = component;
                            }
                        });
                        row.col(|ui| {
                            let text = &component.output_name;
                            let response = ui.label(text);
                            if response.double_clicked() {
                                ui.ctx().copy_text(text.clone());
                            }
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

fn component_type_label(component_type: u8) -> String {
    match component_type {
        0 => "镜像".to_owned(),
        1 => "压缩包".to_owned(),
        other => format!("类型 {other}"),
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
        .unwrap_or_else(|| "update".to_owned());
    let name = format!("{stem}-out");
    if parent.is_empty() {
        name
    } else {
        format!("{parent}/{name}")
    }
}
