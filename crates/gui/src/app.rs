use crate::job::{self, Job, JobResult};
use crate::model::{LayoutChoice, Operation};
use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;

pub(crate) struct HaucetApp {
    operation: Operation,
    layout: LayoutChoice,
    input: String,
    secondary: String,
    output: String,
    tools_dir: String,
    partitions: String,
    force: bool,
    all_erofs: bool,
    allow_grow: bool,
    receiver: Option<Receiver<JobResult>>,
    status: String,
    last_report: String,
}

impl Default for HaucetApp {
    fn default() -> Self {
        Self {
            operation: Operation::FullUnpack,
            layout: LayoutChoice::Auto,
            input: String::new(),
            secondary: String::new(),
            output: String::new(),
            tools_dir: String::new(),
            partitions: String::new(),
            force: false,
            all_erofs: false,
            allow_grow: false,
            receiver: None,
            status: "Idle".to_owned(),
            last_report: String::new(),
        }
    }
}

impl eframe::App for HaucetApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.accept_dropped_files(ctx);
        self.poll_task();

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("haucet-tools");
                ui.separator();
                ui.label(&self.status);
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(8.0);
            egui::Grid::new("main-form")
                .num_columns(2)
                .spacing([18.0, 10.0])
                .show(ui, |ui| self.form(ui));

            ui.add_space(16.0);
            self.drop_zone(ui);
            ui.add_space(16.0);
            self.options(ui);
            ui.add_space(12.0);
            self.actions(ui);
            ui.add_space(14.0);
            self.report(ui);
        });

        if self.receiver.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

impl HaucetApp {
    fn form(&mut self, ui: &mut egui::Ui) {
        ui.label("Operation");
        egui::ComboBox::from_id_salt("operation")
            .selected_text(self.operation.label())
            .show_ui(ui, |ui| {
                for operation in Operation::ALL {
                    ui.selectable_value(&mut self.operation, operation, operation.label());
                }
            });
        ui.end_row();

        self.path_row(ui, self.operation.input_label(), PathTarget::Input);
        if let Some(label) = self.operation.secondary_label() {
            self.path_row(ui, label, PathTarget::Secondary);
        }
        if let Some(label) = self.operation.output_label() {
            self.path_row(ui, label, PathTarget::Output);
        }
        if self.operation.needs_erofs_tools() {
            self.path_row(ui, "Tools dir", PathTarget::Tools);
        }
        if self.operation.needs_layout() {
            ui.label("Layout");
            egui::ComboBox::from_id_salt("layout")
                .selected_text(self.layout.label())
                .show_ui(ui, |ui| {
                    for layout in LayoutChoice::ALL {
                        ui.selectable_value(&mut self.layout, layout, layout.label());
                    }
                });
            ui.end_row();
        }
        if matches!(self.operation, Operation::FullUnpack) {
            ui.label("Partitions");
            ui.text_edit_singleline(&mut self.partitions);
            ui.end_row();
        }
    }

    fn options(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.force, "Force");
            if matches!(self.operation, Operation::FullUnpack) {
                ui.checkbox(&mut self.all_erofs, "All EROFS");
            }
            if matches!(self.operation, Operation::ErofsRepack) {
                ui.checkbox(&mut self.allow_grow, "Allow grow");
            }
        });
    }

    fn actions(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.receiver.is_none(),
                    egui::Button::new("Run").min_size([120.0, 34.0].into()),
                )
                .clicked()
            {
                self.start_task();
            }
            if ui.button("Clear").clicked() && self.receiver.is_none() {
                self.last_report.clear();
                self.status = "Idle".to_owned();
            }
        });
    }

    fn report(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("report")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.last_report)
                        .desired_rows(12)
                        .font(egui::TextStyle::Monospace)
                        .interactive(false),
                );
            });
    }

    fn path_row(&mut self, ui: &mut egui::Ui, label: &str, target: PathTarget) {
        ui.label(label);
        ui.add_sized(
            [ui.available_width(), 24.0],
            egui::TextEdit::singleline(self.path_value_mut(target)),
        );
        ui.end_row();
    }

    fn drop_zone(&mut self, ui: &mut egui::Ui) {
        let (rect, response) =
            ui.allocate_exact_size([ui.available_width(), 112.0].into(), egui::Sense::hover());
        let fill = if response.hovered() {
            ui.visuals().selection.bg_fill.linear_multiply(0.45)
        } else {
            ui.visuals().extreme_bg_color
        };
        ui.painter().rect(
            rect,
            6.0,
            fill,
            egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Drop files or directories",
            egui::TextStyle::Button.resolve(ui.style()),
            ui.visuals().text_color(),
        );
    }

    fn accept_dropped_files(&mut self, ctx: &egui::Context) {
        for file in ctx.input(|input| input.raw.dropped_files.clone()) {
            if let Some(path) = file.path {
                self.assign_dropped_path(path);
            }
        }
    }

    fn assign_dropped_path(&mut self, path: PathBuf) {
        let display = path.display().to_string();
        if self.input.trim().is_empty() {
            self.input = display;
        } else if self.operation.secondary_label().is_some() && self.secondary.trim().is_empty() {
            self.secondary = display;
        } else if self.operation.output_label().is_some() && path.is_dir() {
            self.output = display;
        } else {
            self.input = display;
        }
    }

    fn path_value_mut(&mut self, target: PathTarget) -> &mut String {
        match target {
            PathTarget::Input => &mut self.input,
            PathTarget::Secondary => &mut self.secondary,
            PathTarget::Output => &mut self.output,
            PathTarget::Tools => &mut self.tools_dir,
        }
    }

    fn poll_task(&mut self) {
        let Some(receiver) = &self.receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(result) => {
                self.status = if result.success { "Done" } else { "Failed" }.to_owned();
                self.last_report = result.message;
                self.receiver = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.status = "Failed".to_owned();
                self.last_report = "Worker stopped before returning a result".to_owned();
                self.receiver = None;
            }
        }
    }

    fn start_task(&mut self) {
        let job = Job {
            operation: self.operation,
            layout: self.layout,
            input: self.input.clone(),
            secondary: self.secondary.clone(),
            output: self.output.clone(),
            tools_dir: self.tools_dir.clone(),
            partitions: self.partitions.clone(),
            force: self.force,
            all_erofs: self.all_erofs,
            allow_grow: self.allow_grow,
        };
        let (sender, receiver) = mpsc::channel();
        self.status = format!("Running {}", self.operation.label());
        self.last_report = "Working...".to_owned();
        self.receiver = Some(receiver);
        thread::spawn(move || {
            let result = job::run(job);
            let message = result
                .as_ref()
                .map_or_else(|error| format!("{error:#}"), Clone::clone);
            let _ = sender.send(JobResult {
                success: result.is_ok(),
                message,
            });
        });
    }
}

#[derive(Debug, Clone, Copy)]
enum PathTarget {
    Input,
    Secondary,
    Output,
    Tools,
}
