//! Desktop scanning, file categorisation, moving and undo.
//!
//! Everything in this module is deliberately filesystem-only: no shell COM
//! objects and no elevation. That keeps the dangerous part (moving user data)
//! easy to test and easy to reason about.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

pub const TARGET_FOLDER_NAME: &str = "DeskOrg";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Category {
    Folder,
    Shortcut,
    Image,
    Document,
    Video,
    Audio,
    Archive,
    Installer,
    Code,
    Other,
}

impl Category {
    pub const ALL: [Self; 10] = [
        Self::Folder,
        Self::Shortcut,
        Self::Image,
        Self::Document,
        Self::Video,
        Self::Audio,
        Self::Archive,
        Self::Installer,
        Self::Code,
        Self::Other,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Folder => "文件夹",
            Self::Shortcut => "快捷方式",
            Self::Image => "图片",
            Self::Document => "文档",
            Self::Video => "视频",
            Self::Audio => "音频",
            Self::Archive => "压缩包",
            Self::Installer => "安装包",
            Self::Code => "代码",
            Self::Other => "其他",
        }
    }

    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Folder => "夹",
            Self::Shortcut => "链",
            Self::Image => "图",
            Self::Document => "文",
            Self::Video => "影",
            Self::Audio => "音",
            Self::Archive => "压",
            Self::Installer => "装",
            Self::Code => "码",
            Self::Other => "…",
        }
    }

    pub const fn folder_name(self) -> &'static str {
        self.label()
    }

    pub const fn rgb(self) -> [u8; 3] {
        match self {
            Self::Folder => [125, 211, 252],
            Self::Shortcut => [196, 181, 253],
            Self::Image => [94, 234, 212],
            Self::Document => [96, 165, 250],
            Self::Video => [244, 114, 182],
            Self::Audio => [251, 191, 36],
            Self::Archive => [167, 139, 250],
            Self::Installer => [251, 146, 60],
            Self::Code => [74, 222, 128],
            Self::Other => [148, 163, 184],
        }
    }

    pub fn from_extension(ext: &str) -> Self {
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tif" | "tiff"
            | "heic" | "avif" | "psd" | "raw" | "cr2" | "nef" => Self::Image,
            "doc" | "docx" | "pdf" | "txt" | "md" | "rtf" | "xls" | "xlsx" | "csv" | "ppt"
            | "pptx" | "odt" | "ods" | "odp" | "epub" | "mobi" | "pages" | "numbers" | "key" => {
                Self::Document
            }
            "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m4v" | "mpg" | "mpeg"
            | "m2ts" | "rmvb" => Self::Video,
            "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "wma" | "opus" | "ape" => Self::Audio,
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "iso" | "cab" | "lz4" | "zst" => {
                Self::Archive
            }
            "exe" | "msi" | "msix" | "msixbundle" | "appx" | "appxbundle" | "deb" | "rpm"
            | "dmg" | "pkg" => Self::Installer,
            "rs" | "py" | "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "html" | "htm" | "css"
            | "scss" | "json" | "toml" | "yaml" | "yml" | "c" | "h" | "cpp" | "hpp" | "cc"
            | "cs" | "java" | "kt" | "go" | "sh" | "bash" | "ps1" | "bat" | "cmd" | "sql"
            | "xml" | "vue" | "svelte" => Self::Code,
            "lnk" | "url" | "website" | "appref-ms" | "desktop" => Self::Shortcut,
            _ => Self::Other,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopOrigin {
    User,
    Public,
}

impl DesktopOrigin {
    pub const fn label(self) -> &'static str {
        match self {
            Self::User => "我的桌面",
            Self::Public => "公共桌面",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DesktopItem {
    pub path: PathBuf,
    pub name: String,
    pub category: Category,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub origin: DesktopOrigin,
}

impl DesktopItem {
    pub fn extension(&self) -> String {
        self.path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    }
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub items: Vec<DesktopItem>,
    pub roots: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MoveRecord {
    pub from: PathBuf,
    pub to: PathBuf,
    pub category: Category,
    pub is_dir: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FailedMove {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct OrganizeReport {
    pub moved: Vec<MoveRecord>,
    pub failed: Vec<FailedMove>,
    pub cancelled: bool,
}

#[derive(Debug, Default)]
pub struct UndoReport {
    pub restored: Vec<MoveRecord>,
    pub failed: Vec<FailedMove>,
    pub cancelled: bool,
}

pub fn user_desktop_dir() -> Option<PathBuf> {
    if let Some(path) = dirs::desktop_dir() {
        if path.exists() {
            return Some(path);
        }
    }
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|home| home.join("Desktop"))
        .filter(|path| path.exists())
}

pub fn public_desktop_dir() -> Option<PathBuf> {
    std::env::var_os("PUBLIC")
        .map(PathBuf::from)
        .map(|public| public.join("Desktop"))
        .filter(|path| path.exists())
}

pub fn desktop_roots(include_public: bool) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(user) = user_desktop_dir() {
        roots.push(user);
    }
    if include_public {
        if let Some(public) = public_desktop_dir() {
            if !roots.iter().any(|root| same_path(root, &public)) {
                roots.push(public);
            }
        }
    }
    roots
}

/// Scan the configured desktop roots. `target` (the DeskOrg folder itself) is
/// never returned as an item, so repeated scans cannot re-organise the archive.
pub fn scan(include_public: bool, target: &Path) -> ScanResult {
    let roots = desktop_roots(include_public);
    let mut result = ScanResult {
        roots,
        ..Default::default()
    };

    for (index, root) in result.roots.clone().iter().enumerate() {
        let origin = if index == 0 {
            DesktopOrigin::User
        } else {
            DesktopOrigin::Public
        };
        scan_root(root, target, origin, &mut result);
    }

    result.items.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.path.cmp(&b.path))
    });
    result
}

pub fn scan_root(root: &Path, target: &Path, origin: DesktopOrigin, result: &mut ScanResult) {
    let read_dir = match fs::read_dir(root) {
        Ok(read_dir) => read_dir,
        Err(err) => {
            result
                .warnings
                .push(format!("无法读取 {}：{err}", root.display()));
            return;
        }
    };

    for entry in read_dir.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if is_ignored_name(&name) || is_inside(&path, target) || same_path(&path, target) {
            continue;
        }

        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(err) => {
                result.warnings.push(format!("无法读取 {name}：{err}"));
                continue;
            }
        };

        if is_hidden_or_system(&metadata) || metadata.file_type().is_symlink() {
            continue;
        }

        let is_dir = metadata.is_dir();
        // 用户把归档位置改到别处后，桌面上旧的默认 DeskOrg 目录不能再被
        // 当作普通文件夹整理一次。
        if is_dir && name.eq_ignore_ascii_case(TARGET_FOLDER_NAME) {
            continue;
        }
        let category = if is_dir {
            Category::Folder
        } else {
            Category::from_extension(
                path.extension()
                    .map(|ext| ext.to_string_lossy().as_ref().to_owned())
                    .unwrap_or_default()
                    .as_str(),
            )
        };

        result.items.push(DesktopItem {
            path,
            name,
            category,
            is_dir,
            size: if is_dir { 0 } else { metadata.len() },
            modified: metadata.modified().ok(),
            origin,
        });
    }
}

