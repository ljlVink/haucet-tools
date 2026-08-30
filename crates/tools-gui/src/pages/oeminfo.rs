use crate::app::HaucetApp;
use crate::pages::Page;
use crate::util::{human_size, message_box, open_in_file_manager, section};
use anyhow::{Context, Result, ensure};
use common::oeminfo::{OemInfoBlockSummary, OemInfoImageSummary, OemInfoPayloadKind};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use flate2::read::MultiGzDecoder;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

const MAX_PREVIEW_SOURCE_SIZE: u64 = 32 * 1024 * 1024;
const MAX_PREVIEW_DECOMPRESSED_SIZE: u64 = 64 * 1024 * 1024;
const MAX_PREVIEW_IMAGE_SIDE: u32 = 8192;
const MAX_PREVIEW_DECODE_ALLOCATION: u64 = 64 * 1024 * 1024;
const MAX_PREVIEW_TEXTURE_SIDE: u32 = 2048;
const MAX_PREVIEW_DISPLAY_WIDTH: f32 = 900.0;
const MAX_PREVIEW_DISPLAY_HEIGHT: f32 = 520.0;

pub struct OemInfoPage {
    input: String,
    filter: String,
    active_only: bool,
    summary: Option<OemInfoImageSummary>,
    error: Option<String>,
    selected_block: Option<usize>,
    inspect_requested: bool,
    inspect_generation: u64,
    operation: Option<OemInfoOperation>,
    export_result: Option<ExportResult>,
    preview_generation: u64,
    preview_worker: Option<PreviewWorker>,
    preview_request: Option<PendingPreview>,
    preview_texture: Option<PreviewTexture>,
    preview_error: Option<PreviewError>,
}

impl Default for OemInfoPage {
    fn default() -> Self {
        Self {
            input: String::new(),
            filter: String::new(),
            active_only: true,
            summary: None,
            error: None,
            selected_block: None,
            inspect_requested: false,
            inspect_generation: 0,
            operation: None,
            export_result: None,
            preview_generation: 0,
            preview_worker: None,
            preview_request: None,
            preview_texture: None,
            preview_error: None,
        }
    }
}

#[derive(Debug)]
enum OemInfoOperation {
    Inspect { input: String, generation: u64 },
    Export { output: String },
}

#[derive(Debug)]
struct ExportResult {
    ok: bool,
    summary: String,
    output: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreviewKey {
    generation: u64,
    offset: u64,
    id: u32,
    sub_id: u32,
    age: u32,
}

#[derive(Debug)]
struct PreviewWorker {
    request_sender: Sender<PreviewRequest>,
    result_receiver: Receiver<PreviewMessage>,
}

#[derive(Debug)]
struct PreviewRequest {
    key: PreviewKey,
    input: String,
    block: OemInfoBlockSummary,
    cancelled: Arc<AtomicBool>,
}

#[derive(Debug)]
struct PendingPreview {
    key: PreviewKey,
    cancelled: Arc<AtomicBool>,
}

#[derive(Debug)]
struct PreviewMessage {
    key: PreviewKey,
    result: std::result::Result<DecodedPreview, String>,
}

#[derive(Debug)]
struct DecodedPreview {
    rgba: Vec<u8>,
    size: [usize; 2],
    original_size: [u32; 2],
}

struct PreviewTexture {
    key: PreviewKey,
    texture: egui::TextureHandle,
    original_size: [u32; 2],
    preview_size: [usize; 2],
}

#[derive(Debug)]
struct PreviewError {
    key: PreviewKey,
    message: String,
}

impl OemInfoPage {
    pub fn select_input(&mut self, input: String) {
        self.input = input;
        self.summary = None;
        self.error = None;
        self.export_result = None;
        self.selected_block = None;
        self.clear_preview();
        self.request_inspection(!self.input.trim().is_empty());
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        self.poll_result(app);
        self.poll_preview(ui.ctx());

        egui::ScrollArea::vertical()
            .id_salt("oeminfo-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.render_input(ui, app);

                if let Some(path) = app.take_drops(ui.ctx()).first().cloned() {
                    app.settings.remember_path(&path);
                    self.select_input(path.display().to_string());
                }
                self.start_inspection(app);

                if let Some(error) = &self.error {
                    ui.add_space(10.0);
                    message_box(ui, egui::Color32::from_rgb(230, 90, 90), error);
                }

                self.render_export_result(ui);

                if self.summary.is_some() {
                    ui.add_space(12.0);
                    self.render_summary(ui, app);
                }
                ui.add_space(20.0);
            });
    }

