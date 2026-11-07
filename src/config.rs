//! Persisted user settings and the undo journal.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::desktop::MoveRecord;
use crate::fences::Fence;

const APP_DIR: &str = "DeskOrg";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Index into [`crate::theme::ACCENTS`].
    pub accent: usize,
    pub light_theme: bool,
    /// Empty means `<desktop>/DeskOrg`.
    pub target_dir: String,
    /// Also scan `C:\Users\Public\Desktop`.
    pub include_public: bool,
    /// Double-click an empty area of the desktop to toggle icons.
    pub desktop_double_click: bool,
    /// Make the icons visible again when DeskOrg closes.
    pub restore_on_exit: bool,
    /// Enter clean mode right after a successful organize operation.
    pub auto_hide_after_organize: bool,
    /// Last wallpaper picked in the app, used for the preview card.
    pub wallpaper_path: String,
    /// Free-floating transparent category boxes.
    pub fences: Vec<Fence>,
    /// Whether the fence desktop view is enabled.
    pub fences_enabled: bool,
    /// Create/refresh fences after a classification operation.
    pub fences_after_organize: bool,
    /// Add DeskOrg to the current user's Windows startup entries.
    pub auto_start: bool,
    /// Keep running in the background when the main window is closed.
    pub close_to_background: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            accent: 0,
            light_theme: false,
            target_dir: String::new(),
            include_public: true,
            desktop_double_click: true,
            restore_on_exit: true,
            auto_hide_after_organize: false,
            wallpaper_path: String::new(),
            fences: Vec::new(),
            fences_enabled: false,
            fences_after_organize: true,
            auto_start: false,
            close_to_background: false,
        }
    }
}

impl Config {
    /// 读取配置；损坏时把原文件备份为 `config.json.corrupt` 并返回提示。
    pub fn load_checked() -> (Self, Option<String>) {
        Self::load_from_checked(&config_path())
    }

    #[cfg(test)]
    pub fn load_from(path: &std::path::Path) -> Self {
        Self::load_from_checked(path).0
    }

    pub fn load_from_checked(path: &std::path::Path) -> (Self, Option<String>) {
        match fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(config) => (config, None),
                Err(error) => {
                    let backup = path.with_extension("json.corrupt");
                    let _ = fs::write(&backup, &text);
                    (
                        Self::default(),
                        Some(format!(
                            "设置文件损坏（{error}），已备份到 {} 并恢复默认设置",
                            backup.display()
                        )),
                    )
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => (Self::default(), None),
            Err(error) => (
                Self::default(),
                Some(format!("无法读取设置文件：{error}")),
            ),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&config_path())
    }

    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| format!("设置序列化失败：{error}"))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("无法创建设置目录：{error}"))?;
        }
        write_atomic(path, text.as_bytes()).map_err(|error| format!("无法写入设置文件：{error}"))
    }

    pub fn resolved_target(&self) -> PathBuf {
        let raw = self.target_dir.trim();
        if raw.is_empty() {
            return default_target_dir();
        }
        let path = PathBuf::from(raw);
        if path.is_absolute() {
            path
        } else {
            crate::desktop::user_desktop_dir()
                .map(|desktop| desktop.join(&path))
                .unwrap_or(path)
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UndoState {
    /// Unix seconds, only used for display.
    pub created_at: u64,
    pub moves: Vec<MoveRecord>,
}

impl UndoState {
    /// 读取撤销记录；损坏时备份为 `undo.json.corrupt` 并返回提示。
    pub fn load_checked() -> (Option<Self>, Option<String>) {
        Self::load_from_checked(&undo_path())
    }

    #[cfg(test)]
    pub fn load_from(path: &std::path::Path) -> Option<Self> {
        Self::load_from_checked(path).0
    }

    pub fn load_from_checked(path: &std::path::Path) -> (Option<Self>, Option<String>) {
        match fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Self>(&text) {
                Ok(state) if state.moves.is_empty() => (None, None),
                Ok(state) => (Some(state), None),
                Err(error) => {
                    let backup = path.with_extension("json.corrupt");
                    let _ = fs::write(&backup, &text);
                    (
                        None,
                        Some(format!(
                            "撤销记录损坏（{error}），已备份到 {} 并忽略",
                            backup.display()
                        )),
                    )
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => (None, None),
            Err(error) => (None, Some(format!("无法读取撤销记录：{error}"))),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&undo_path())
    }

    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("无法创建撤销记录目录：{error}"))?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| format!("撤销记录序列化失败：{error}"))?;
        write_atomic(path, text.as_bytes()).map_err(|error| format!("无法写入撤销记录：{error}"))
    }

    pub fn clear() -> Result<(), String> {
        match fs::remove_file(undo_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("无法删除撤销记录：{error}")),
        }
    }
}