/// Move every supplied item into `<target>/<category>/`.
#[allow(dead_code)] // 保留给单元测试和不关心中间进度的调用方
pub fn organize(items: &[DesktopItem], target: &Path) -> OrganizeReport {
    let cancel = AtomicBool::new(false);
    organize_with(items, target, &mut |_| {}, &cancel)
}

/// Same as [`organize`], but calls `on_moved` after every successful move so
/// the caller can persist an undo journal incrementally. `cancel` is checked
/// before each item; the item currently being moved is allowed to finish.
pub fn organize_with<F>(
    items: &[DesktopItem],
    target: &Path,
    on_moved: &mut F,
    cancel: &AtomicBool,
) -> OrganizeReport
where
    F: FnMut(&MoveRecord),
{
    let mut report = OrganizeReport::default();

    for item in items {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        if !item.path.exists() {
            report.failed.push(FailedMove {
                path: item.path.clone(),
                message: "文件已不存在".to_owned(),
            });
            continue;
        }
        if is_inside(&item.path, target) {
            continue;
        }

        let category_dir = target.join(item.category.folder_name());
        if let Err(err) = fs::create_dir_all(&category_dir) {
            report.failed.push(FailedMove {
                path: item.path.clone(),
                message: format!("无法创建分类文件夹：{err}"),
            });
            continue;
        }

        let Some(file_name) = item.path.file_name() else {
            continue;
        };
        let desired = category_dir.join(file_name);

        match move_to_unique(&item.path, &desired, item.is_dir) {
            Ok(destination) => {
                let record = MoveRecord {
                    from: item.path.clone(),
                    to: destination,
                    category: item.category,
                    is_dir: item.is_dir,
                };
                on_moved(&record);
                report.moved.push(record);
            }
            Err(err) => report.failed.push(FailedMove {
                path: item.path.clone(),
                message: friendly_move_error(&err),
            }),
        }
    }

    if cancel.load(Ordering::Relaxed) {
        report.cancelled = true;
    }
    report
}