    fn render_input(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("镜像文件").strong());
            let field_width = (ui.available_width() - 190.0).max(120.0);
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.input)
                    .hint_text("选择 OEMINFO 镜像或拖放文件")
                    .font(egui::TextStyle::Monospace)
                    .desired_width(field_width),
            );
            let submit =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if response.changed() {
                self.summary = None;
                self.error = None;
                self.export_result = None;
                self.selected_block = None;
                self.clear_preview();
                self.request_inspection(std::path::Path::new(self.input.trim()).is_file());
            }
            if submit {
                self.request_inspection(!self.input.trim().is_empty());
            }
            if ui.button("选择文件").clicked()
                && let Some(path) = app.pick_file("选择 OEMINFO 镜像", &[])
            {
                self.select_input(path.display().to_string());
            }
        });
    }

    fn render_summary(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        {
            let Some(summary) = self.summary.as_ref() else {
                return;
            };

            section(ui, "镜像概览");
            egui::Grid::new("oeminfo-summary-grid")
                .num_columns(4)
                .spacing([18.0, 7.0])
                .show(ui, |ui| {
                    summary_value(ui, "文件大小", human_size(summary.file_size));
                    summary_value(ui, "区域大小", human_size(summary.region_size));
                    ui.end_row();
                    summary_value(
                        ui,
                        "数据块",
                        format!("{}（活动 {}）", summary.total_blocks, summary.active_blocks),
                    );
                    summary_value(
                        ui,
                        "候选头",
                        format!(
                            "{}（丢弃 {}）",
                            summary.candidate_headers, summary.discarded_headers
                        ),
                    );
                    ui.end_row();
                    summary_value(
                        ui,
                        "区域分布",
                        format!(
                            "A {} / B {} / 其他 {}",
                            summary.region_a_blocks,
                            summary.region_b_blocks,
                            summary.unknown_region_blocks
                        ),
                    );
                    summary_value(
                        ui,
                        "布局分布",
                        format!(
                            "标准 {} / 紧凑 {} / 复用 {}",
                            summary.standard_blocks, summary.compact_blocks, summary.reused_blocks
                        ),
                    );
                    ui.end_row();
                });

            if summary.discarded_headers != 0 {
                ui.add_space(8.0);
                message_box(
                    ui,
                    egui::Color32::from_rgb(225, 155, 60),
                    format!(
                        "扫描时丢弃了 {} 个重叠或无效的候选头。",
                        summary.discarded_headers
                    ),
                );
            }
        }

        ui.add_space(12.0);
        self.render_filter(ui);

        let summary = self
            .summary
            .as_ref()
            .expect("summary exists for OEMINFO result rendering");
        let filter = self.filter.trim().to_ascii_lowercase();
        let visible = summary
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| !self.active_only || block.active)
            .filter(|(_, block)| block_matches(block, &filter))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        ui.label(
            egui::RichText::new(format!(
                "显示 {} / {} 个数据块",
                visible.len(),
                summary.blocks.len()
            ))
            .small()
            .weak(),
        );
        ui.add_space(4.0);

        let mut selected = None;
        render_blocks_table(ui, summary, &visible, self.selected_block, &mut selected);
        let selection_changed = selected.is_some_and(|index| self.selected_block != Some(index));
        if let Some(index) = selected {
            self.selected_block = Some(index);
        }
        if selection_changed {
            self.clear_preview();
        }

        let selected_block = self
            .selected_block
            .and_then(|index| self.summary.as_ref()?.blocks.get(index))
            .cloned();
        if let Some(block) = selected_block {
            ui.add_space(12.0);
            render_block_details(ui, &block);
            if is_image_block(&block) {
                ui.add_space(12.0);
                self.render_image_preview(ui, app, &block);
            }
        }
    }

    fn render_filter(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("筛选").strong());
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("ID、区域、布局、类型或预览内容")
                    .desired_width((ui.available_width() - 170.0).max(150.0)),
            );
            ui.checkbox(&mut self.active_only, "仅活动副本");
            if !self.filter.is_empty() && ui.button("清除").clicked() {
                self.filter.clear();
            }
        });
    }

    fn poll_result(&mut self, app: &mut HaucetApp) {
        let Some(result) = app.take_result(Page::OemInfo) else {
            return;
        };
        let Some(operation) = self.operation.take() else {
            return;
        };
        match operation {
            OemInfoOperation::Inspect { input, generation } => {
                if input != self.input.trim() || generation != self.inspect_generation {
                    return;
                }
                self.finish_inspection(result);
            }
            OemInfoOperation::Export { output } => {
                self.export_result = Some(ExportResult {
                    ok: result.ok,
                    summary: result.summary,
                    output,
                });
            }
        }
    }

    fn finish_inspection(&mut self, result: crate::job::JobResult) {
        if !result.ok {
            self.summary = None;
            self.selected_block = None;
            self.clear_preview();
            self.error = Some(result.summary);
            return;
        }

        let Some(payload) = result.payload else {
            self.summary = None;
            self.selected_block = None;
            self.clear_preview();
            self.error = Some("后台任务未返回 OEMINFO 摘要".to_owned());
            return;
        };
        match serde_json::from_value::<OemInfoImageSummary>(payload) {
            Ok(summary) => {
                self.selected_block = summary
                    .blocks
                    .iter()
                    .position(|block| block.active)
                    .or_else(|| (!summary.blocks.is_empty()).then_some(0));
                self.clear_preview();
                self.summary = Some(summary);
                self.error = None;
            }
            Err(error) => {
                self.summary = None;
                self.selected_block = None;
                self.clear_preview();
                self.error = Some(format!("无法解析 OEMINFO 读取结果: {error}"));
            }
        }
    }

    fn start_inspection(&mut self, app: &mut HaucetApp) {
        if !self.inspect_requested || app.job_running() {
            return;
        }
        let image = self.input.trim().to_owned();
        self.inspect_requested = false;
        if image.is_empty() {
            return;
        }
        self.operation = Some(OemInfoOperation::Inspect {
            input: image.clone(),
            generation: self.inspect_generation,
        });
        app.start_job(crate::worker::JobOp::OemInfoInspect { image });
    }

    fn request_inspection(&mut self, requested: bool) {
        self.inspect_generation = self.inspect_generation.wrapping_add(1);
        self.inspect_requested = requested;
    }

    fn render_export_result(&self, ui: &mut egui::Ui) {
        let Some(result) = &self.export_result else {
            return;
        };
        ui.add_space(10.0);
        let color = if result.ok {
            egui::Color32::from_rgb(90, 200, 120)
        } else {
            egui::Color32::from_rgb(230, 90, 90)
        };
        message_box(ui, color, &result.summary);
        if result.ok && ui.button("打开导出位置").clicked() {
            open_in_file_manager(Path::new(&result.output));
        }
    }

    fn render_image_preview(
        &mut self,
        ui: &mut egui::Ui,
        app: &mut HaucetApp,
        block: &OemInfoBlockSummary,
    ) {
        section(ui, "图片预览");
        let key = self.preview_key(block);
        self.ensure_preview(ui.ctx(), key.clone(), block.clone());

        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(match block.payload_kind {
                    OemInfoPayloadKind::ImageRaw => "BMP",
                    OemInfoPayloadKind::ImageGzip => "GZIP / BMP",
                    _ => "图片",
                })
                .strong(),
            );
            ui.label(human_size(block.length.saturating_sub(0x1a) as u64));

            let (extension, button_label) = match block.payload_kind {
                OemInfoPayloadKind::ImageRaw => ("bmp", "导出原始 BMP"),
                OemInfoPayloadKind::ImageGzip => ("gz", "导出原始 GZIP"),
                _ => return,
            };
            let can_export = !app.job_running() && self.operation.is_none();
            if ui
                .add_enabled(can_export, egui::Button::new(button_label))
                .clicked()
            {
                let suggested = format!(
                    "oeminfo_{}_{}_0x{:X}.{extension}",
                    block.id, block.sub_id, block.offset
                );
                if let Some(output) = app.pick_save("导出 OEMINFO 图片", &suggested) {
                    self.export_result = None;
                    let output = output.display().to_string();
                    self.operation = Some(OemInfoOperation::Export {
                        output: output.clone(),
                    });
                    app.start_job(crate::worker::JobOp::OemInfoExportImage {
                        image: self.input.trim().to_owned(),
                        block: block.clone(),
                        output,
                    });
                }
            }
        });
        ui.add_space(8.0);

        if let Some(preview) = self
            .preview_texture
            .as_ref()
            .filter(|preview| preview.key == key)
        {
            ui.label(
                egui::RichText::new(format!(
                    "{} x {} 像素",
                    preview.original_size[0], preview.original_size[1]
                ))
                .small()
                .weak(),
            );
            let display_size = preview_display_size(preview.preview_size, ui.available_width());
            ui.add(egui::Image::new(&preview.texture).fit_to_exact_size(display_size))
                .on_hover_text(format!(
                    "预览纹理 {} x {}",
                    preview.preview_size[0], preview.preview_size[1]
                ));
        } else if let Some(message) = self
            .preview_error
            .as_ref()
            .filter(|error| error.key == key)
            .map(|error| error.message.clone())
        {
            message_box(
                ui,
                egui::Color32::from_rgb(230, 90, 90),
                format!("无法生成图片预览: {message}"),
            );
            if ui.button("重新加载预览").clicked() {
                self.preview_error = None;
            }
        } else {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(16.0));
                ui.label(egui::RichText::new("正在生成预览").weak());
            });
        }
    }

    fn preview_key(&self, block: &OemInfoBlockSummary) -> PreviewKey {
        PreviewKey {
            generation: self.preview_generation,
            offset: block.offset,
            id: block.id,
            sub_id: block.sub_id,
            age: block.age,
        }
    }

    fn ensure_preview(&mut self, ctx: &egui::Context, key: PreviewKey, block: OemInfoBlockSummary) {
        if self
            .preview_texture
            .as_ref()
            .is_some_and(|preview| preview.key == key)
            || self
                .preview_error
                .as_ref()
                .is_some_and(|error| error.key == key)
            || self
                .preview_request
                .as_ref()
                .is_some_and(|request| request.key == key)
        {
            return;
        }

        if let Some(request) = self.preview_request.take() {
            request.cancelled.store(true, Ordering::Relaxed);
        }
        if self.preview_worker.is_none() {
            self.preview_worker = Some(spawn_preview_worker(ctx));
        }

        let cancelled = Arc::new(AtomicBool::new(false));
        let request = PreviewRequest {
            key: key.clone(),
            input: self.input.trim().to_owned(),
            block,
            cancelled: Arc::clone(&cancelled),
        };
        let sent = self
            .preview_worker
            .as_ref()
            .expect("preview worker was initialized")
            .request_sender
            .send(request)
            .is_ok();
        if sent {
            self.preview_request = Some(PendingPreview { key, cancelled });
        } else {
            self.preview_worker = None;
            self.preview_error = Some(PreviewError {
                key,
                message: "image preview worker exited unexpectedly".to_owned(),
            });
        }
    }

    fn poll_preview(&mut self, ctx: &egui::Context) {
        loop {
            let event = match self.preview_worker.as_ref() {
                Some(worker) => worker.result_receiver.try_recv(),
                None => return,
            };
            let message = match event {
                Ok(message) => message,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.preview_worker = None;
                    if let Some(request) = self.preview_request.take()
                        && self.current_preview_key().as_ref() == Some(&request.key)
                    {
                        self.preview_error = Some(PreviewError {
                            key: request.key,
                            message: "image preview worker exited unexpectedly".to_owned(),
                        });
                    }
                    return;
                }
            };
            if self
                .preview_request
                .as_ref()
                .is_some_and(|request| request.key == message.key)
            {
                self.preview_request = None;
            }
            if self.current_preview_key().as_ref() != Some(&message.key) {
                continue;
            }
            match message.result {
                Ok(decoded) => {
                    let image =
                        egui::ColorImage::from_rgba_unmultiplied(decoded.size, &decoded.rgba);
                    let texture = ctx.load_texture(
                        format!(
                            "oeminfo-image-{}-{}-{}",
                            message.key.generation, message.key.offset, message.key.age
                        ),
                        image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.preview_texture = Some(PreviewTexture {
                        key: message.key,
                        texture,
                        original_size: decoded.original_size,
                        preview_size: decoded.size,
                    });
                    self.preview_error = None;
                }
                Err(message_text) => {
                    self.preview_texture = None;
                    self.preview_error = Some(PreviewError {
                        key: message.key,
                        message: message_text,
                    });
                }
            }
        }
    }

    fn clear_preview(&mut self) {
        self.preview_generation = self.preview_generation.wrapping_add(1);
        if let Some(request) = self.preview_request.take() {
            request.cancelled.store(true, Ordering::Relaxed);
        }
        self.preview_texture = None;
        self.preview_error = None;
    }

    fn current_preview_key(&self) -> Option<PreviewKey> {
        let block = self
            .selected_block
            .and_then(|index| self.summary.as_ref()?.blocks.get(index))?;
        is_image_block(block).then(|| self.preview_key(block))
    }
}

