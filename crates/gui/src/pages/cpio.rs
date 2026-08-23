use crate::app::HaucetApp;
use crate::pages::badge_text;
use crate::util::{human_size, message_box, mode_string, open_in_file_manager};
use common::compress::decompress_vec;
use common::formats::cpio::{self, Cpio, S_IFDIR, S_IFMT};
use common::formats::harmony::HvbFrame;
use common::formats::header::check_fmt;
use eframe::egui;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::thread;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CpioSource {
    #[default]
    File,
    Image,
    Workspace,
}

impl CpioSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::File => "cpio 文件",
            Self::Image => "ramdisk 镜像",
            Self::Workspace => "解包工作区",
        }
    }
}

#[derive(Debug, Default)]
pub struct CpioPage {
    pub source: CpioSource,
    pub path: String,
    pub loaded: Option<Loaded>,
    pub load_job: Option<LocalJob>,
    pub message: Option<(bool, String)>,
    pub selection: Option<String>,
    pub filter: String,
    pub expand: bool,
    pub dirty: bool,
    pub add_target: String,
    pub add_mode: String,
    pub mkdir_target: String,
    pub pending_add: Option<String>,
}

pub struct Loaded {
    pub cpio: Cpio,
    pub source_path: String,
    pub from_image: bool,
    children: HashMap<String, Vec<String>>,
}

impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loaded")
            .field("entries", &self.cpio.entries.len())
            .field("source_path", &self.source_path)
            .field("from_image", &self.from_image)
            .finish()
    }
}

impl Loaded {
    fn new(cpio: Cpio, source_path: String, from_image: bool) -> Self {
        let mut loaded = Self {
            cpio,
            source_path,
            from_image,
            children: HashMap::new(),
        };
        loaded.rebuild_tree();
        loaded
    }

    fn rebuild_tree(&mut self) {
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        for path in self.cpio.entries.keys() {
            let (parent, name) = split_path(path);
            children.entry(parent).or_default().push(name);
        }
        for names in children.values_mut() {
            names.sort();
        }
        self.children = children;
    }

    fn stats(&self) -> (usize, u64, usize) {
        let mut total = 0_u64;
        let mut dirs = 0_usize;
        for entry in self.cpio.entries.values() {
            if entry.mode & S_IFMT == S_IFDIR {
                dirs += 1;
            }
            total += entry.data.len() as u64;
        }
        (self.cpio.entries.len(), total, dirs)
    }

    /// Snapshot of the archive for use in a worker thread (entries are
    /// cloned; `Cpio` itself is cheap to re-wrap since it owns its data).
    fn snapshot(&self) -> Cpio {
        Cpio {
            entries: self.cpio.entries.clone(),
        }
    }
}

enum LocalOutcome {
    Loaded(Loaded),
    Done(String),
}

pub struct LocalJob {
    rx: Receiver<std::result::Result<LocalOutcome, String>>,
    pub label: &'static str,
}

impl std::fmt::Debug for LocalJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalJob")
            .field("label", &self.label)
            .finish()
    }
}

impl LocalJob {
    fn poll(&mut self) -> Option<std::result::Result<LocalOutcome, String>> {
        self.rx.try_recv().ok()
    }
}

fn spawn_local<F>(label: &'static str, work: F) -> LocalJob
where
    F: FnOnce() -> anyhow::Result<LocalOutcome> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = work().map_err(|error| format!("{error:#}"));
        let _ = tx.send(result);
    });
    LocalJob { rx, label }
}

fn split_path(path: &str) -> (String, String) {
    match path.rfind('/') {
        Some(index) => (path[..index].to_owned(), path[index + 1..].to_owned()),
        None => (String::new(), path.to_owned()),
    }
}

/// TODO REMOVE
/// 快速判断文件是否以 HARMONY! 头开始（ramdisk 镜像）。
fn is_harmony_image(path: &std::path::Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0_u8; 8];
    file.read_exact(&mut head).is_ok() && &head == b"HARMONY!"
}

