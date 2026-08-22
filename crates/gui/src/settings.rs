//! Persistent UI settings (last-used paths and options), stored as JSON in
//! the platform data directory.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Directory the file dialogs start from.
    pub last_dir: Option<String>,
    /// Directory used for extraction outputs.
    pub last_output: Option<String>,
    /// Recently opened files (most recent first).
    pub recent: Vec<String>,
    pub show_log: bool,
    pub force: bool,
    pub allow_grow: bool,
    pub all_erofs: bool,
    /// update.bin 布局: "auto" | "l1" | "l2"
    pub layout: String,
    /// EROFS 工具目录（为空则自动查找）
    pub tools_dir: String,
    pub cpio_extract_dir: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            last_dir: None,
            last_output: None,
            recent: Vec::new(),
            show_log: true,
            force: false,
            allow_grow: false,
            all_erofs: false,
            layout: "auto".to_owned(),
            tools_dir: String::new(),
            cpio_extract_dir: None,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(parent) = path().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path(), json);
        }
    }

    pub fn remember_path(&mut self, path: &std::path::Path) {
        if path.is_file() {
            let parent = path.parent().map(|p| p.display().to_string());
            if let Some(parent) = parent {
                self.last_dir = Some(parent);
            }
            let text = path.display().to_string();
            self.recent.retain(|entry| entry != &text);
            self.recent.insert(0, text);
            self.recent.truncate(8);
        } else if path.is_dir() {
            self.last_dir = Some(path.display().to_string());
        }
    }
}

fn path() -> PathBuf {
    let mut dir = data_dir();
    dir.push("haucet-tools-gui");
    let _ = std::fs::create_dir_all(&dir);
    dir.push("settings.json");
    dir
}

fn data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata);
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join("Library/Application Support");
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(xdg);
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".local/share");
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}