fn is_image_block(block: &OemInfoBlockSummary) -> bool {
    matches!(
        block.payload_kind,
        OemInfoPayloadKind::ImageRaw | OemInfoPayloadKind::ImageGzip
    )
}

fn preview_display_size(texture_size: [usize; 2], available_width: f32) -> egui::Vec2 {
    let source = egui::vec2(texture_size[0] as f32, texture_size[1] as f32);
    if source.x <= 0.0 || source.y <= 0.0 {
        return egui::Vec2::ZERO;
    }
    let bounds = egui::vec2(
        available_width.clamp(1.0, MAX_PREVIEW_DISPLAY_WIDTH),
        MAX_PREVIEW_DISPLAY_HEIGHT,
    );
    let scale = (bounds.x / source.x).min(bounds.y / source.y).min(1.0);
    source * scale
}

fn spawn_preview_worker(ctx: &egui::Context) -> PreviewWorker {
    let (request_sender, request_receiver) = mpsc::channel::<PreviewRequest>();
    let (result_sender, result_receiver) = mpsc::channel();
    let repaint = ctx.clone();
    std::thread::spawn(move || {
        while let Ok(mut request) = request_receiver.recv() {
            for newer_request in request_receiver.try_iter() {
                request.cancelled.store(true, Ordering::Relaxed);
                request = newer_request;
            }
            let result = decode_image_preview(
                Path::new(&request.input),
                &request.block,
                &request.cancelled,
            )
            .map_err(|error| format!("{error:#}"));
            if result_sender
                .send(PreviewMessage {
                    key: request.key,
                    result,
                })
                .is_err()
            {
                break;
            }
            repaint.request_repaint();
        }
    });
    PreviewWorker {
        request_sender,
        result_receiver,
    }
}

