use crate::job::{self, JobEvent, JobResult, RunningJob};
use crate::pages::{self, Page};
use crate::settings::Settings;
use crate::worker::JobOp;
use eframe::egui;
use std::path::PathBuf;

pub(crate) struct HaucetApp {
    pub current: Page,
    pub home: pages::home::HomePage,
    pub package: pages::package::PackagePage,
    pub update_bin: pages::update_bin::UpdateBinPage,
    pub erofs: pages::erofs::ErofsPage,
    pub ramdisk: pages::ramdisk::RamdiskPage,
    pub partition: pages::partition::PartitionPage,
    pub fastboot: pages::fastboot::FastbootPage,
    pub entropy: pages::entropy::EntropyPage,
    pub cpio: pages::cpio::CpioPage,

    pub job: Option<RunningJob>,
    pub job_owner: Page,
    pub logs: Vec<String>,
    pub settings: Settings,
    pub font_loaded: bool,
    pub logo: Option<egui::TextureHandle>,
    pub last_result: Option<(Page, JobResult)>,
}

impl HaucetApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        font_loaded: bool,
        logo_rgba: Option<(Vec<u8>, [usize; 2])>,
    ) -> Self {
        let logo = logo_rgba.and_then(|(rgba, [width, height])| {
            if rgba.len() != width * height * 4 || width == 0 || height == 0 {
                return None;
            }
            let image = egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba);
            Some(
                cc.egui_ctx
                    .load_texture("haucet-logo", image, egui::TextureOptions::LINEAR),
            )
        });
        Self {
            current: Page::Home,
            home: pages::home::HomePage::default(),
            package: pages::package::PackagePage::default(),
            update_bin: pages::update_bin::UpdateBinPage::default(),
            erofs: pages::erofs::ErofsPage::default(),
            ramdisk: pages::ramdisk::RamdiskPage::default(),
            partition: pages::partition::PartitionPage::default(),
            fastboot: pages::fastboot::FastbootPage::default(),
            entropy: pages::entropy::EntropyPage::default(),
            cpio: pages::cpio::CpioPage::default(),
            job: None,
            job_owner: Page::Home,
            logs: Vec::new(),
            settings: Settings::load(),
            font_loaded,
            logo,
            last_result: None,
        }
    }
}

impl eframe::App for HaucetApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_job();
        draw_window_background(ctx);

        egui::TopBottomPanel::top("title-bar")
            .exact_height(38.0)
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| self.title_bar(ui));
        egui::SidePanel::left("nav")
            .resizable(false)
            .exact_width(200.0)
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| self.nav_panel(ui));
        egui::TopBottomPanel::bottom("log-panel")
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| self.log_panel(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 0)))
            .show(ctx, |ui| self.central(ui));

        if !self.font_loaded {
            egui::Area::new("font-warning".into())
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 46.0))
                .show(ctx, |ui| {
                    egui::Frame::group(ui.style())
                        .fill(egui::Color32::from_rgb(90, 60, 10).gamma_multiply(0.9))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(
                                    "未找到中文字体, 界面文字可能无法显示。\n请安装 Noto Sans CJK / 微软雅黑 等字体后重启。",
                                )
                                .color(egui::Color32::from_rgb(255, 220, 160)),
                            );
                        });
                });
        }

        if self.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.settings.save();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Color32::TRANSPARENT.to_normalized_gamma_f32()
    }
}

impl HaucetApp {
    fn poll_job(&mut self) {
        let mut events = Vec::new();
        if let Some(job) = &mut self.job {
            while let Some(event) = job.poll() {
                events.push(event);
            }
        }
        let mut finished = false;
        for event in events {
            match event {
                JobEvent::Log(line) => self.push_log(line),
                JobEvent::Done(result) => {
                    let owner = self.job_owner;
                    let mark = if result.cancelled {
                        "[已取消]"
                    } else if result.ok {
                        "[成功]"
                    } else {
                        "[失败]"
                    };
                    self.push_log(format!("{mark} {}", result.summary));
                    self.last_result = Some((owner, result));
                    finished = true;
                }
            }
        }
        if finished {
            self.job = None;
            self.settings.save();
        }
    }

    pub fn job_running(&self) -> bool {
        self.job.is_some()
    }

    pub fn start_job(&mut self, op: JobOp) -> bool {
        if self.job.is_some() {
            return false;
        }
        match job::start(op) {
            Ok(running) => {
                let label = job_label(&running.op);
                self.job_owner = self.current;
                self.push_log(format!("── 开始任务：{label}"));
                self.job = Some(running);
                self.last_result = None;
                true
            }
            Err(error) => {
                self.push_log(format!("[错误] 无法启动任务：{error:#}"));
                self.last_result = Some((
                    self.current,
                    JobResult {
                        ok: false,
                        cancelled: false,
                        summary: format!("无法启动任务：{error:#}"),
                        payload: None,
                    },
                ));
                false
            }
        }
    }

    pub fn cancel_job(&mut self) {
        if let Some(job) = &mut self.job {
            job.cancel();
        }
    }