impl CpioPage {
    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        self.poll_local_job();
        let mut loaded = self.loaded.take();

        egui::ScrollArea::vertical()
            .id_salt("cpio-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_space(6.0);

                self.open_row(ui, app);
                ui.add_space(6.0);
                if let Some(loaded) = &loaded {
                    self.summary_row(ui, loaded);
                }
                if let Some((ok, text)) = &self.message {
                    ui.add_space(6.0);
                    let color = if *ok {
                        egui::Color32::from_rgb(90, 200, 120)
                    } else {
                        egui::Color32::from_rgb(230, 90, 90)
                    };
                    message_box(ui, color, text);
                }
                ui.add_space(6.0);
                if let Some(loaded) = &mut loaded {
                    self.browser(ui, app, loaded);
                }
                ui.add_space(20.0);
            });

        self.loaded = loaded;
    }

    fn open_row(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("打开来源").strong());
            for source in [CpioSource::File, CpioSource::Image, CpioSource::Workspace] {
                ui.selectable_value(&mut self.source, source, source.label());
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.path)
                    .hint_text(match self.source {
                        CpioSource::File => "ramdisk.cpio 文件路径",
                        CpioSource::Image => "ramdisk 镜像路径",
                        CpioSource::Workspace => "解包工作区目录",
                    })
                    .desired_width(ui.available_width() - 240.0),
            );
            if ui.button("选择…").clicked() {
                let picked = match self.source {
                    CpioSource::File => app.pick_file("选择 cpio 文件", &[("cpio", &["cpio"])]),
                    CpioSource::Image => app.pick_file("选择 ramdisk 镜像", &[("镜像", &["img"])]),
                    CpioSource::Workspace => app.pick_dir("选择解包工作区"),
                };
                if let Some(path) = picked {
                    self.path = path.display().to_string();
                }
            }
            if ui
                .add_enabled(
                    !self.path.trim().is_empty() && self.load_job.is_none(),
                    egui::Button::new("加载"),
                )
                .clicked()
            {
                self.start_load(app);
            }
        });
        let drops = app.take_drops(ui.ctx());
        if let Some(path) = drops.first() {
            self.path = path.display().to_string();
            if path.is_dir() {
                self.source = CpioSource::Workspace;
            } else if is_harmony_image(path) {
                self.source = CpioSource::Image;
            }
            self.start_load(app);
        }
    }

    fn start_load(&mut self, app: &mut HaucetApp) {
        let source = self.source;
        let path = self.path.trim().to_owned();
        if path.is_empty() || self.load_job.is_some() {
            return;
        }
        app.settings.remember_path(std::path::Path::new(&path));
        self.loaded = None;
        self.selection = None;
        self.message = None;
        self.dirty = false;
        self.load_job = Some(spawn_local("加载 cpio", move || {
            let (cpio, from_image, source_path) = match source {
                CpioSource::File => (Cpio::load_from_file(&path)?, false, path.clone()),
                CpioSource::Workspace => {
                    let cpio_path = std::path::Path::new(&path).join("ramdisk.cpio");
                    let text = cpio_path
                        .to_str()
                        .ok_or_else(|| anyhow::anyhow!("路径不是 UTF-8"))?;
                    (Cpio::load_from_file(text)?, false, text.to_owned())
                }
                CpioSource::Image => {
                    let frame = HvbFrame::load(std::path::Path::new(&path))?;
                    let payload = frame.extract_image_payload();
                    anyhow::ensure!(!payload.is_empty(), "镜像内没有负载数据");
                    let fmt = check_fmt(payload);
                    let bytes = if fmt.is_compressed() {
                        decompress_vec(fmt, payload).map_err(std::io::Error::other)?
                    } else {
                        payload.to_vec()
                    };
                    (Cpio::load_from_data(&bytes)?, true, path.clone())
                }
            };
            let loaded = Loaded::new(cpio, source_path, from_image);
            Ok(LocalOutcome::Loaded(loaded))
        }));
    }

    fn poll_local_job(&mut self) {
        let Some(job) = &mut self.load_job else {
            return;
        };
        let Some(result) = job.poll() else {
            return;
        };
        let label = job.label;
        self.load_job = None;
        match result {
            Ok(LocalOutcome::Loaded(loaded)) => {
                self.message = Some((
                    true,
                    format!(
                        "已加载 {} 个条目（来源：{}）",
                        loaded.cpio.entries.len(),
                        label
                    ),
                ));
                self.loaded = Some(loaded);
            }
            Ok(LocalOutcome::Done(text)) => {
                self.message = Some((true, text));
            }
            Err(error) => {
                self.message = Some((false, error));
            }
        }
    }

    fn summary_row(&self, ui: &mut egui::Ui, loaded: &Loaded) {
        let (count, total, dirs) = loaded.stats();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} 个条目 · {} 个目录 · 数据 {}",
                    count,
                    dirs,
                    human_size(total)
                ))
                .weak(),
            );
            ui.separator();
            ui.label(egui::RichText::new("补丁状态").weak());
            if loaded.cpio.exists(".backup/init_early") {
                badge_text(ui, "已打过补丁", egui::Color32::from_rgb(230, 170, 40));
            } else if loaded.cpio.exists("bin/init_early") {
                badge_text(
                    ui,
                    "原厂（bin/init_early 存在）",
                    egui::Color32::from_rgb(90, 200, 120),
                );
            } else if loaded.cpio.exists("init") {
                badge_text(
                    ui,
                    "原厂（init 存在）",
                    egui::Color32::from_rgb(90, 200, 120),
                );
            } else {
                badge_text(ui, "未知布局", egui::Color32::from_rgb(230, 170, 40));
            }
            if self.dirty {
                ui.separator();
                badge_text(ui, "有未保存的修改", egui::Color32::from_rgb(230, 170, 40));
            }
            if !loaded.from_image {
                ui.separator();
                ui.label(egui::RichText::new(&loaded.source_path).weak().monospace());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("打开来源位置").clicked()
                    && let Some(parent) = std::path::Path::new(&loaded.source_path).parent()
                {
                    open_in_file_manager(parent);
                }
            });
        });
        ui.add_space(4.0);
    }

    fn browser(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp, loaded: &mut Loaded) {
        ui.horizontal(|ui| {
            ui.label("过滤");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("按路径关键字过滤")
                    .desired_width(220.0),
            );
            if ui.button("展开全部").clicked() {
                self.expand = true;
            }
            if ui.button("折叠全部").clicked() {
                self.expand = false;
            }
            ui.label(egui::RichText::new("单击选中条目 · 双击复制路径").weak());
        });
        ui.add_space(2.0);

        let mut clicked: Option<String> = None;
        let filter = self.filter.trim().to_lowercase();
        egui::ScrollArea::vertical()
            .id_salt("cpio-tree")
            .max_height(300.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if filter.is_empty() {
                    self.draw_dir(ui, loaded, "", &mut clicked);
                } else {
                    for path in loaded.cpio.entries.keys() {
                        if path.to_lowercase().contains(&filter) {
                            let selected = self.selection.as_deref() == Some(path.as_str());
                            let response = ui.selectable_label(selected, format!("  {path}"));
                            if response.clicked() {
                                clicked = Some(path.clone());
                            }
                            if response.double_clicked() {
                                ui.ctx().copy_text(path.clone());
                            }
                        }
                    }
                }
            });
        if let Some(path) = clicked {
            self.selection = Some(path);
        }

        ui.add_space(6.0);
        self.actions_row(ui, app, loaded);
        ui.add_space(6.0);
        self.detail_panel(ui, loaded);
    }

    #[allow(clippy::only_used_in_recursion)]
    fn draw_dir(
        &self,
        ui: &mut egui::Ui,
        loaded: &Loaded,
        dir: &str,
        clicked: &mut Option<String>,
    ) {
        let Some(names) = loaded.children.get(dir) else {
            return;
        };
        for name in names {
            let full = if dir.is_empty() {
                name.clone()
            } else {
                format!("{dir}/{name}")
            };
            let is_dir = loaded
                .cpio
                .entries
                .get(&full)
                .map(|entry| entry.mode & S_IFMT == S_IFDIR)
                .unwrap_or(false);
            if is_dir {
                let default_open = self.expand || dir.is_empty();
                egui::CollapsingHeader::new(format!("📁 {name}"))
                    .id_salt(("cpio-dir", full.as_str(), self.expand))
                    .default_open(default_open)
                    .show(ui, |ui| self.draw_dir(ui, loaded, &full, clicked));
            } else {
                let selected = self.selection.as_deref() == Some(full.as_str());
                let response = ui.selectable_label(selected, format!("   {name}"));
                if response.clicked() {
                    *clicked = Some(full.clone());
                }
                if response.double_clicked() {
                    ui.ctx().copy_text(full.clone());
                }
            }
        }
    }

    fn actions_row(&mut self, ui: &mut egui::Ui, app: &mut HaucetApp, loaded: &mut Loaded) {
        let selection = self.selection.clone();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(selection.is_some(), egui::Button::new("提取选中…"))
                .clicked()
                && let Some(dir) = app.pick_dir("选择提取目标目录")
            {
                let entry = selection.clone().unwrap_or_default();
                let dir = dir.display().to_string();
                let snapshot = loaded.snapshot();
                self.message = None;
                self.load_job = Some(spawn_local("提取条目", move || {
                    extract_entries(&snapshot, std::slice::from_ref(&entry), &dir)?;
                    Ok(LocalOutcome::Done(format!("已提取 {entry}")))
                }));
            }
            if ui.button("提取全部…").clicked()
                && let Some(dir) = app.pick_dir("选择提取目标目录")
            {
                let paths = loaded.cpio.entries.keys().cloned().collect::<Vec<_>>();
                let dir = dir.display().to_string();
                let snapshot = loaded.snapshot();
                self.message = None;
                self.load_job = Some(spawn_local("提取全部", move || {
                    let count = extract_entries(&snapshot, &paths, &dir)?;
                    Ok(LocalOutcome::Done(format!("已提取 {count} 个条目到 {dir}")))
                }));
            }
            if ui
                .add_enabled(selection.is_some(), egui::Button::new("删除选中"))
                .clicked()
                && let Some(entry) = selection.clone()
            {
                let is_dir = loaded
                    .cpio
                    .entries
                    .get(&entry)
                    .map(|entry| entry.mode & S_IFMT == S_IFDIR)
                    .unwrap_or(false);
                loaded.cpio.rm(&entry, is_dir);
                loaded.rebuild_tree();
                self.selection = None;
                self.dirty = true;
                self.message = Some((true, format!("已删除 {entry}")));
            }
            if ui.button("添加文件…").clicked()
                && let Some(file) = app.pick_file("选择要添加的文件", &[])
            {
                let suggested = file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.pending_add = Some(file.display().to_string());
                self.add_target = suggested;
                self.add_mode = "0750".to_owned();
            }
            if ui.button("新建目录").clicked() {
                self.mkdir_target = "new/dir".to_owned();
            }
        });

        if self.pending_add.is_some() {
            ui.add_space(4.0);
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("归档路径");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.add_target)
                                .hint_text("例如 bin/init_early")
                                .desired_width(240.0),
                        );
                        ui.label("模式");
                        ui.add(egui::TextEdit::singleline(&mut self.add_mode).desired_width(64.0));
                        if ui.button("确认添加").clicked() {
                            let mode = cpio::parse_cpio_mode(self.add_mode.trim());
                            let target = self.add_target.trim().to_owned();
                            match (mode, self.pending_add.clone()) {
                                (Ok(mode), Some(file)) if !target.is_empty() => {
                                    match loaded.cpio.add(mode, &target, &file) {
                                        Ok(()) => {
                                            loaded.rebuild_tree();
                                            self.dirty = true;
                                            self.message =
                                                Some((true, format!("已添加 {file} → {target}")));
                                            self.pending_add = None;
                                        }
                                        Err(error) => {
                                            self.message =
                                                Some((false, format!("添加失败：{error}")));
                                        }
                                    }
                                }
                                (Err(error), _) => {
                                    self.message = Some((false, format!("无效的模式：{error}")));
                                }
                                _ => {
                                    self.message =
                                        Some((false, "请选择文件并填写归档路径".to_owned()));
                                }
                            }
                        }
                        if ui.button("取消").clicked() {
                            self.pending_add = None;
                        }
                    });
                });
        }
        if !self.mkdir_target.is_empty() {
            ui.add_space(4.0);
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("目录路径");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.mkdir_target)
                                .hint_text("例如 new/dir")
                                .desired_width(240.0),
                        );
                        if ui.button("创建").clicked() {
                            let target = self.mkdir_target.trim().to_owned();
                            if !target.is_empty() {
                                loaded.cpio.mkdir(0o750, &target);
                                loaded.rebuild_tree();
                                self.dirty = true;
                                self.message = Some((true, format!("已创建目录 {target}")));
                                self.mkdir_target.clear();
                            }
                        }
                    });
                });
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !loaded.from_image && self.dirty,
                    egui::Button::new("保存修改"),
                )
                .on_hover_text("写回当前来源文件")
                .clicked()
            {
                let path = loaded.source_path.clone();
                let snapshot = loaded.snapshot();
                self.message = None;
                self.load_job = Some(spawn_local("保存", move || {
                    let mut bytes = Vec::new();
                    snapshot.dump_to(&mut bytes)?;
                    std::fs::write(&path, bytes)?;
                    Ok(LocalOutcome::Done(format!("已保存到 {path}")))
                }));
                self.dirty = false;
            }
            if ui.button("另存为…").clicked()
                && let Some(path) = app.pick_save("保存 cpio 文件", "ramdisk.cpio")
            {
                let path = path.display().to_string();
                let snapshot = loaded.snapshot();
                self.message = None;
                self.load_job = Some(spawn_local("另存为", move || {
                    let mut bytes = Vec::new();
                    snapshot.dump_to(&mut bytes)?;
                    std::fs::write(&path, bytes)?;
                    Ok(LocalOutcome::Done(format!("已保存到 {path}")))
                }));
                self.dirty = false;
            }
            if loaded.from_image {
                ui.label(
                    egui::RichText::new(
                        "内容来自镜像：修改后请“另存为”cpio, 再到 Ramdisk 页重新打包",
                    )
                    .weak(),
                );
            }
        });
    }

    fn detail_panel(&self, ui: &mut egui::Ui, loaded: &Loaded) {
        let Some(selection) = &self.selection else {
            return;
        };
        let Some(entry) = loaded.cpio.entries.get(selection) else {
            return;
        };
        ui.add_space(4.0);
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.label(egui::RichText::new(selection).strong());
                egui::Grid::new("cpio-entry-detail")
                    .num_columns(2)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        crate::util::kv(ui, "权限", mode_string(entry.mode));
                        crate::util::kv(ui, "uid / gid", format!("{} / {}", entry.uid, entry.gid));
                        crate::util::kv(ui, "大小", human_size(entry.data.len() as u64));
                    });
            });
    }
}

fn extract_entries(cpio: &Cpio, paths: &[String], dir: &str) -> anyhow::Result<usize> {
    let mut count = 0;
    for path in paths {
        let output = format!("{dir}/{path}");
        cpio.extract_entry(path, &output)
            .map_err(|error| anyhow::anyhow!("提取 {path} 失败：{error}"))?;
        count += 1;
    }
    Ok(count)
}