fn decode_image_preview(
    path: &Path,
    block: &OemInfoBlockSummary,
    cancelled: &AtomicBool,
) -> Result<DecodedPreview> {
    ensure_preview_active(cancelled)?;
    let embedded =
        common::oeminfo::read_embedded_image_with_limit(path, block, MAX_PREVIEW_SOURCE_SIZE)
            .with_context(|| format!("读取 ID {} / SubID {} 图片", block.id, block.sub_id))?;
    ensure_preview_active(cancelled)?;
    decode_image_data(embedded.kind, embedded.data, cancelled)
}

fn decode_image_data(
    kind: OemInfoPayloadKind,
    data: Vec<u8>,
    cancelled: &AtomicBool,
) -> Result<DecodedPreview> {
    let bmp = match kind {
        OemInfoPayloadKind::ImageRaw => data,
        OemInfoPayloadKind::ImageGzip => {
            decompress_gzip_limited(data, MAX_PREVIEW_DECOMPRESSED_SIZE, cancelled)?
        }
        kind => anyhow::bail!("载荷类型 {kind} 不是可预览图片"),
    };
    ensure_preview_active(cancelled)?;
    ensure!(bmp.starts_with(b"BM"), "图片数据不是 BMP 格式");

    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_PREVIEW_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_PREVIEW_IMAGE_SIDE);
    limits.max_alloc = Some(MAX_PREVIEW_DECODE_ALLOCATION);
    let mut reader = image::ImageReader::with_format(Cursor::new(bmp), image::ImageFormat::Bmp);
    reader.limits(limits);
    let decoded = reader.decode().context("解码 OEMINFO BMP 图片")?;
    ensure_preview_active(cancelled)?;
    let original_size = [decoded.width(), decoded.height()];
    ensure!(
        original_size[0] != 0 && original_size[1] != 0,
        "BMP 图片尺寸为空"
    );
    let preview = if original_size[0] > MAX_PREVIEW_TEXTURE_SIDE
        || original_size[1] > MAX_PREVIEW_TEXTURE_SIDE
    {
        decoded.thumbnail(MAX_PREVIEW_TEXTURE_SIDE, MAX_PREVIEW_TEXTURE_SIDE)
    } else {
        decoded
    };
    let rgba = preview.into_rgba8();
    let (width, height) = rgba.dimensions();
    let size = [width as usize, height as usize];
    let rgba = rgba.into_raw();
    ensure!(
        size[0]
            .checked_mul(size[1])
            .and_then(|pixels| pixels.checked_mul(4))
            == Some(rgba.len()),
        "BMP 解码结果尺寸无效"
    );
    Ok(DecodedPreview {
        rgba,
        size,
        original_size,
    })
}