    pub fn take_result(&mut self, page: Page) -> Option<JobResult> {
        match &self.last_result {
            Some((owner, _)) if *owner == page => self.last_result.take().map(|(_, r)| r),
            _ => None,
        }
    }

    pub fn nav(&mut self, page: Page) {
        if self.current != page {
            self.current = page;
        }
    }

    pub fn push_log(&mut self, line: String) {
        self.logs.push(line);
        if self.logs.len() > 2000 {
            let overflow = self.logs.len() - 2000;
            self.logs.drain(..overflow);
        }
    }

    pub fn pick_file(&mut self, title: &str, filters: &[(&str, &[&str])]) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title(title);
        if let Some(dir) = &self.settings.last_dir {
            dialog = dialog.set_directory(dir);
        }
        for (name, extensions) in filters {
            dialog = dialog.add_filter(*name, extensions);
        }
        let picked = dialog.pick_file();
        if let Some(path) = &picked {
            self.settings.remember_path(path);
        }
        picked
    }

    pub fn pick_dir(&mut self, title: &str) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title(title);
        if let Some(dir) = &self.settings.last_dir {
            dialog = dialog.set_directory(dir);
        }
        let picked = dialog.pick_folder();
        if let Some(path) = &picked {
            self.settings.remember_path(path);
        }
        picked
    }

    pub fn pick_save(&mut self, title: &str, file_name: &str) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new()
            .set_title(title)
            .set_file_name(file_name);
        if let Some(dir) = &self.settings.last_dir {
            dialog = dialog.set_directory(dir);
        }
        let picked = dialog.save_file();
        if let Some(path) = &picked {
            self.settings.remember_path(path);
        }
        picked
    }

    /// Drained list of files dropped onto the window in this frame.
    pub fn take_drops(&mut self, ctx: &egui::Context) -> Vec<PathBuf> {
        ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect::<Vec<_>>()
        })
    }

    fn title_bar(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let bottom = rect.left_bottom() + egui::vec2(rect.width(), 0.0);
        ui.painter().line_segment(
            [rect.left_bottom(), bottom],
            ui.visuals().widgets.noninteractive.bg_stroke,
        );

        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_space(12.0);

            let title_response = ui.add(
                egui::Label::new(egui::RichText::new("Haucet Tools").strong().size(17.0))
                    .sense(egui::Sense::click_and_drag()),
            );
            handle_title_bar_response(ui, &title_response);

            let status_width = if self.job.is_some() { 260.0 } else { 64.0 };
            let controls_width = 118.0;
            let spacer_width =
                (ui.available_width() - status_width - controls_width - 14.0).max(0.0);
            let spacer_response = ui.allocate_response(
                egui::vec2(spacer_width, 32.0),
                egui::Sense::click_and_drag(),
            );
            handle_title_bar_response(ui, &spacer_response);

            ui.allocate_ui_with_layout(
                egui::vec2(status_width, 32.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| self.title_status(ui),
            );
            ui.separator();
            window_controls(ui);
        });
    }

    fn title_status(&mut self, ui: &mut egui::Ui) {
        if let Some(job) = &mut self.job {
            ui.add(egui::Spinner::new().size(16.0));
            ui.label(egui::RichText::new(format!("运行中 {}s", job.elapsed().as_secs())).strong());
            if ui.button("取消").clicked() {
                self.cancel_job();
            }
        } else if let Some((_, result)) = &self.last_result {
            if result.cancelled {
                pages::badge_text(ui, "上次任务已取消", egui::Color32::from_rgb(230, 170, 40));
            } else if result.ok {
                pages::badge_text(ui, "上次任务成功", egui::Color32::from_rgb(90, 200, 120));
            } else {
                pages::badge_text(ui, "上次任务失败", egui::Color32::from_rgb(230, 90, 90));
            }
        } else {
            ui.label(egui::RichText::new("空闲").weak());
        }
    }

    fn nav_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        for page in Page::ALL {
            let selected = self.current == page;
            let response = ui.add_sized(
                [ui.available_width(), 34.0],
                egui::Button::selectable(selected, egui::RichText::new(page.title()).size(15.0)),
            );
            if response.clicked() {
                self.nav(page);
            }
        }
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("关于").weak());
                ui.label(
                    egui::RichText::new("Huawei/HarmonyOS 镜像工具")
                        .weak()
                        .size(11.0),
                );
            });
        });
    }

    fn log_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.settings.show_log, "运行日志");
            ui.separator();
            if ui.button("清空").clicked() {
                self.logs.clear();
            }
            if ui.button("复制").clicked() {
                let text = self.logs.join("\n");
                ui.ctx().copy_text(text);
            }
            if let Some(job) = &self.job {
                ui.label(egui::RichText::new(job_label(&job.op)).weak());
            }
        });
        if self.settings.show_log && !self.logs.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("job-log")
                .max_height(170.0)
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &self.logs {
                        ui.label(egui::RichText::new(line).monospace().size(12.0));
                    }
                });
        }
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        let current = self.current;
        ui.scope(|ui| {
            apply_content_text_style(ui);
            match current {
                Page::Home => {
                    let mut page = std::mem::take(&mut self.home);
                    page.ui(ui, self);
                    self.home = page;
                }
                Page::Package => {
                    let mut page = std::mem::take(&mut self.package);
                    page.ui(ui, self);
                    self.package = page;
                }
                Page::UpdateBin => {
                    let mut page = std::mem::take(&mut self.update_bin);
                    page.ui(ui, self);
                    self.update_bin = page;
                }
                Page::Erofs => {
                    let mut page = std::mem::take(&mut self.erofs);
                    page.ui(ui, self);
                    self.erofs = page;
                }
                Page::Ramdisk => {
                    let mut page = std::mem::take(&mut self.ramdisk);
                    page.ui(ui, self);
                    self.ramdisk = page;
                }
                Page::Partition => {
                    let mut page = std::mem::take(&mut self.partition);
                    page.ui(ui, self);
                    self.partition = page;
                }
                Page::Fastboot => {
                    let mut page = std::mem::take(&mut self.fastboot);
                    page.ui(ui, self);
                    self.fastboot = page;
                }
                Page::Entropy => {
                    let mut page = std::mem::take(&mut self.entropy);
                    page.ui(ui, self);
                    self.entropy = page;
                }
                Page::Cpio => {
                    let mut page = std::mem::take(&mut self.cpio);
                    page.ui(ui, self);
                    self.cpio = page;
                }
            }
        });
    }
}

