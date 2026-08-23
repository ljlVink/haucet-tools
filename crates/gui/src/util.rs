use eframe::egui;

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn hex64(value: u64) -> String {
    format!("0x{value:X}")
}

pub fn open_in_file_manager(path: &std::path::Path) {
    let path_text = path.display().to_string();
    let result = open_command(path);
    if let Some(mut command) = result {
        let _ = command.spawn();
    }
    let _ = path_text;
}

fn open_command(path: &std::path::Path) -> Option<std::process::Command> {
    #[cfg(target_os = "windows")]
    {
        let mut command = std::process::Command::new("explorer");
        command.arg(path);
        return Some(command);
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("open");
        command.arg("-R").arg(path);
        return Some(command);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(path);
        return Some(command);
    }
    #[allow(unreachable_code)]
    {
        let _ = path;
        None
    }
}

pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.clone().next() == Some('[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

pub fn kv(ui: &mut egui::Ui, key: &str, value: impl Into<egui::WidgetText>) {
    ui.label(egui::RichText::new(key).weak());
    ui.label(value);
    ui.end_row();
}

pub fn message_box(ui: &mut egui::Ui, color: egui::Color32, text: impl Into<egui::WidgetText>) {
    let text = text.into();
    egui::Frame::group(ui.style())
        .fill(color.gamma_multiply(0.08))
        .stroke(egui::Stroke::new(1.0_f32, color.gamma_multiply(0.6)))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.label(text);
        });
}

pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title).strong().size(16.0));
    ui.add_space(2.0);
}

pub fn mode_string(mode: u32) -> String {
    use common::formats::cpio::*;
    let type_char = match mode & S_IFMT {
        S_IFDIR => 'd',
        S_IFREG => '-',
        S_IFLNK => 'l',
        S_IFBLK => 'b',
        S_IFCHR => 'c',
        _ => '?',
    };
    let mut out = String::with_capacity(10);
    out.push(type_char);
    for (bit, ch) in [
        (S_IRUSR, 'r'),
        (S_IWUSR, 'w'),
        (S_IXUSR, 'x'),
        (S_IRGRP, 'r'),
        (S_IWGRP, 'w'),
        (S_IXGRP, 'x'),
        (S_IROTH, 'r'),
        (S_IWOTH, 'w'),
        (S_IXOTH, 'x'),
    ] {
        out.push(if mode & bit != 0 { ch } else { '-' });
    }
    out
}