fn decompress_gzip_limited(data: Vec<u8>, limit: u64, cancelled: &AtomicBool) -> Result<Vec<u8>> {
    let decoder = MultiGzDecoder::new(Cursor::new(data));
    let mut limited = decoder.take(limit.saturating_add(1));
    let mut decoded = Vec::new();
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        ensure_preview_active(cancelled)?;
        let count = limited.read(&mut chunk).context("解压 OEMINFO GZIP 图片")?;
        if count == 0 {
            break;
        }
        decoded.extend_from_slice(&chunk[..count]);
    }
    ensure!(
        decoded.len() as u64 <= limit,
        "解压后图片超过 {}",
        human_size(limit)
    );
    Ok(decoded)
}

fn ensure_preview_active(cancelled: &AtomicBool) -> Result<()> {
    ensure!(
        !cancelled.load(Ordering::Relaxed),
        "image preview request was cancelled"
    );
    Ok(())
}

fn summary_value(ui: &mut egui::Ui, label: &str, value: impl Into<egui::WidgetText>) {
    ui.label(egui::RichText::new(label).weak());
    ui.label(value);
}

fn block_matches(block: &OemInfoBlockSummary, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let mut searchable = format!(
        "{} {} {}:{} {} {} {} {} {:#x} {:#x} {}",
        block.id,
        block.sub_id,
        block.id,
        block.sub_id,
        block.region,
        block.layout,
        block.payload_kind,
        if block.active {
            "active 活动"
        } else {
            "inactive 非活动"
        },
        block.offset,
        block.length,
        block.age,
    )
    .to_ascii_lowercase();
    for extra in [
        block.text_preview.as_deref(),
        block.tlv_description.as_deref(),
        block.image_version_hex.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        searchable.push(' ');
        searchable.push_str(&extra.to_ascii_lowercase());
    }
    searchable.contains(filter)
}