fn apply_content_text_style(ui: &mut egui::Ui) {
    let text_styles = &mut ui.style_mut().text_styles;
    text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(15.5));
    text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(15.5));
    text_styles.insert(egui::TextStyle::Monospace, egui::FontId::monospace(14.5));
    text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(13.0));
}

impl Default for HaucetApp {
    fn default() -> Self {
        // Only used as a fallback for mem::take; real construction goes
        // through new().
        unreachable!("HaucetApp is constructed through new()")
    }
}

fn draw_window_background(ctx: &egui::Context) {
    let rect = ctx.screen_rect();
    let radius = if is_maximized(ctx) {
        egui::CornerRadius::ZERO
    } else {
        egui::CornerRadius::same(10)
    };
    let painter = ctx.layer_painter(egui::LayerId::background());
    painter.rect_filled(rect, radius, egui::Color32::from_rgb(24, 24, 24));
    painter.rect_stroke(
        rect.shrink(0.5),
        radius,
        egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(58, 58, 58)),
        egui::StrokeKind::Inside,
    );
    if !is_maximized(ctx) {
        painter.rect_stroke(
            rect.shrink(1.5),
            radius,
            egui::Stroke::new(
                1.0_f32,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 18),
            ),
            egui::StrokeKind::Inside,
        );
    }
}

fn handle_title_bar_response(ui: &egui::Ui, response: &egui::Response) {
    if response.double_clicked() {
        toggle_maximized(ui.ctx());
    } else if response.is_pointer_button_down_on()
        && ui.input(|input| input.pointer.primary_pressed())
    {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}

fn window_controls(ui: &mut egui::Ui) {
    if window_button(ui, "-", "最小化").clicked() {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }

    let maximized = is_maximized(ui.ctx());
    let max_label = if maximized { "❐" } else { "□" };
    let max_tooltip = if maximized { "还原" } else { "最大化" };
    if window_button(ui, max_label, max_tooltip).clicked() {
        toggle_maximized(ui.ctx());
    }

    if window_button(ui, "×", "关闭").clicked() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

fn window_button(ui: &mut egui::Ui, label: &str, tooltip: &str) -> egui::Response {
    ui.add_sized(
        [34.0, 28.0],
        egui::Button::new(egui::RichText::new(label).size(16.0)),
    )
    .on_hover_text(tooltip)
}

fn toggle_maximized(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized(ctx)));
}

fn is_maximized(ctx: &egui::Context) -> bool {
    ctx.input(|input| input.viewport().maximized.unwrap_or(false))
}

fn job_label(op: &JobOp) -> &'static str {
    use crate::worker::JobOp::*;
    match op {
        PackageInspect { .. } => "读取更新包内容",
        PackageUnpack { .. } => "解包更新包",
        UpdateList { .. } => "读取 update.bin 索引",
        UpdateUnpack { .. } => "解包 update.bin",
        ErofsUnpack { .. } => "解包 EROFS 镜像",
        ErofsRepack { .. } => "重新打包 EROFS 镜像",
        RamdiskUnpack { .. } => "解包 ramdisk",
        RamdiskRepack { .. } => "重新打包 ramdisk",
        RamdiskPatch { .. } => "给 ramdisk 打补丁",
        RamdiskProbe { .. } => "检查 ramdisk 镜像",
        PartitionInfo { .. } => "读取分区信息",
        FileEntropy { .. } => "计算文件信息熵",
        FastbootStatus { .. } => "检测 fastboot 设备",
        FastbootFlash { .. } => "刷写 fastboot 镜像",
    }
}