pub fn app_data_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR)
}

pub fn config_path() -> PathBuf {
    app_data_dir().join("config.json")
}

pub fn undo_path() -> PathBuf {
    app_data_dir().join("undo.json")
}

pub fn default_target_dir() -> PathBuf {
    crate::desktop::user_desktop_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(crate::desktop::TARGET_FOLDER_NAME)
}

fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes)?;
    // Windows 上 std::fs::rename 使用 MOVEFILE_REPLACE_EXISTING，可直接覆盖旧文件。
    // 先删旧文件会在删掉与改名之间留下“配置文件不存在”的窗口。
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trip() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp");
        fs::create_dir_all(&base).unwrap();
        let path = base.join(format!("config-test-{}.json", std::process::id()));
        let config = Config {
            accent: 3,
            light_theme: true,
            target_dir: r"D:\Desktop\Archive".to_owned(),
            include_public: false,
            desktop_double_click: false,
            restore_on_exit: true,
            auto_hide_after_organize: true,
            wallpaper_path: r"C:\wallpaper.jpg".to_owned(),
            fences: Vec::new(),
            fences_enabled: true,
            fences_after_organize: false,
            auto_start: true,
            close_to_background: true,
        };
        config.save_to(&path).unwrap();
        let loaded = Config::load_from(&path);
        assert_eq!(loaded.accent, 3);
        assert!(loaded.light_theme);
        assert_eq!(loaded.target_dir, r"D:\Desktop\Archive");
        assert!(!loaded.include_public);
        assert!(!loaded.desktop_double_click);
        assert!(loaded.auto_hide_after_organize);
        assert_eq!(loaded.wallpaper_path, r"C:\wallpaper.jpg");
        assert!(loaded.fences_enabled);
        assert!(!loaded.fences_after_organize);
        assert!(loaded.auto_start);
        assert!(loaded.close_to_background);
        let _ = fs::remove_file(path);
    }

    fn test_base() -> PathBuf {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp");
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn undo_save_to_overwrites_existing_file_and_leaves_no_temp() {
        let path = test_base().join(format!("undo-test-{}.json", std::process::id()));
        let first = UndoState {
            created_at: 1,
            moves: vec![MoveRecord {
                from: PathBuf::from(r"C:\Desktop\a.png"),
                to: PathBuf::from(r"C:\Desktop\DeskOrg\图片\a.png"),
                category: crate::desktop::Category::Image,
                is_dir: false,
            }],
        };
        first.save_to(&path).unwrap();

        let second = UndoState {
            created_at: 2,
            moves: first.moves.clone(),
        };
        second.save_to(&path).unwrap();

        let loaded = UndoState::load_from(&path).unwrap();
        assert_eq!(loaded.created_at, 2);
        assert_eq!(loaded.moves.len(), 1);
        assert!(!path.with_extension("json.tmp").exists());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn undo_save_to_reports_error_when_parent_is_a_file() {
        let blocked = test_base().join(format!("undo-blocked-{}.txt", std::process::id()));
        fs::write(&blocked, b"x").unwrap();
        let state = UndoState::default();
        assert!(state.save_to(&blocked.join("undo.json")).is_err());
        let _ = fs::remove_file(blocked);
    }

    #[test]
    fn corrupt_config_is_backed_up_and_reported() {
        let path = test_base().join(format!("config-corrupt-{}.json", std::process::id()));
        fs::write(&path, b"{not valid json").unwrap();

        let (config, warning) = Config::load_from_checked(&path);

        assert_eq!(config.accent, Config::default().accent);
        assert!(warning.is_some());
        let backup = path.with_extension("json.corrupt");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "{not valid json");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(backup);
    }

    #[test]
    fn corrupt_undo_is_backed_up_and_reported() {
        let path = test_base().join(format!("undo-corrupt-{}.json", std::process::id()));
        fs::write(&path, b"not json").unwrap();

        let (state, warning) = UndoState::load_from_checked(&path);

        assert!(state.is_none());
        assert!(warning.is_some());
        let backup = path.with_extension("json.corrupt");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "not json");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(backup);
    }
}