fn render_blocks_table(
    ui: &mut egui::Ui,
    summary: &OemInfoImageSummary,
    visible: &[usize],
    selected_block: Option<usize>,
    selected: &mut Option<usize>,
) {
    let table_height = (ui.clip_rect().height() * 0.44).clamp(260.0, 420.0);
    TableBuilder::new(ui)
        .id_salt("oeminfo-blocks-table")
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click())
        .min_scrolled_height(table_height)
        .max_scroll_height(table_height)
        .auto_shrink([false, false])
        .column(Column::exact(62.0))
        .column(Column::initial(92.0).at_least(76.0))
        .column(Column::exact(52.0))
        .column(Column::initial(128.0).at_least(100.0).clip(true))
        .column(Column::remainder().at_least(120.0).clip(true))
        .column(Column::exact(62.0))
        .column(Column::exact(94.0))
        .column(Column::exact(88.0))
        .header(30.0, |mut header| {
            for label in [
                "状态",
                "ID / SubID",
                "区域",
                "布局",
                "载荷类型",
                "代次",
                "偏移",
                "长度",
            ] {
                header.col(|ui| {
                    ui.strong(label);
                });
            }
        })
        .body(|mut body| {
            if visible.is_empty() {
                body.row(44.0, |mut row| {
                    for column in 0..8 {
                        row.col(|ui| {
                            if column == 4 {
                                ui.label(egui::RichText::new("没有匹配的数据块").weak());
                            }
                        });
                    }
                });
                return;
            }

            body.rows(30.0, visible.len(), |mut row| {
                let index = visible[row.index()];
                let block = &summary.blocks[index];
                row.set_selected(selected_block == Some(index));
                row.col(|ui| {
                    let (text, color) = if block.active {
                        ("活动", egui::Color32::from_rgb(95, 190, 125))
                    } else {
                        ("非活动", ui.visuals().weak_text_color())
                    };
                    ui.label(egui::RichText::new(text).strong().color(color));
                });
                row.col(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{} / {}", block.id, block.sub_id)).monospace(),
                    );
                });
                row.col(|ui| {
                    ui.label(block.region.to_string());
                });
                row.col(|ui| {
                    ui.add(egui::Label::new(block.layout.to_string()).truncate());
                });
                row.col(|ui| {
                    ui.add(egui::Label::new(block.payload_kind.to_string()).truncate());
                });
                row.col(|ui| {
                    ui.label(egui::RichText::new(block.age.to_string()).monospace());
                });
                row.col(|ui| {
                    ui.label(egui::RichText::new(format!("0x{:X}", block.offset)).monospace());
                });
                row.col(|ui| {
                    ui.label(human_size(block.length as u64));
                });
                let response = row
                    .response()
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if response.clicked() {
                    *selected = Some(index);
                }
            });
        });
}