/// Undo a previous organize operation, newest move first.
#[allow(dead_code)] // 保留给单元测试和不关心中间进度的调用方
pub fn undo(moves: &[MoveRecord]) -> UndoReport {
    let cancel = AtomicBool::new(false);
    undo_with(moves, &mut |_| {}, &cancel)
}

/// Same as [`undo`], but calls `on_restored` after every successful restore.
pub fn undo_with<F>(moves: &[MoveRecord], on_restored: &mut F, cancel: &AtomicBool) -> UndoReport
where
    F: FnMut(&MoveRecord),
{
    let mut report = UndoReport::default();

    for record in moves.iter().rev() {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        if !record.to.exists() {
            report.failed.push(FailedMove {
                path: record.to.clone(),
                message: "原文件已不存在，可能已被移动或删除".to_owned(),
            });
            continue;
        }

        match move_to_unique(&record.to, &record.from, record.is_dir) {
            Ok(destination) => {
                // 回调传原始记录，调用方按 from 从撤销日志里移除；
                // 报告里记录实际恢复到的路径（可能因同名而加了序号）。
                on_restored(record);
                let mut restored = record.clone();
                restored.from = destination;
                report.restored.push(restored);
            }
            Err(err) => report.failed.push(FailedMove {
                path: record.to.clone(),
                message: friendly_move_error(&err),
            }),
        }
    }

    if cancel.load(Ordering::Relaxed) {
        report.cancelled = true;
    }
    report
}

pub fn count_by_category(items: &[DesktopItem]) -> BTreeMap<Category, usize> {
    let mut counts = BTreeMap::new();
    for item in items {
        *counts.entry(item.category).or_insert(0) += 1;
    }
    counts
}

pub fn total_size(items: &[DesktopItem]) -> u64 {
    items.iter().map(|item| item.size).sum()
}

pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.2} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes / KB)
    } else {
        format!("{:.0} B", bytes)
    }
}

pub fn make_unique_path(path: &Path, is_dir: bool) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "item".to_owned());

    let (stem, extension) = if is_dir {
        (name.clone(), None)
    } else {
        (
            path.file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_else(|| name.clone()),
            path.extension()
                .map(|ext| ext.to_string_lossy().to_string()),
        )
    };

    for index in 2..10_000 {
        let candidate_name = match &extension {
            Some(extension) if !extension.is_empty() => {
                format!("{stem} ({index}).{extension}")
            }
            _ => format!("{stem} ({index})"),
        };
        let candidate = parent.join(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
    }

    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    parent.join(format!("{stem} ({}-{nonce})", std::process::id()))
}

