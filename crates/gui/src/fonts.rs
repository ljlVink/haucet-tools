use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MIN_FONT_BYTES: u64 = 100 * 1024;
const MAX_FONT_BYTES: u64 = 64 * 1024 * 1024;

const CJK_KEYWORDS: &[&str] = &[
    "cjk",
    "wqy",
    "droid",
    "noto",
    "han",
    "hei",
    "song",
    "ming",
    "pingfang",
    "yahei",
    "sourcehan",
    "uming",
    "ukai",
    "microhei",
    "zenhei",
    "fallback",
];

pub fn install_cjk_font(ctx: &egui::Context) -> bool {
    for candidate in candidate_paths() {
        let Some(bytes) = load_candidate(&candidate) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "cjk".to_owned(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("cjk".to_owned());
        }
        ctx.set_fonts(fonts);
        return true;
    }
    false
}

fn load_candidate(path: &Path) -> Option<Vec<u8>> {
    let metadata = std::fs::metadata(path).ok()?;
    let length = metadata.len();
    if !(MIN_FONT_BYTES..=MAX_FONT_BYTES).contains(&length) {
        return None;
    }
    std::fs::read(path).ok()
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    named_candidates(&mut candidates);
    scanned_candidates(&mut candidates);
    candidates
}

fn named_candidates(candidates: &mut Vec<PathBuf>) {
    #[cfg(windows)]
    {
        let dir = PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into()));
        let fonts = dir.join("Fonts");
        for name in [
            "msyh.ttc",
            "msyhbd.ttc",
            "msyhl.ttc",
            "simhei.ttf",
            "simsun.ttc",
            "Deng.ttf",
            "simkai.ttf",
        ] {
            candidates.push(fonts.join(name));
        }
    }
    #[cfg(target_os = "macos")]
    {
        for dir in ["/System/Library/Fonts", "/Library/Fonts"] {
            for name in [
                "PingFang.ttc",
                "STHeiti Light.ttc",
                "Hiragino Sans GB.ttc",
                "Arial Unicode.ttf",
            ] {
                candidates.push(PathBuf::from(dir).join(name));
            }
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for path in [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
            "/usr/share/fonts/truetype/arphic/uming.ttc",
            "/usr/share/fonts/truetype/arphic/ukai.ttc",
            "/usr/share/fonts/adobe-source-han-sans/SourceHanSansCN-Regular.otf",
        ] {
            candidates.push(PathBuf::from(path));
        }
    }
}

fn scanned_candidates(candidates: &mut Vec<PathBuf>) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut roots = vec![
            PathBuf::from("/usr/share/fonts"),
            PathBuf::from("/usr/local/share/fonts"),
        ];
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            roots.push(home.join(".fonts"));
            roots.push(home.join(".local/share/fonts"));
        }
        for root in roots {
            scan_font_dir(&root, candidates, 0);
        }
    }
}

fn scan_font_dir(dir: &Path, candidates: &mut Vec<PathBuf>, depth: usize) {
    if depth > 3 || !dir.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_font_dir(&path, candidates, depth + 1);
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        let has_font_ext =
            name.ends_with(".ttf") || name.ends_with(".ttc") || name.ends_with(".otf");
        if has_font_ext && CJK_KEYWORDS.iter().any(|keyword| name.contains(keyword)) {
            candidates.push(path);
        }
    }
}