fn render_block_details(ui: &mut egui::Ui, block: &OemInfoBlockSummary) {
    section(ui, "数据块详情");
    ui.horizontal_wrapped(|ui| {
        let color = if block.active {
            egui::Color32::from_rgb(95, 190, 125)
        } else {
            ui.visuals().weak_text_color()
        };
        ui.label(
            egui::RichText::new(if block.active {
                "活动副本"
            } else {
                "非活动副本"
            })
            .strong()
            .color(color),
        );
        ui.label(
            egui::RichText::new(format!("ID {} / SubID {}", block.id, block.sub_id))
                .strong()
                .monospace(),
        );
    });
    ui.add_space(6.0);

    let payload_offset = block.offset.saturating_add(block.header_size as u64);
    let payload_end = payload_offset.saturating_add(block.length as u64);
    egui::Grid::new("oeminfo-block-detail-grid")
        .num_columns(4)
        .spacing([18.0, 7.0])
        .show(ui, |ui| {
            detail_value(ui, "区域", block.region.to_string());
            detail_value(ui, "布局", block.layout.to_string());
            ui.end_row();
            detail_value(ui, "头版本", block.version.to_string());
            detail_value(ui, "代次", block.age.to_string());
            ui.end_row();
            detail_value(ui, "块偏移", format!("0x{:X}", block.offset));
            detail_value(ui, "头大小", format!("0x{:X}", block.header_size));
            ui.end_row();
            detail_value(
                ui,
                "载荷范围",
                format!("0x{payload_offset:X}..0x{payload_end:X}"),
            );
            detail_value(
                ui,
                "载荷大小",
                format!("{} (0x{:X})", human_size(block.length as u64), block.length),
            );
            ui.end_row();
            detail_value(ui, "载荷类型", block.payload_kind.to_string());
            detail_value(
                ui,
                "填充字节",
                format!(
                    "头 0x{:02X} / 块 0x{:02X}",
                    block.header_padding_byte, block.block_padding_byte
                ),
            );
            ui.end_row();
            if block.tlv_parts != 0 || block.tlv_description.is_some() {
                detail_value(ui, "TLV 段", block.tlv_parts.to_string());
                detail_value(
                    ui,
                    "TLV 描述",
                    block.tlv_description.as_deref().unwrap_or("-"),
                );
                ui.end_row();
            }
            if block.image_version_hex.is_some() || block.image_random_adjust.is_some() {
                let image_offset = payload_offset.saturating_add(0x1a);
                detail_value(
                    ui,
                    "镜像版本",
                    block.image_version_hex.as_deref().unwrap_or("-"),
                );
                detail_value(
                    ui,
                    "随机调整",
                    block
                        .image_random_adjust
                        .map(|value| format!("0x{value:X}"))
                        .unwrap_or_else(|| "-".to_owned()),
                );
                ui.end_row();
                detail_value(
                    ui,
                    "原始文件范围",
                    format!("0x{image_offset:X}..0x{payload_end:X}"),
                );
                detail_value(
                    ui,
                    "原始文件大小",
                    human_size(block.length.saturating_sub(0x1a) as u64),
                );
                ui.end_row();
            }
        });

    if let Some(preview) = block.text_preview.as_deref() {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("文本预览").strong());
            if ui.button("复制预览").clicked() {
                ui.ctx().copy_text(preview.to_owned());
            }
        });
        egui::Frame::group(ui.style())
            .fill(ui.visuals().extreme_bg_color)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(
                    egui::Label::new(egui::RichText::new(preview).monospace())
                        .selectable(true)
                        .wrap(),
                );
            });
    }
}

fn detail_value(ui: &mut egui::Ui, label: &str, value: impl Into<egui::WidgetText>) {
    ui.label(egui::RichText::new(label).weak());
    ui.label(value);
}