/// Move `source` to the first free name derived from `desired`.
///
/// `MoveFileExW` is called without `MOVEFILE_REPLACE_EXISTING`, so a race with
/// another process can never overwrite an existing file: on collision the name
/// is bumped and the move is retried.
fn move_to_unique(source: &Path, desired: &Path, is_dir: bool) -> io::Result<PathBuf> {
    let mut candidate = make_unique_path(desired, is_dir);
    for _ in 0..8 {
        match move_path(source, &candidate, is_dir) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                candidate = make_unique_path(desired, is_dir);
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "无法找到不重复的目标文件名",
    ))
}

fn move_path(source: &Path, destination: &Path, is_dir: bool) -> io::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }

    match rename_no_replace(source, destination) {
        Ok(()) => Ok(()),
        Err(first_error) if !is_dir && is_cross_device(&first_error) => {
            // Cross-volume moves are uncommon for the desktop, but a custom
            // target directory may live on another drive. Reserve the target
            // with create_new so the copy also cannot overwrite.
            copy_new(source, destination)?;
            match fs::remove_file(source) {
                Ok(()) => Ok(()),
                Err(error) => {
                    let _ = fs::remove_file(destination);
                    Err(error)
                }
            }
        }
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    let source: Vec<u16> = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // 不传 MOVEFILE_REPLACE_EXISTING：目标已存在时返回 ERROR_ALREADY_EXISTS，
    // 调用方会换一个带序号的名字重试，绝不覆盖。
    let result = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) };
    if result != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(windows))]
fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    if destination.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "目标已存在",
        ));
    }
    fs::rename(source, destination)
}

fn copy_new(source: &Path, destination: &Path) -> io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    match fs::copy(source, destination) {
        Ok(_) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(destination);
            Err(error)
        }
    }
}

#[cfg(windows)]
fn is_cross_device(error: &io::Error) -> bool {
    // ERROR_NOT_SAME_DEVICE
    error.raw_os_error() == Some(17)
}

#[cfg(not(windows))]
fn is_cross_device(error: &io::Error) -> bool {
    // EXDEV
    error.raw_os_error() == Some(18)
}

fn friendly_move_error(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::PermissionDenied => {
            "没有权限移动该项目（公共桌面可能需要管理员权限）".to_owned()
        }
        _ => format!("移动失败：{error}"),
    }
}

fn is_ignored_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "desktop.ini" | "thumbs.db" | ".ds_store"
    )
}

#[cfg(windows)]
fn is_hidden_or_system(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
    metadata.file_attributes() & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0
}

#[cfg(not(windows))]
fn is_hidden_or_system(_metadata: &fs::Metadata) -> bool {
    false
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

pub fn is_inside(path: &Path, root: &Path) -> bool {
    let path = path.to_string_lossy().to_lowercase();
    let root = root
        .to_string_lossy()
        .to_lowercase()
        .trim_end_matches(['\\', '/'])
        .to_owned();
    let backslash_prefix = format!("{root}\\");
    let slash_prefix = format!("{root}/");
    path == root || path.starts_with(&backslash_prefix) || path.starts_with(&slash_prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let unique = format!(
            "deskorg-test-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp");
        let path = base.join(unique);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn classifies_common_extensions() {
        assert_eq!(Category::from_extension("PNG"), Category::Image);
        assert_eq!(Category::from_extension(".pdf"), Category::Document);
        assert_eq!(Category::from_extension("rs"), Category::Code);
        assert_eq!(Category::from_extension("lnk"), Category::Shortcut);
        assert_eq!(Category::from_extension("7z"), Category::Archive);
        assert_eq!(Category::from_extension("exe"), Category::Installer);
        assert_eq!(Category::from_extension("unknown"), Category::Other);
    }

    #[test]
    fn unique_path_keeps_extension() {
        let dir = temp_dir("unique");
        fs::write(dir.join("photo.jpg"), b"a").unwrap();
        let path = make_unique_path(&dir.join("photo.jpg"), false);
        assert_eq!(path.file_name().unwrap().to_string_lossy(), "photo (2).jpg");
        fs::write(&path, b"b").unwrap();
        let path = make_unique_path(&dir.join("photo.jpg"), false);
        assert_eq!(path.file_name().unwrap().to_string_lossy(), "photo (3).jpg");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn organize_and_undo_round_trip() {
        let desktop = temp_dir("desktop");
        let target = desktop.join(TARGET_FOLDER_NAME);
        let image = desktop.join("sunset.png");
        let doc = desktop.join("notes.md");
        let folder = desktop.join("project");
        fs::write(&image, b"png").unwrap();
        fs::write(&doc, b"md").unwrap();
        fs::create_dir(&folder).unwrap();
        fs::write(desktop.join("desktop.ini"), b"ignored").unwrap();

        let mut result = ScanResult::default();
        scan_root(&desktop, &target, DesktopOrigin::User, &mut result);
        assert_eq!(result.items.len(), 3);

        let report = organize(&result.items, &target);
        assert_eq!(report.moved.len(), 3);
        assert!(report.failed.is_empty());
        assert!(target.join("图片").join("sunset.png").exists());
        assert!(target.join("文档").join("notes.md").exists());
        assert!(target.join("文件夹").join("project").exists());
        assert!(!image.exists());

        let undo_report = undo(&report.moved);
        assert_eq!(undo_report.restored.len(), 3);
        assert!(undo_report.failed.is_empty());
        assert!(image.exists());
        assert!(doc.exists());
        assert!(folder.exists());

        fs::remove_dir_all(desktop).unwrap();
    }

    #[test]
    fn second_scan_skips_target_folder() {
        let desktop = temp_dir("skip-target");
        let target = desktop.join(TARGET_FOLDER_NAME);
        fs::create_dir_all(target.join("图片")).unwrap();
        fs::write(desktop.join("a.png"), b"png").unwrap();
        fs::write(target.join("图片").join("b.png"), b"png").unwrap();

        let mut result = ScanResult::default();
        scan_root(&desktop, &target, DesktopOrigin::User, &mut result);
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].name, "a.png");

        fs::remove_dir_all(desktop).unwrap();
    }

    #[test]
    fn detects_nested_target_paths() {
        assert!(is_inside(
            Path::new(r"D:\Desktop\DeskOrg\图片\a.png"),
            Path::new(r"D:\Desktop\DeskOrg")
        ));
        assert!(!is_inside(
            Path::new(r"D:\Desktop\DeskOrg-backup\a.png"),
            Path::new(r"D:\Desktop\DeskOrg")
        ));
    }

    #[test]
    fn custom_target_skips_old_default_archive() {
        let desktop = temp_dir("custom-target");
        let old_archive = desktop.join(TARGET_FOLDER_NAME);
        let target = desktop.join("Archive");
        fs::create_dir_all(old_archive.join("图片")).unwrap();
        fs::write(old_archive.join("图片").join("old.png"), b"png").unwrap();
        fs::write(desktop.join("new.png"), b"png").unwrap();

        let mut result = ScanResult::default();
        scan_root(&desktop, &target, DesktopOrigin::User, &mut result);

        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].name, "new.png");
        fs::remove_dir_all(desktop).unwrap();
    }

    #[test]
    fn organize_with_reports_each_successful_move() {
        let desktop = temp_dir("organize-with");
        let target = desktop.join(TARGET_FOLDER_NAME);
        fs::write(desktop.join("a.png"), b"a").unwrap();
        fs::write(desktop.join("b.md"), b"b").unwrap();

        let mut result = ScanResult::default();
        scan_root(&desktop, &target, DesktopOrigin::User, &mut result);

        let mut seen = Vec::new();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let report = organize_with(
            &result.items,
            &target,
            &mut |record| {
                seen.push(record.to.clone());
                assert!(record.to.exists());
            },
            &cancel,
        );

        assert_eq!(report.moved.len(), 2);
        assert_eq!(seen.len(), 2);
        assert!(!report.cancelled);
        fs::remove_dir_all(desktop).unwrap();
    }

    #[test]
    fn undo_with_reports_each_restored_item() {
        let desktop = temp_dir("undo-with");
        let target = desktop.join(TARGET_FOLDER_NAME);
        fs::write(desktop.join("a.png"), b"a").unwrap();

        let mut result = ScanResult::default();
        scan_root(&desktop, &target, DesktopOrigin::User, &mut result);
        let organized = organize(&result.items, &target);

        let mut seen = Vec::new();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let undo_report = undo_with(
            &organized.moved,
            &mut |record| {
                seen.push(record.from.clone());
                assert!(record.from.exists());
            },
            &cancel,
        );

        assert_eq!(undo_report.restored.len(), 1);
        assert_eq!(seen.len(), 1);
        assert!(!undo_report.cancelled);
        fs::remove_dir_all(desktop).unwrap();
    }

    #[test]
    fn organize_with_stops_when_cancelled() {
        let desktop = temp_dir("organize-cancel");
        let target = desktop.join(TARGET_FOLDER_NAME);
        fs::write(desktop.join("a.png"), b"a").unwrap();
        fs::write(desktop.join("b.png"), b"b").unwrap();

        let mut result = ScanResult::default();
        scan_root(&desktop, &target, DesktopOrigin::User, &mut result);

        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut moved = 0;
        let report = organize_with(
            &result.items,
            &target,
            &mut |_| {
                moved += 1;
                cancel.store(true, Ordering::Relaxed);
            },
            &cancel,
        );

        assert!(report.cancelled);
        assert_eq!(report.moved.len(), 1);
        assert_eq!(moved, 1);
        let remaining = fs::read_dir(&desktop)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".png"))
            .count();
        assert_eq!(remaining, 1);
        fs::remove_dir_all(desktop).unwrap();
    }

    #[test]
    fn undo_with_stops_when_cancelled() {
        let desktop = temp_dir("undo-cancel");
        let target = desktop.join(TARGET_FOLDER_NAME);
        fs::write(desktop.join("a.png"), b"a").unwrap();
        fs::write(desktop.join("b.png"), b"b").unwrap();

        let mut result = ScanResult::default();
        scan_root(&desktop, &target, DesktopOrigin::User, &mut result);
        let organized = organize(&result.items, &target);

        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut restored = 0;
        let report = undo_with(
            &organized.moved,
            &mut |_| {
                restored += 1;
                cancel.store(true, Ordering::Relaxed);
            },
            &cancel,
        );

        assert!(report.cancelled);
        assert_eq!(report.restored.len(), 1);
        assert_eq!(restored, 1);
        let archived = fs::read_dir(target.join(Category::Image.folder_name()))
            .unwrap()
            .count();
        assert_eq!(archived, 1);
        fs::remove_dir_all(desktop).unwrap();
    }

    #[test]
    fn move_to_unique_does_not_overwrite_existing_file() {
        let root = temp_dir("no-overwrite-file");
        let source = root.join("source.txt");
        let target_dir = root.join("target");
        fs::create_dir_all(&target_dir).unwrap();
        let destination = target_dir.join("source.txt");
        fs::write(&source, b"source").unwrap();
        fs::write(&destination, b"existing").unwrap();

        let moved = move_to_unique(&source, &destination, false).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"existing");
        assert_eq!(fs::read(&moved).unwrap(), b"source");
        assert_eq!(
            moved.file_name().unwrap().to_string_lossy(),
            "source (2).txt"
        );
        assert!(!source.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn move_to_unique_does_not_merge_existing_directory() {
        let root = temp_dir("no-overwrite-dir");
        let source = root.join("photos");
        let target_dir = root.join("target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(source.join("new.jpg"), b"new").unwrap();
        let destination = target_dir.join("photos");
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join("old.jpg"), b"old").unwrap();

        let moved = move_to_unique(&source, &destination, true).unwrap();

        assert_eq!(fs::read(destination.join("old.jpg")).unwrap(), b"old");
        assert_eq!(fs::read(moved.join("new.jpg")).unwrap(), b"new");
        assert!(!source.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
