//! The DeskOrg application window.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui::{
    self, Align, Align2, Color32, FontId, Id, Layout, RichText, Sense, Stroke, Ui, Vec2,
};

use crate::config::{Config, UndoState};
use crate::desktop::{
    self, Category, DesktopItem, DesktopOrigin, MoveRecord, OrganizeReport, ScanResult, UndoReport,
};
use crate::fences::{self, Fence, FenceAction, FenceData, FenceTheme};
use crate::shell::{self, ShellEvent};
use crate::theme::{self, with_alpha, Palette};
use crate::widgets;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Overview,
    Organize,
    Fences,
    Beauty,
    Settings,
}

impl Page {
    const ALL: [(Self, &'static str, &'static str); 5] = [
        (Self::Overview, "总", "总览"),
        (Self::Organize, "整", "桌面整理"),
        (Self::Fences, "框", "桌面框框"),
        (Self::Beauty, "美", "美化"),
        (Self::Settings, "设", "设置"),
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Overview => "总览",
            Self::Organize => "桌面整理",
            Self::Fences => "桌面框框",
            Self::Beauty => "美化",
            Self::Settings => "设置",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::Overview => "让桌面，只留下重要的事。",
            Self::Organize => "按类型归位，随时可撤销。",
            Self::Fences => "把分类装进透明盒子里。",
            Self::Beauty => "壁纸、配色与清爽模式。",
            Self::Settings => "整理范围、手势与归档位置。",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Confirm {
    Organize { count: usize },
    Undo { count: usize },
}

#[derive(Clone, Copy, Debug)]
enum NoticeKind {
    Success,
    Info,
    Error,
}

struct Notice {
    text: String,
    kind: NoticeKind,
    born: Instant,
}

struct JobState {
    rx: Receiver<JobOutcome>,
    label: String,
    progress: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    total: usize,
    started: Instant,
}

struct ScanState {
    rx: Receiver<ScanSnapshot>,
    target: PathBuf,
    include_public: bool,
    started: Instant,
}

struct ScanSnapshot {
    scan: ScanResult,
    fence_data: HashMap<Category, FenceData>,
}

enum JobOutcome {
    Organize {
        report: OrganizeReport,
        skipped_shortcuts: usize,
        make_fences: bool,
        created_at: u64,
        journal_error: Option<String>,
    },
    Undo {
        report: UndoReport,
        remaining: Vec<MoveRecord>,
        created_at: u64,
        journal_error: Option<String>,
    },
}

pub struct DeskOrgApp {
    config: Config,
    page: Page,
    scan: ScanResult,
    recent: Vec<DesktopItem>,
    selected: HashSet<PathBuf>,
    undo: Option<UndoState>,
    events: Receiver<ShellEvent>,
    icons_visible: bool,
    fences_visible: bool,
    fences_were_showing: bool,
    force_quit: bool,
    start_background: bool,
    hidden_by_app: bool,
    last_icon_poll: Instant,
    last_scan: Option<Instant>,
    search: String,
    category_filter: Option<Category>,
    confirm: Option<Confirm>,
    confirm_archive: bool,
    confirm_make_fences: bool,
    notice: Option<Notice>,
    wallpaper: Option<egui::TextureHandle>,
    wallpaper_job: Option<Receiver<(String, Result<egui::TextureHandle, String>)>>,
    logo: Option<egui::TextureHandle>,
    wallpaper_error: Option<String>,
    applied_window_theme: Option<bool>,
    fence_data: HashMap<Category, FenceData>,
    icon_cache: HashMap<PathBuf, Option<egui::TextureHandle>>,
    icon_retry: HashMap<PathBuf, Instant>,
    frost: Option<egui::TextureHandle>,
    frost_job: Option<Receiver<(String, Result<egui::TextureHandle, String>)>>,
    frost_source: String,
    desktop_origin: Vec2,
    desktop_size: Vec2,
    last_fence_refresh: Instant,
    config_dirty: bool,
    fence_viewports_created: HashSet<u64>,
    last_config_save: Instant,
    new_fence_open: bool,
    new_fence_category: Category,
    new_fence_theme: FenceTheme,
    hide_frames: u8,
    job: Option<JobState>,
    scan_job: Option<ScanState>,
    tray_available: bool,
}

impl DeskOrgApp {
    pub fn new(cc: &eframe::CreationContext<'_>, start_background: bool) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let (config, config_warning) = Config::load_checked();
        theme::apply_style(&cc.egui_ctx, config.light_theme, config.accent);
        if config.auto_start {
            // Refresh the executable path and keep `--background` in sync.
            let _ = shell::set_auto_start(true);
        }

        let target = config.resolved_target();
        let scan = desktop::scan(config.include_public, &target);
        let recent = recent_items(&scan.items);
        let selected = scan.items.iter().map(|item| item.path.clone()).collect();
        let (undo, undo_warning) = UndoState::load_checked();

        let mut icons_visible = shell::desktop_icons_visible().unwrap_or(true);
        let mut hidden_by_app = false;
        let fences_requested = config.fences_enabled && !config.fences.is_empty();
        let mut fences_visible = fences_requested;
        let mut startup_notice: Option<(String, NoticeKind)> = None;

        // 启动时只有真的把原生图标藏起来，才允许把主窗口收进托盘。
        // 否则框框会叠在原生图标上，而且用户看不到任何窗口。
        if fences_requested && icons_visible {
            match shell::set_desktop_icons_visible(false) {
                Ok(false) => {
                    icons_visible = false;
                    hidden_by_app = true;
                }
                Ok(true) => {
                    fences_visible = false;
                    startup_notice = Some((
                        "启动时 Windows 没有隐藏桌面图标，框框暂时未显示；可稍后在「桌面框框」页重试"
                            .to_owned(),
                        NoticeKind::Error,
                    ));
                }
                Err(error) => {
                    fences_visible = false;
                    startup_notice = Some((
                        format!(
                            "启动时无法隐藏桌面图标，框框暂时未显示（{error}）；可稍后重试"
                        ),
                        NoticeKind::Error,
                    ));
                }
            }
        }

        // 壁纸预览和毛玻璃纹理都在后台加载，避免启动时解码大图卡住窗口。
        let frost_source = String::new();
        // GetSystemMetrics 返回物理像素；egui 的 viewport 坐标是逻辑点。
        let scale = cc.egui_ctx.pixels_per_point().max(0.1);
        let (desktop_origin, desktop_size) = shell::virtual_desktop_rect()
            .map(|(x, y, width, height)| {
                (
                    Vec2::new(x / scale, y / scale),
                    Vec2::new(width / scale, height / scale),
                )
            })
            .unwrap_or_else(|| {
                let size = cc
                    .egui_ctx
                    .input(|input| input.viewport().monitor_size)
                    .unwrap_or(Vec2::new(1920.0, 1080.0));
                (Vec2::ZERO, size)
            });

        let mut fence_data = HashMap::new();
        for category in Category::ALL {
            fence_data.insert(
                category,
                fences::collect_data(&scan.items, &target, category),
            );
        }

        let logo = image::load_from_memory(include_bytes!("../assets/icon.png"))
            .ok()
            .map(|image| {
                let image = image.to_rgba8();
                let size = [image.width() as usize, image.height() as usize];
                cc.egui_ctx.load_texture(
                    "deskorg-logo",
                    egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
                    egui::TextureOptions::LINEAR,
                )
            });
        let (tx, events) = channel();
        let watcher_error = shell::start_watchers(tx, cc.egui_ctx.clone()).err();
        let tray_result = shell::start_tray();
        let tray_available = tray_result.is_ok();
        let tray_error = tray_result.err();
        // 没有托盘就没有恢复入口。保留用户的后台运行设置，但本次会话
        // 由 tray_available 拦住所有自动隐藏逻辑。
        shell::set_double_click_enabled(config.desktop_double_click);
        // 没有托盘就没有恢复入口，任何情况下都不要自动隐藏主窗口。
        let hide_frames = if tray_available && (start_background || fences_visible) {
            12
        } else {
            0
        };

        let mut app = Self {
            config,
            page: Page::Overview,
            scan,
            recent,
            selected,
            undo,
            events,
            icons_visible,
            fences_visible,
            fences_were_showing: false,
            force_quit: false,
            start_background,
            hidden_by_app,
            last_icon_poll: Instant::now(),
            last_scan: Some(Instant::now()),
            search: String::new(),
            category_filter: None,
            confirm: None,
            confirm_archive: true,
            confirm_make_fences: true,
            notice: None,
            wallpaper: None,
            wallpaper_job: None,
            logo,
            wallpaper_error: None,
            applied_window_theme: None,
            fence_data,
            icon_cache: HashMap::new(),
            icon_retry: HashMap::new(),
            frost: None,
            frost_job: None,
            frost_source,
            desktop_origin,
            desktop_size,
            last_fence_refresh: Instant::now(),
            config_dirty: false,
            fence_viewports_created: HashSet::new(),
            last_config_save: Instant::now(),
            new_fence_open: false,
            new_fence_category: Category::Image,
            new_fence_theme: FenceTheme::Mint,
            hide_frames,
            job: None,
            scan_job: None,
            tray_available,
        };

        let mut startup_errors = Vec::new();
        if let Some(warning) = config_warning {
            startup_errors.push(warning);
        }
        if let Some(warning) = undo_warning {
            startup_errors.push(warning);
        }
        if let Some(error) = watcher_error {
            startup_errors.push(error);
        }
        if !tray_available {
            shell::stop_tray();
            startup_errors.push(format!(
                "托盘图标不可用，本次不会隐藏到后台{}",
                tray_error
                    .map(|error| format!("：{error}"))
                    .unwrap_or_default()
            ));
        }
        if let Some((text, _kind)) = startup_notice {
            startup_errors.push(text);
        }
        if !startup_errors.is_empty() {
            app.notify(startup_errors.join("；"), NoticeKind::Error);
        }
        app
    }

    fn palette(&self) -> Palette {
        theme::palette(self.config.light_theme, self.config.accent)
    }

    fn notify(&mut self, text: impl Into<String>, kind: NoticeKind) {
        let text = text.into();
        // 主窗口可能已经收进托盘，错误信息再用系统气泡提醒一次。
        if matches!(kind, NoticeKind::Error) && self.tray_available {
            shell::show_tray_balloon("DeskOrg", &text, true);
        }
        self.notice = Some(Notice {
            text,
            kind,
            born: Instant::now(),
        });
    }

    fn save_config(&mut self) {
        match self.config.save() {
            Ok(()) => self.last_config_save = Instant::now(),
            Err(error) => self.notify(format!("设置保存失败：{error}"), NoticeKind::Error),
        }
    }

    fn flush_config_if_due(&mut self) {
        if !self.config_dirty || self.last_config_save.elapsed() < Duration::from_millis(700) {
            return;
        }
        if let Err(error) = self.config.save() {
            self.notify(format!("设置保存失败：{error}"), NoticeKind::Error);
        }
        self.config_dirty = false;
        self.last_config_save = Instant::now();
    }

    fn target(&self) -> PathBuf {
        self.config.resolved_target()
    }

    fn rescan(&mut self, preserve_selection: bool) {
        // 同步扫描会覆盖状态，丢弃可能在飞行中的后台扫描结果。
        self.scan_job = None;
        let target = self.target();
        let scan = desktop::scan(self.config.include_public, &target);
        let available: HashSet<PathBuf> = scan.items.iter().map(|item| item.path.clone()).collect();
        if preserve_selection {
            self.selected.retain(|path| available.contains(path));
        } else {
            self.selected = available;
        }
        self.scan = scan;
        self.recent = recent_items(&self.scan.items);
        self.last_scan = Some(Instant::now());
        self.refresh_fence_data();
    }

    fn start_background_scan(&mut self) {
        if self.scan_job.is_some()
            || self.job.is_some()
            || self.confirm.is_some()
            || self.new_fence_open
        {
            return;
        }
        let target = self.target();
        let include_public = self.config.include_public;
        let worker_target = target.clone();
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("deskorg-scan".to_owned())
            .spawn(move || {
                let scan = desktop::scan(include_public, &worker_target);
                let mut fence_data = HashMap::new();
                for category in Category::ALL {
                    fence_data.insert(
                        category,
                        fences::collect_data(&scan.items, &worker_target, category),
                    );
                }
                let _ = tx.send(ScanSnapshot { scan, fence_data });
            });
        match spawned {
            Ok(_) => {
                self.scan_job = Some(ScanState {
                    rx,
                    target,
                    include_public,
                    started: Instant::now(),
                });
                self.last_fence_refresh = Instant::now();
            }
            Err(error) => {
                self.last_fence_refresh = Instant::now();
                self.notify(format!("无法启动后台扫描：{error}"), NoticeKind::Error);
            }
        }
    }

    fn poll_scan_job(&mut self) {
        let received = self.scan_job.as_ref().map(|job| job.rx.try_recv());
        match received {
            Some(Ok(snapshot)) => {
                let Some(job) = self.scan_job.take() else {
                    return;
                };
                // 扫描期间用户改了归档位置/扫描范围，结果作废。
                if job.target != self.target() || job.include_public != self.config.include_public {
                    return;
                }
                let available: HashSet<PathBuf> = snapshot
                    .scan
                    .items
                    .iter()
                    .map(|item| item.path.clone())
                    .collect();
                self.selected.retain(|path| available.contains(path));
                self.scan = snapshot.scan;
                self.recent = recent_items(&self.scan.items);
                self.fence_data = snapshot.fence_data;
                self.last_scan = Some(Instant::now());
                self.last_fence_refresh = Instant::now();
                self.prune_icon_cache();
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.scan_job = None;
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
    }

    fn refresh_fence_data(&mut self) {
        let target = self.target();
        let mut data = HashMap::new();
        for category in Category::ALL {
            data.insert(
                category,
                fences::collect_data(&self.scan.items, &target, category),
            );
        }
        self.fence_data = data;
        self.last_fence_refresh = Instant::now();
        self.prune_icon_cache();
    }

    fn prune_icon_cache(&mut self) {
        // 只保留当前确实有框框的分类的图标；删除框框后纹理能及时释放。
        let used: HashSet<PathBuf> = self
            .config
            .fences
            .iter()
            .filter_map(|fence| self.fence_data.get(&fence.category))
            .flat_map(|data| data.items.iter().map(|item| item.path.clone()))
            .collect();
        self.icon_cache.retain(|path, _| used.contains(path));
        self.icon_retry.retain(|path, _| used.contains(path));
    }

    fn icon_needs_load(&self, path: &PathBuf) -> bool {
        match self.icon_cache.get(path) {
            None => true,
            Some(Some(_)) => false,
            // 临时失败（Explorer 忙、文件被占用）不要永久缓存，60 秒后重试。
            Some(None) => self
                .icon_retry
                .get(path)
                .map(|retry| retry.elapsed() >= Duration::from_secs(60))
                .unwrap_or(true),
        }
    }

    fn ensure_fence_icons(&mut self, ctx: &egui::Context, budget: usize) {
        let mut paths = Vec::new();
        let mut seen = HashSet::new();
        for fence in &self.config.fences {
            if let Some(data) = self.fence_data.get(&fence.category) {
                for item in &data.items {
                    if !self.icon_needs_load(&item.path) {
                        continue;
                    }
                    if seen.insert(item.path.clone()) {
                        paths.push(item.path.clone());
                    }
                }
            }
        }
        let mut loaded = 0;
        let deadline = Instant::now() + Duration::from_millis(4);
        for path in paths {
            // 每帧最多花 4ms 读图标，避免大量文件/网络路径时卡住绘制。
            if loaded >= budget || Instant::now() >= deadline {
                break;
            }
            let texture = shell::file_icon_rgba(&path)
                .ok()
                .map(|(rgba, width, height)| {
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [width as usize, height as usize],
                        &rgba,
                    );
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    use std::hash::{Hash, Hasher};
                    path.hash(&mut hasher);
                    ctx.load_texture(
                        format!("deskorg-icon-{:x}", hasher.finish()),
                        image,
                        egui::TextureOptions::LINEAR,
                    )
                });
            if texture.is_some() {
                self.icon_retry.remove(&path);
            } else {
                self.icon_retry.insert(path.clone(), Instant::now());
            }
            self.icon_cache.insert(path, texture);
            loaded += 1;
        }
        if loaded > 0 {
            ctx.request_repaint();
        }
    }

    fn reload_frost(&mut self, ctx: &egui::Context) {
        // 没有框框开启毛玻璃时不加载壁纸纹理，省掉一次大图解码和纹理。
        if !self.config.fences.iter().any(|fence| fence.blur) {
            self.frost = None;
            self.frost_job = None;
            self.frost_source.clear();
            return;
        }
        let source = shell::current_wallpaper()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default();
        if source == self.frost_source {
            return;
        }
        self.frost_source = source.clone();
        if source.trim().is_empty() {
            self.frost = None;
            self.frost_job = None;
            return;
        }

        // 壁纸解码可能很慢，放到后台线程；旧纹理在新纹理完成前继续使用。
        let worker_source = source.clone();
        let worker_ctx = ctx.clone();
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("deskorg-frost".to_owned())
            .spawn(move || {
                let result = load_frost_texture(&worker_ctx, &worker_source);
                let _ = tx.send((worker_source, result));
            });
        match spawned {
            Ok(_) => self.frost_job = Some(rx),
            Err(_) => self.frost = None,
        }
    }

    fn poll_frost_job(&mut self) {
        let received = self.frost_job.as_ref().map(|job| job.try_recv());
        match received {
            Some(Ok((source, result))) => {
                self.frost_job = None;
                if source != self.frost_source {
                    return;
                }
                self.frost = result.ok();
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.frost_job = None;
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
    }

    fn next_fence_id(&self) -> u64 {
        self.config
            .fences
            .iter()
            .map(|fence| fence.id)
            .max()
            .unwrap_or(0)
            + 1
    }

    fn fence_count(&self, category: Category) -> usize {
        self.fence_data
            .get(&category)
            .map(FenceData::len)
            .unwrap_or(0)
    }

    /// 找一个不与现有框框重叠的自动布局位置。删除中间框框后，
    /// 单纯用 fences.len() 当索引会和已有的框框重合。
    fn next_fence_position(&self) -> (f32, f32) {
        let count = self.config.fences.len();
        for index in 0..=count {
            let (x, y) = fences::auto_position(index, self.desktop_size, self.desktop_origin);
            let candidate = egui::Rect::from_min_size(
                egui::pos2(x, y),
                Vec2::new(fences::FENCE_WIDTH, fences::FENCE_HEIGHT),
            );
            let overlaps = self.config.fences.iter().any(|fence| {
                candidate.intersects(egui::Rect::from_min_size(
                    egui::pos2(fence.x, fence.y),
                    Vec2::new(fence.w, fence.h),
                ))
            });
            if !overlaps {
                return (x, y);
            }
        }
        fences::auto_position(count, self.desktop_size, self.desktop_origin)
    }

    fn ensure_fences_for_populated_categories(&mut self) -> usize {
        let categories: Vec<Category> = Category::ALL
            .into_iter()
            .filter(|category| self.fence_count(*category) > 0)
            .collect();
        let mut created = 0;
        for category in categories {
            if self
                .config
                .fences
                .iter()
                .any(|fence| fence.category == category)
            {
                continue;
            }
            let id = self.next_fence_id();
            let index = self.config.fences.len();
            let mut fence = Fence::new(
                id,
                category,
                index,
                self.desktop_size,
                self.desktop_origin,
            );
            let (x, y) = self.next_fence_position();
            fence.x = x;
            fence.y = y;
            self.config.fences.push(fence);
            created += 1;
        }
        if created > 0 {
            self.config_dirty = true;
        }
        created
    }

    fn enable_fences(&mut self, ctx: &egui::Context) {
        if self.config.fences.is_empty() {
            self.notify("还没有框框，先生成一个吧", NoticeKind::Info);
            return;
        }
        self.config.fences_enabled = true;
        let actually_visible = shell::desktop_icons_visible().unwrap_or(self.icons_visible);
        let mut success = true;
        if actually_visible {
            match shell::set_desktop_icons_visible(false) {
                Ok(false) => {
                    self.icons_visible = false;
                    self.hidden_by_app = true;
                    self.last_icon_poll = Instant::now();
                }
                Ok(true) => {
                    success = false;
                    self.notify(
                        "Windows 没有隐藏桌面图标，框框暂时无法显示",
                        NoticeKind::Error,
                    );
                }
                Err(error) => {
                    success = false;
                    self.notify(error, NoticeKind::Error);
                }
            }
        }
        self.fences_visible = success;
        if !success {
            self.config.fences_enabled = false;
        }
        self.save_config();
        if success && self.tray_available {
            // Leave only the tray icon; hiding the native root window also
            // removes its taskbar button while keeping the egui event loop alive.
            self.hide_frames = 12;
            shell::hide_main_window();
        }
        ctx.request_repaint();
    }

    fn disable_fences(&mut self, ctx: &egui::Context) {
        self.config.fences_enabled = false;
        self.fences_visible = false;
        // 只恢复 DeskOrg 自己隐藏的图标；用户原本手动隐藏的状态要保留。
        if self.hidden_by_app {
            if let Ok(true) = shell::set_desktop_icons_visible(true) {
                self.icons_visible = true;
                self.hidden_by_app = false;
                self.last_icon_poll = Instant::now();
            }
        }
        self.save_config();
        ctx.request_repaint();
    }

    fn delete_fence(&mut self, id: u64, ctx: &egui::Context) {
        self.config.fences.retain(|fence| fence.id != id);
        self.fence_viewports_created.remove(&id);
        self.prune_icon_cache();
        // The id is gone for good, so closing this viewport is safe.
        ctx.send_viewport_cmd_to(fence_viewport_id(id), egui::ViewportCommand::Close);
        if self.config.fences.is_empty() {
            self.config.fences_enabled = false;
            self.fences_visible = false;
            if self.hidden_by_app {
                if let Ok(true) = shell::set_desktop_icons_visible(true) {
                    self.icons_visible = true;
                    self.hidden_by_app = false;
                }
            }
        }
        let _ = self.config.save();
        ctx.request_repaint();
    }

    fn select_all(&mut self) {
        self.selected = self
            .scan
            .items
            .iter()
            .map(|item| item.path.clone())
            .collect();
    }

    fn clear_selection(&mut self) {
        self.selected.clear();
    }

    fn selected_items(&self) -> Vec<DesktopItem> {
        self.scan
            .items
            .iter()
            .filter(|item| self.selected.contains(&item.path))
            .cloned()
            .collect()
    }

    fn toggle_icons(&mut self) {
        if let Err(error) = self.toggle_icons_inner() {
            self.notify(error, NoticeKind::Error);
        }
    }

    fn toggle_fences_from_tray(&mut self, ctx: &egui::Context) {
        if self.config.fences.is_empty() {
            self.notify("还没有桌面框框，请先到「桌面框框」页生成", NoticeKind::Info);
            return;
        }
        if self.config.fences_enabled {
            self.toggle_icons();
        } else {
            self.enable_fences(ctx);
        }
    }

    fn toggle_icons_inner(&mut self) -> Result<(), String> {
        // While fence mode is active, double-click toggles every fence together.
        // Native desktop icons stay hidden so the two layers never alternate.
        if self.config.fences_enabled && !self.config.fences.is_empty() {
            let show = !self.fences_visible;
            if show && shell::desktop_icons_visible() == Some(true) {
                if shell::set_desktop_icons_visible(false)? {
                    return Err("Windows 没有隐藏桌面图标".to_owned());
                }
                self.icons_visible = false;
                self.hidden_by_app = true;
            }
            self.fences_visible = show;
            self.last_icon_poll = Instant::now();
            if show {
                self.notify("所有框框已显示", NoticeKind::Success);
            } else {
                self.notify("所有框框已隐藏 · 再双击桌面恢复", NoticeKind::Info);
            }
            return Ok(());
        }

        let current =
            shell::desktop_icons_visible().ok_or_else(|| "找不到桌面图标窗口".to_owned())?;
        let visible = shell::set_desktop_icons_visible(!current)?;
        self.icons_visible = visible;
        self.hidden_by_app = !visible;
        self.last_icon_poll = Instant::now();
        if visible {
            self.notify("桌面图标回来啦", NoticeKind::Success);
        } else {
            self.notify("桌面图标已隐藏 · 只剩你和壁纸", NoticeKind::Info);
        }
        Ok(())
    }

    fn request_organize(&mut self, archive_default: bool) {
        if self.job.is_some() {
            self.notify("后台任务还在处理，请稍候", NoticeKind::Info);
            return;
        }
        let count = self.selected.len();
        if count == 0 {
            self.notify("先选择要整理的文件，或点「全选」", NoticeKind::Info);
            return;
        }
        // 弹窗打开期间不要再用后台扫描结果改动选择。
        self.scan_job = None;
        self.confirm_archive = archive_default;
        self.confirm_make_fences = self.config.fences_after_organize;
        self.confirm = Some(Confirm::Organize { count });
    }

    fn perform_organize(&mut self, ctx: &egui::Context, do_archive: bool) {
        if self.job.is_some() {
            self.notify("后台任务还在处理，请稍候", NoticeKind::Info);
            return;
        }
        self.scan_job = None;
        let make_fences = self.config.fences_after_organize;
        let selected = self.selected_items();
        let skipped_shortcuts = if do_archive {
            selected
                .iter()
                .filter(|item| item.category == Category::Shortcut)
                .count()
        } else {
            0
        };
        let items: Vec<_> = if do_archive {
            selected
                .into_iter()
                .filter(|item| item.category != Category::Shortcut)
                .collect()
        } else {
            selected
        };
        if items.is_empty() {
            self.notify(
                if skipped_shortcuts > 0 {
                    "快捷方式会保留在桌面，不放进归档文件夹"
                } else {
                    "没有可整理的项目"
                },
                NoticeKind::Info,
            );
            return;
        }

        // 纯视觉归类路径：不物理移动文件，只在桌面上生成透明框框。
        // 文件保持原位，退出框框模式时图标自然可见。
        if !do_archive {
            if !make_fences {
                self.notify(
                    "请至少选择「生成透明框框」或「同时归档文件」",
                    NoticeKind::Info,
                );
                return;
            }
            let created = self.ensure_fences_for_populated_categories();
            self.enable_fences(ctx);
            self.page = Page::Fences;
            if self.config.fences_enabled {
                self.notify(
                    if created > 0 {
                        format!("已生成 {created} 个透明框框，文件保持原位")
                    } else {
                        "桌面框框已就绪，文件保持原位".to_owned()
                    },
                    NoticeKind::Success,
                );
            }
            return;
        }

        self.spawn_organize_job(items, make_fences, skipped_shortcuts);
    }

    fn request_undo(&mut self) {
        if self.job.is_some() {
            self.notify("后台任务还在处理，请稍候", NoticeKind::Info);
            return;
        }
        if let Some(undo) = &self.undo {
            self.scan_job = None;
            self.confirm = Some(Confirm::Undo {
                count: undo.moves.len(),
            });
        } else {
            self.notify("还没有可以撤销的整理记录", NoticeKind::Info);
        }
    }

    fn perform_undo(&mut self) {
        if self.job.is_some() {
            self.notify("后台任务还在处理，请稍候", NoticeKind::Info);
            return;
        }
        self.scan_job = None;
        let Some(undo) = self.undo.clone() else {
            return;
        };
        self.spawn_undo_job(undo);
    }

    fn open_target(&mut self) {
        let target = self.target();
        let _ = std::fs::create_dir_all(&target);
        if let Err(error) = shell::open_in_explorer(&target) {
            self.notify(error, NoticeKind::Error);
        }
    }

    fn pick_wallpaper(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("选择桌面壁纸")
            .add_filter("图片", &["png", "jpg", "jpeg", "bmp", "webp"])
            .pick_file()
        else {
            return;
        };
        self.config.wallpaper_path = path.to_string_lossy().to_string();
        self.save_config();
        self.wallpaper_error = None;

        // 大图解码放到后台，避免选完壁纸后界面卡住。
        match self.start_wallpaper_preview_load(ctx) {
            Ok(()) => self.notify("正在载入壁纸…", NoticeKind::Info),
            Err(error) => {
                self.wallpaper = None;
                self.wallpaper_error = Some(error.clone());
                self.notify(error, NoticeKind::Error);
            }
        }
    }

    fn start_wallpaper_preview_load(&mut self, ctx: &egui::Context) -> Result<(), String> {
        let path = self.config.wallpaper_path.trim().to_string();
        if path.is_empty() {
            self.wallpaper = None;
            return Ok(());
        }
        let worker_path = path.clone();
        let worker_ctx = ctx.clone();
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("deskorg-wallpaper".to_owned())
            .spawn(move || {
                let result = load_wallpaper_texture(&worker_ctx, &worker_path);
                let _ = tx.send((worker_path, result));
            })
            .map_err(|error| format!("无法启动壁纸载入：{error}"))?;
        self.wallpaper_job = Some(rx);
        Ok(())
    }

    fn poll_wallpaper_job(&mut self) {
        let received = self
            .wallpaper_job
            .as_ref()
            .map(|job| job.try_recv());
        match received {
            Some(Ok((path, result))) => {
                self.wallpaper_job = None;
                if path != self.config.wallpaper_path {
                    return;
                }
                match result {
                    Ok(texture) => {
                        self.wallpaper = Some(texture);
                        self.wallpaper_error = None;
                        self.notify("壁纸已载入，点「应用为壁纸」生效", NoticeKind::Info);
                    }
                    Err(error) => {
                        self.wallpaper = None;
                        self.wallpaper_error = Some(error.clone());
                        self.notify(error, NoticeKind::Error);
                    }
                }
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.wallpaper_job = None;
                self.notify("壁纸载入失败", NoticeKind::Error);
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
    }

    fn apply_wallpaper(&mut self, ctx: &egui::Context) {
        let path = PathBuf::from(self.config.wallpaper_path.trim());
        if path.as_os_str().is_empty() {
            self.notify("请先选择一张壁纸", NoticeKind::Info);
            return;
        }
        match shell::set_wallpaper(&path) {
            Ok(()) => {
                self.frost_source.clear();
                self.reload_frost(ctx);
                self.notify("壁纸已更新，桌面焕然一新", NoticeKind::Success);
            }
            Err(error) => self.notify(error, NoticeKind::Error),
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        // 托盘线程意外退出时，把主窗口找回来，避免用户再也打不开界面。
        if self.tray_available && !self.force_quit && !shell::tray_alive() {
            self.tray_available = false;
            self.hide_frames = 0;
            shell::show_main_window();
            self.notify("托盘图标已不可用，已恢复主窗口", NoticeKind::Error);
            ctx.request_repaint();
        }

        while let Ok(event) = self.events.try_recv() {
            match event {
                ShellEvent::Toggle => self.toggle_icons(),
                ShellEvent::ToggleFences => self.toggle_fences_from_tray(ctx),
                ShellEvent::ShowMain => {
                    self.hide_frames = 0;
                    shell::show_main_window();
                    ctx.request_repaint();
                }
                ShellEvent::Exit => {
                    self.force_quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        if self.last_icon_poll.elapsed() >= Duration::from_millis(900) {
            self.last_icon_poll = Instant::now();
            if let Some(visible) = shell::desktop_icons_visible() {
                let fence_mode = self.config.fences_enabled
                    && self.fences_visible
                    && !self.config.fences.is_empty();
                if visible && fence_mode {
                    // Native icons must never share the desktop with fences.
                    // 该调用内部用 SendMessageTimeout 代替 Sleep 轮询，
                    // 正常情况立即返回，Explorer 卡死时最多阻塞 200 ms。
                    if shell::set_desktop_icons_visible(false) == Ok(false) {
                        self.icons_visible = false;
                        self.hidden_by_app = true;
                        ctx.request_repaint();
                    }
                } else if visible != self.icons_visible {
                    self.icons_visible = visible;
                    if visible {
                        self.hidden_by_app = false;
                    }
                    ctx.request_repaint();
                }
            }
        }

        self.poll_job(ctx);
        self.poll_scan_job();
        self.poll_wallpaper_job();
        self.poll_frost_job();

        if let Some((x, y, width, height)) = shell::virtual_desktop_rect() {
            if width > 0.0 && height > 0.0 {
                // 物理像素 -> egui 逻辑点，避免高 DPI 下毛玻璃 UV 和
                // 自动布局使用错误的比例。
                let scale = ctx.pixels_per_point().max(0.1);
                self.desktop_origin = Vec2::new(x / scale, y / scale);
                self.desktop_size = Vec2::new(width / scale, height / scale);
            }
        } else if let Some(size) = ctx.input(|input| input.viewport().monitor_size) {
            if size.x > 0.0 && size.y > 0.0 {
                self.desktop_size = size;
            }
        }

        if (self.config.fences_enabled || self.page == Page::Fences)
            && self.last_fence_refresh.elapsed() > Duration::from_secs(4)
        {
            self.start_background_scan();
            self.reload_frost(ctx);
        } else if self.last_fence_refresh.elapsed() > Duration::from_secs(15) {
            self.reload_frost(ctx);
            self.last_fence_refresh = Instant::now();
        }

        if let Some(notice) = &self.notice {
            if notice.born.elapsed() > Duration::from_secs(4) {
                self.notice = None;
            }
        }

        // 空闲时约 1 Hz 轮询图标状态；只有后台任务、通知或待保存配置
        // 需要更快的重绘。热键/托盘事件会通过 shell 的 request_repaint
        // 主动唤醒 UI，所以不需要靠 250 ms 定时重绘来保证响应。
        let mut next_repaint = Duration::from_millis(900);
        if self.job.is_some() {
            next_repaint = Duration::from_millis(80);
        }
        if self.notice.is_some() {
            next_repaint = next_repaint.min(Duration::from_millis(250));
        }
        if self.config_dirty {
            next_repaint = next_repaint.min(Duration::from_millis(200));
        }
        if self.scan_job.is_some() {
            next_repaint = next_repaint.min(Duration::from_millis(300));
        }
        if self.wallpaper_job.is_some() || self.frost_job.is_some() {
            next_repaint = next_repaint.min(Duration::from_millis(200));
        }
        ctx.request_repaint_after(next_repaint);
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let received = self.job.as_ref().map(|job| job.rx.try_recv());
        match received {
            Some(Ok(outcome)) => {
                self.job = None;
                match outcome {
                    JobOutcome::Organize {
                        report,
                        skipped_shortcuts,
                        make_fences,
                        created_at,
                        journal_error,
                    } => self.finish_organize(
                        ctx,
                        report,
                        skipped_shortcuts,
                        make_fences,
                        created_at,
                        journal_error,
                    ),
                    JobOutcome::Undo {
                        report,
                        remaining,
                        created_at,
                        journal_error,
                    } => self.finish_undo(report, remaining, created_at, journal_error),
                }
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.job = None;
                self.notify("后台任务意外结束，请重新操作", NoticeKind::Error);
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
    }

    fn spawn_organize_job(
        &mut self,
        items: Vec<DesktopItem>,
        make_fences: bool,
        skipped_shortcuts: usize,
    ) {
        let target = self.target();
        let created_at = unix_now();
        let total = items.len();
        let progress = Arc::new(AtomicUsize::new(0));
        let worker_progress = Arc::clone(&progress);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("deskorg-organize".to_owned())
            .spawn(move || {
                let mut journal: Vec<MoveRecord> = Vec::new();
                let mut journal_error: Option<String> = None;
                let report = desktop::organize_with(&items, &target, &mut |record| {
                    worker_progress.fetch_add(1, Ordering::Relaxed);
                    journal.push(record.clone());
                    if journal_error.is_none() {
                        let state = UndoState {
                            created_at,
                            moves: journal.clone(),
                        };
                        if let Err(error) = state.save() {
                            journal_error = Some(error);
                        }
                    }
                }, &worker_cancel);
                if !report.moved.is_empty() {
                    let state = UndoState {
                        created_at,
                        moves: report.moved.clone(),
                    };
                    match state.save() {
                        Ok(()) => journal_error = None,
                        Err(error) => {
                            if journal_error.is_none() {
                                journal_error = Some(error);
                            }
                        }
                    }
                }
                let _ = tx.send(JobOutcome::Organize {
                    report,
                    skipped_shortcuts,
                    make_fences,
                    created_at,
                    journal_error,
                });
            });

        match spawned {
            Ok(_) => {
                self.job = Some(JobState {
                    rx,
                    label: format!("正在整理 {total} 个项目"),
                    progress,
                    cancel,
                    total,
                    started: Instant::now(),
                });
            }
            Err(error) => self.notify(
                format!("无法启动后台整理任务：{error}"),
                NoticeKind::Error,
            ),
        }
    }

    fn spawn_undo_job(&mut self, undo: UndoState) {
        let created_at = undo.created_at;
        let moves = undo.moves;
        let total = moves.len();
        let progress = Arc::new(AtomicUsize::new(0));
        let worker_progress = Arc::clone(&progress);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("deskorg-undo".to_owned())
            .spawn(move || {
                let mut remaining = moves.clone();
                let mut journal_error: Option<String> = None;
                let report = desktop::undo_with(&moves, &mut |record| {
                    worker_progress.fetch_add(1, Ordering::Relaxed);
                    if let Some(index) =
                        remaining.iter().position(|item| item.from == record.from)
                    {
                        remaining.remove(index);
                    }
                    let result = if remaining.is_empty() {
                        UndoState::clear()
                    } else {
                        UndoState {
                            created_at,
                            moves: remaining.clone(),
                        }
                        .save()
                    };
                    if let Err(error) = result {
                        journal_error = Some(error);
                    }
                }, &worker_cancel);
                let result = if remaining.is_empty() {
                    UndoState::clear()
                } else {
                    UndoState {
                        created_at,
                        moves: remaining.clone(),
                    }
                    .save()
                };
                match result {
                    Ok(()) => journal_error = None,
                    Err(error) => {
                        if journal_error.is_none() {
                            journal_error = Some(error);
                        }
                    }
                }
                let _ = tx.send(JobOutcome::Undo {
                    report,
                    remaining,
                    created_at,
                    journal_error,
                });
            });

        match spawned {
            Ok(_) => {
                self.job = Some(JobState {
                    rx,
                    label: format!("正在恢复 {total} 个项目"),
                    progress,
                    cancel,
                    total,
                    started: Instant::now(),
                });
            }
            Err(error) => self.notify(format!("无法启动后台撤销任务：{error}"), NoticeKind::Error),
        }
    }

    fn finish_organize(
        &mut self,
        ctx: &egui::Context,
        report: OrganizeReport,
        skipped_shortcuts: usize,
        make_fences: bool,
        created_at: u64,
        journal_error: Option<String>,
    ) {
        let cancelled = report.cancelled;
        let moved = report.moved.len();
        let failed = report.failed.len();
        let failed_paths: HashSet<PathBuf> =
            report.failed.iter().map(|item| item.path.clone()).collect();
        let shortcut_note = if skipped_shortcuts > 0 {
            format!("，{skipped_shortcuts} 个快捷方式保留在桌面")
        } else {
            String::new()
        };
        let failure_note = if failed > 0 {
            format!("，{failed} 项失败（已保留选择）")
        } else {
            String::new()
        };

        if cancelled && moved == 0 {
            self.notify("已停止，没有项目被移动", NoticeKind::Info);
            return;
        }

        if moved > 0 {
            self.undo = Some(UndoState {
                created_at,
                moves: report.moved.clone(),
            });
            self.rescan(true);
            if failed > 0 {
                self.selected.retain(|path| failed_paths.contains(path));
            }

            let created = if make_fences && !cancelled {
                self.ensure_fences_for_populated_categories()
            } else {
                0
            };
            if make_fences && !cancelled {
                self.page = Page::Fences;
                // 撤销记录失败的提示必须让用户看见，所以先不要把主窗口
                // 收进托盘；框框已经创建，用户可以手动显示。
                if journal_error.is_none() {
                    self.enable_fences(ctx);
                }
            }

            if let Some(error) = journal_error {
                self.notify(
                    format!(
                        "{}已归档 {moved} 项，但撤销记录保存失败：{error}{shortcut_note}",
                        if cancelled { "已停止；" } else { "" }
                    ),
                    NoticeKind::Error,
                );
            } else if cancelled {
                self.notify(
                    format!(
                        "已停止：已归档 {moved} 项{shortcut_note}{failure_note}；可撤销"
                    ),
                    NoticeKind::Info,
                );
            } else if make_fences {
                // enable_fences 失败时会自己 notify 错误，不要覆盖它。
                if self.config.fences_enabled {
                    self.notify(
                        if created > 0 {
                            format!(
                                "已归档 {moved} 项，并生成 {created} 个桌面框框{shortcut_note}{failure_note}"
                            )
                        } else {
                            format!(
                                "已归档 {moved} 项，桌面框框已更新{shortcut_note}{failure_note}"
                            )
                        },
                        NoticeKind::Success,
                    );
                }
            } else if failed > 0 {
                self.notify(
                    format!("已归档 {moved} 项，{failed} 项失败（已保留选择）{shortcut_note}"),
                    NoticeKind::Info,
                );
            } else {
                self.notify(
                    format!("已归档 {moved} 项，桌面清爽了{shortcut_note}"),
                    NoticeKind::Success,
                );
                if self.config.auto_hide_after_organize && self.icons_visible {
                    self.toggle_icons();
                }
            }
        } else if let Some(first) = report.failed.first() {
            self.notify(format!("整理失败：{}", first.message), NoticeKind::Error);
        } else {
            self.notify("没有项目需要整理", NoticeKind::Info);
        }
    }

    fn finish_undo(
        &mut self,
        report: UndoReport,
        remaining: Vec<MoveRecord>,
        created_at: u64,
        journal_error: Option<String>,
    ) {
        let cancelled = report.cancelled;
        let restored = report.restored.len();
        let failed = report.failed.len();

        if remaining.is_empty() {
            self.undo = None;
        } else {
            self.undo = Some(UndoState {
                created_at,
                moves: remaining,
            });
        }
        self.rescan(true);

        if let Some(error) = journal_error {
            let stopped = if cancelled { "已停止；" } else { "" };
            let prefix = if restored > 0 {
                format!("已把 {restored} 项放回桌面，")
            } else {
                String::new()
            };
            self.notify(
                format!("{stopped}{prefix}但撤销记录更新失败：{error}"),
                NoticeKind::Error,
            );
        } else if cancelled {
            if restored > 0 {
                let suffix = if failed > 0 {
                    format!("，{failed} 项未能恢复")
                } else {
                    String::new()
                };
                self.notify(
                    format!("已停止：已把 {restored} 项放回桌面{suffix}"),
                    NoticeKind::Info,
                );
            } else {
                self.notify("已停止，没有项目被恢复", NoticeKind::Info);
            }
        } else if restored > 0 {
            let suffix = if failed > 0 {
                format!("，{failed} 项未能恢复")
            } else {
                String::new()
            };
            self.notify(
                format!("已把 {restored} 项放回桌面{suffix}"),
                NoticeKind::Success,
            );
        } else if let Some(first) = report.failed.first() {
            self.notify(format!("撤销失败：{}", first.message), NoticeKind::Error);
        }
    }

    fn ensure_window_theme(&mut self, frame: &eframe::Frame) {
        if self.applied_window_theme == Some(self.config.light_theme) {
            return;
        }
        #[cfg(windows)]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(handle) = frame.window_handle() {
                if let RawWindowHandle::Win32(win32) = handle.as_raw() {
                    let hwnd = win32.hwnd.get() as *mut std::ffi::c_void;
                    shell::set_window_theme(hwnd, !self.config.light_theme);
                    self.applied_window_theme = Some(self.config.light_theme);
                }
            }
        }
        #[cfg(not(windows))]
        {
            self.applied_window_theme = Some(self.config.light_theme);
        }
    }

    fn page_header(&self, ui: &mut Ui, p: &Palette) -> bool {
        let mut refresh = false;
        let width = (ui.available_width() - 22.0).max(320.0);
        ui.allocate_ui_with_layout(
            Vec2::new(width, 56.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(self.page.title())
                            .size(27.0)
                            .strong()
                            .color(p.text),
                    );
                    ui.label(
                        RichText::new(self.page.subtitle())
                            .size(12.5)
                            .color(p.muted),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::ghost_button(ui, p, "刷新").clicked() {
                        refresh = true;
                    }
                    let fences_active =
                        self.config.fences_enabled && !self.config.fences.is_empty();
                    let (label, color) = if fences_active {
                        if self.fences_visible {
                            ("框框显示", p.success)
                        } else {
                            ("框框隐藏", p.accent_ink)
                        }
                    } else if self.icons_visible {
                        ("图标可见", p.success)
                    } else {
                        ("清爽模式", p.accent_ink)
                    };
                    let _ = widgets::pill(ui, p, label, color);
                    // 周期扫描通常几十毫秒就结束；只有明显偏慢时才显示提示，
                    // 避免每 4 秒闪一下“扫描中”。
                    if self
                        .scan_job
                        .as_ref()
                        .map(|job| job.started.elapsed() > Duration::from_millis(800))
                        .unwrap_or(false)
                    {
                        let _ = widgets::pill(ui, p, "扫描中", p.warning);
                    }
                });
            },
        );
        ui.add_space(8.0);
        refresh
    }

    fn sidebar(&mut self, ui: &mut Ui, p: &Palette) {
        ui.horizontal(|ui| {
            widgets::app_logo(ui, p, self.logo.as_ref());
            ui.vertical(|ui| {
                ui.label(RichText::new("DeskOrg").size(19.0).strong().color(p.text));
                ui.label(
                    RichText::new("桌面整理 · 一键清爽")
                        .size(10.5)
                        .color(p.faint),
                );
            });
        });

        ui.add_space(24.0);
        for (page, glyph, label) in Page::ALL {
            if widgets::nav_item(ui, p, glyph, label, self.page == page).clicked() {
                self.page = page;
                self.confirm = None;
            }
        }

        ui.add_space((ui.available_height() - 196.0).max(14.0));

        theme::soft_frame(*p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let fences_enabled = self.config.fences_enabled && !self.config.fences.is_empty();
            let fences_showing = fences_enabled && self.fences_visible;
            ui.horizontal(|ui| {
                let color = if fences_showing || self.icons_visible {
                    p.success
                } else {
                    p.accent
                };
                let (rect, _) = ui.allocate_exact_size(Vec2::new(9.0, 9.0), Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.5, color);
                ui.label(
                    RichText::new(if fences_showing {
                        "桌面框框显示中"
                    } else if fences_enabled {
                        "所有框框已隐藏"
                    } else if self.icons_visible {
                        "桌面图标可见"
                    } else {
                        "清爽模式进行中"
                    })
                    .size(12.0)
                    .color(p.text),
                );
            });
            ui.label(
                RichText::new(if fences_showing {
                    "双击桌面空白处可全部隐藏"
                } else if fences_enabled {
                    "双击桌面空白处可显示全部框框"
                } else {
                    "双击桌面空白处，或按 Ctrl+Alt+D"
                })
                .size(10.5)
                .color(p.faint),
            );
            ui.add_space(4.0);
            if widgets::secondary_button(ui, p, "打开归档位置", 0.0).clicked() {
                self.open_target();
            }
        });

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                    .size(10.0)
                    .color(p.faint),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new("Windows 10 / 11").size(10.0).color(p.faint));
            });
        });
    }

    fn show_overview(&mut self, ui: &mut Ui, p: &Palette) {
        if self.page_header(ui, p) {
            self.start_background_scan();
        }

        let fences_active = self.config.fences_enabled && !self.config.fences.is_empty();
        let visible = if fences_active {
            self.fences_visible
        } else {
            self.icons_visible
        };
        let mut toggle_requested = false;

        theme::card_frame(*p)
            .inner_margin(egui::Margin::symmetric(28, 24))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    let _ = widgets::pill(ui, p, "ZEN MODE", p.accent_ink);
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(if fences_active {
                            "双击，显示或隐藏所有框框"
                        } else {
                            "双击，让桌面安静下来"
                        })
                        .size(25.0)
                        .strong()
                        .color(p.text),
                    );
                    ui.label(
                        RichText::new(if fences_active {
                            "所有框框会一起显示或一起隐藏，原生桌面图标始终保持隐藏。"
                        } else {
                            "图标会暂时隐身，壁纸重新成为主角。再双击一次，它们就会回来。"
                        })
                        .size(12.5)
                        .color(p.muted),
                    );
                    ui.add_space(2.0);
                    let response = widgets::hero_toggle(ui, p, visible);
                    if response.double_clicked() {
                        toggle_requested = true;
                    }
                    let (status, color) = if fences_active {
                        if visible {
                            ("所有框框显示中 · 双击月亮全部隐藏", p.success)
                        } else {
                            ("所有框框已隐藏 · 双击月亮恢复", p.accent_ink)
                        }
                    } else if visible {
                        ("桌面图标当前可见 · 双击月亮隐藏", p.success)
                    } else {
                        ("桌面图标已隐藏 · 双击月亮找回", p.accent_ink)
                    };
                    ui.label(RichText::new(status).size(12.5).color(color));
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("也可以").size(10.5).color(p.faint));
                        widgets::key_cap(ui, p, "Ctrl");
                        ui.label(RichText::new("+").size(10.5).color(p.faint));
                        widgets::key_cap(ui, p, "Alt");
                        ui.label(RichText::new("+").size(10.5).color(p.faint));
                        widgets::key_cap(ui, p, "D");
                        ui.label(RichText::new("· 双击桌面空白处").size(10.5).color(p.faint));
                    });
                });
            });

        if toggle_requested {
            self.toggle_icons();
        }

        ui.add_space(14.0);

        let counts = desktop::count_by_category(&self.scan.items);
        let categories_used = counts.values().filter(|count| **count > 0).count();
        let selected_count = self.selected.len();
        let size = desktop::total_size(&self.scan.items);
        let columns = 4;
        ui.columns(columns, |cols| {
            stat_tile(
                &mut cols[0],
                p,
                &self.scan.items.len().to_string(),
                "桌面项目",
                p.text,
            );
            stat_tile(
                &mut cols[1],
                p,
                &selected_count.to_string(),
                "当前选中",
                p.accent_ink,
            );
            stat_tile(
                &mut cols[2],
                p,
                &format!("{categories_used}/10"),
                "已用分类",
                p.text,
            );
            stat_tile(
                &mut cols[3],
                p,
                &desktop::format_size(size),
                "桌面占用",
                p.text,
            );
        });

        ui.add_space(18.0);
        widgets::section_header(ui, p, "桌面构成", "点击分类可快速筛选");
        ui.add_space(2.0);

        let mut picked_category = None;
        for chunk in Category::ALL.chunks(5) {
            ui.columns(5, |cols| {
                for (index, category) in chunk.iter().enumerate() {
                    let count = counts.get(category).copied().unwrap_or(0);
                    if category_tile(&mut cols[index], p, *category, count).clicked() {
                        picked_category = Some(*category);
                    }
                }
            });
            ui.add_space(4.0);
        }
        if let Some(category) = picked_category {
            self.category_filter = Some(category);
            self.selected = self
                .scan
                .items
                .iter()
                .filter(|item| item.category == category)
                .map(|item| item.path.clone())
                .collect();
            self.page = Page::Organize;
        }

        ui.add_space(14.0);
        if !self.recent.is_empty() {
            widgets::section_header(ui, p, "桌面速览", "双击文件名可打开");
            for item in &self.recent {
                item_row(ui, p, item, false, &mut false);
            }
        }
    }

    fn show_organize(&mut self, ui: &mut Ui, p: &Palette) {
        if self.page_header(ui, p) {
            self.start_background_scan();
        }

        if !self.scan.warnings.is_empty() {
            let warning = self.scan.warnings.first().cloned().unwrap_or_default();
            egui::Frame::new()
                .fill(with_alpha(p.warning, 18))
                .stroke(Stroke::new(1.0, with_alpha(p.warning, 70)))
                .corner_radius(12)
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(format!("扫描提示：{warning}"))
                            .size(11.5)
                            .color(p.warning),
                    );
                });
            ui.add_space(10.0);
        }

        let target = self.target();
        let target_text = target.to_string_lossy().to_string();
        let mut open_clicked = false;
        theme::card_frame(*p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("归档位置").size(12.0).color(p.faint));
                    ui.add(
                        egui::Label::new(
                            RichText::new(&target_text)
                                .monospace()
                                .size(12.0)
                                .color(p.text),
                        )
                        .truncate(),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::secondary_button(ui, p, "打开", 74.0).clicked() {
                        open_clicked = true;
                    }
                });
            });
            ui.label(
                RichText::new(
                    "整理只移动桌面第一层项目，不会删除文件；归档后仍可用「撤销上次整理」放回。",
                )
                .size(10.5)
                .color(p.faint),
            );
        });
        if open_clicked {
            self.open_target();
        }

        let public_count = self
            .scan
            .items
            .iter()
            .filter(|item| item.origin == DesktopOrigin::Public)
            .count();
        if public_count > 0 {
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "{public_count} 个项目来自公共桌面，移动时可能需要管理员权限。"
                ))
                .size(10.5)
                .color(p.warning),
            );
        }

        ui.add_space(14.0);

        let mut organize_clicked = false;
        let mut undo_clicked = false;
        let mut select_all = false;
        let mut clear = false;
        theme::card_frame(*p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(format!("已选择 {} 个项目", self.selected.len()))
                            .size(15.0)
                            .strong()
                            .color(p.text),
                    );
                    ui.label(
                        RichText::new("勾选要归位的项目，分类文件夹会自动创建")
                            .size(10.5)
                            .color(p.faint),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::primary_button(
                        ui,
                        p,
                        &format!("整理选中 ({})", self.selected.len()),
                        138.0,
                    )
                    .clicked()
                    {
                        organize_clicked = true;
                    }
                    if widgets::secondary_button(ui, p, "清空", 62.0).clicked() {
                        clear = true;
                    }
                    if widgets::secondary_button(ui, p, "全选", 62.0).clicked() {
                        select_all = true;
                    }
                });
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if let Some(undo) = &self.undo {
                    if widgets::ghost_button(
                        ui,
                        p,
                        &format!("撤销上次整理 · {} 项", undo.moves.len()),
                    )
                    .clicked()
                    {
                        undo_clicked = true;
                    }
                } else {
                    widgets::footer_hint(ui, p, "暂无撤销记录");
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let selected_size = self
                        .scan
                        .items
                        .iter()
                        .filter(|item| self.selected.contains(&item.path))
                        .map(|item| item.size)
                        .sum::<u64>();
                    widgets::footer_hint(
                        ui,
                        p,
                        &format!("已选 {}", desktop::format_size(selected_size)),
                    );
                });
            });
        });

        if select_all {
            self.select_all();
        }
        if clear {
            self.clear_selection();
        }
        if undo_clicked {
            self.request_undo();
        }
        if organize_clicked {
            self.request_organize(true);
        }

        ui.add_space(14.0);

        let counts = desktop::count_by_category(&self.scan.items);
        let mut chosen_filter: Option<Option<Category>> = None;
        ui.horizontal_wrapped(|ui| {
            if filter_chip(
                ui,
                p,
                "全部",
                self.scan.items.len(),
                p.accent,
                self.category_filter.is_none(),
            )
            .clicked()
            {
                chosen_filter = Some(None);
            }
            for category in Category::ALL {
                let count = counts.get(&category).copied().unwrap_or(0);
                if filter_chip(
                    ui,
                    p,
                    category.label(),
                    count,
                    theme::category_color(category),
                    self.category_filter == Some(category),
                )
                .clicked()
                {
                    chosen_filter = Some(Some(category));
                }
            }
        });
        if let Some(filter) = chosen_filter {
            self.category_filter = filter;
        }

        ui.add_space(8.0);
        let filtered_before_edit =
            filtered_items(&self.scan.items, self.category_filter, &self.search);
        let mut select_filtered = false;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("搜索桌面文件…")
                    .desired_width(250.0),
            );
            if widgets::ghost_button(ui, p, "选中当前列表").clicked() {
                select_filtered = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(filter) = self.category_filter {
                    if widgets::ghost_button(ui, p, "清除筛选").clicked() {
                        self.category_filter = None;
                    }
                    let _ = widgets::pill(ui, p, filter.label(), theme::category_color(filter));
                }
            });
        });
        if select_filtered {
            self.selected = filtered_before_edit
                .iter()
                .map(|item| item.path.clone())
                .collect();
        }

        ui.add_space(6.0);
        let items = filtered_items(&self.scan.items, self.category_filter, &self.search);
        if items.is_empty() {
            theme::card_frame(*p).show(ui, |ui| {
                ui.set_width(ui.available_width());
                widgets::empty_state(
                    ui,
                    p,
                    "✓",
                    if self.scan.items.is_empty() {
                        "桌面已经很干净"
                    } else {
                        "没有符合条件的项目"
                    },
                    "调整搜索或筛选条件，或者刷新桌面。",
                );
            });
        } else {
            for item in &items {
                let mut checked = self.selected.contains(&item.path);
                item_row(ui, p, item, true, &mut checked);
                if checked {
                    self.selected.insert(item.path.clone());
                } else {
                    self.selected.remove(&item.path);
                }
            }
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "显示 {} / {} 项",
                    items.len(),
                    self.scan.items.len()
                ))
                .size(10.5)
                .color(p.faint),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(last) = self.last_scan {
                    ui.label(
                        RichText::new(format!("扫描于 {} 秒前", last.elapsed().as_secs()))
                            .size(10.5)
                            .color(p.faint),
                    );
                }
            });
        });
    }

    fn show_fences_page(&mut self, ui: &mut Ui, p: &Palette) {
        if self.page_header(ui, p) {
            self.start_background_scan();
            self.reload_frost(ui.ctx());
        }

        let showing = self.config.fences_enabled && self.fences_visible;
        let fence_total = self.config.fences.len();
        let total_items: usize = self.fence_data.values().map(FenceData::len).sum();
        let mut toggle_clicked = false;
        let mut exit_clicked = false;
        let mut new_clicked = false;
        let mut generate_clicked = false;
        let mut only_generate_clicked = false;
        let mut organize_clicked = false;

        theme::card_frame(*p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        let _ = widgets::pill(ui, p, "FENCES", p.accent_ink);
                        ui.label(RichText::new("桌面框框").size(22.0).strong().color(p.text));
                    });
                    ui.label(
                        RichText::new("把分类文件装进透明盒子，拖动顶部即可摆放。")
                            .size(11.5)
                            .color(p.muted),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::primary_button(
                        ui,
                        p,
                        if showing {
                            "全部隐藏"
                        } else if self.config.fences_enabled {
                            "显示全部"
                        } else {
                            "显示框框"
                        },
                        108.0,
                    )
                    .clicked()
                    {
                        toggle_clicked = true;
                    }
                    if widgets::secondary_button(ui, p, "新建框框", 96.0).clicked() {
                        new_clicked = true;
                    }
                    if self.config.fences_enabled
                        && widgets::ghost_button(ui, p, "退出框框模式").clicked()
                    {
                        exit_clicked = true;
                    }
                });
            });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                let (label, color) = if showing {
                    ("显示中", p.success)
                } else if self.config.fences_enabled {
                    ("所有框框已隐藏", p.warning)
                } else {
                    ("已暂停", p.faint)
                };
                let _ = widgets::pill(ui, p, label, color);
                let _ = widgets::pill(ui, p, &format!("{fence_total} 个框框"), p.accent_ink);
                let _ = widgets::pill(ui, p, &format!("{total_items} 个项目"), p.text);
                if self.config.fences_after_organize {
                    let _ = widgets::pill(ui, p, "整理后自动生成", p.success);
                }
            });
            ui.label(
                RichText::new(
                    "双击桌面空白处可一起显示或隐藏所有框框；启用后主窗口收进右下角托盘，点托盘图标可再次打开控制中心。",
                )
                .size(9.5)
                .color(p.faint),
            );
        });

        ui.add_space(14.0);

        theme::card_frame(*p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("分类并生成框框")
                            .size(16.0)
                            .strong()
                            .color(p.text),
                    );
                    ui.label(
                        RichText::new(
                            "把桌面文件按类型归档，并自动生成对应分类的透明框框；文件只会移动，不会删除。",
                        )
                        .size(10.5)
                        .color(p.faint),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::primary_button(ui, p, "分类并生成框框", 142.0).clicked() {
                        organize_clicked = true;
                    }
                    if widgets::secondary_button(ui, p, "仅生成框框", 102.0).clicked() {
                        only_generate_clicked = true;
                    }
                });
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "当前桌面有 {} 个项目。也可以只生成框框，不移动任何文件。",
                    self.scan.items.len()
                ))
                .size(10.0)
                .color(p.faint),
            );
        });

        ui.add_space(16.0);
        ui.horizontal(|ui| {
            widgets::section_header(ui, p, "我的框框", "拖动顶部移动 · 右下角缩放");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::ghost_button(ui, p, "生成分类框框").clicked() {
                    generate_clicked = true;
                }
            });
        });
        ui.add_space(4.0);

        if self.config.fences.is_empty() {
            theme::card_frame(*p).show(ui, |ui| {
                ui.set_width(ui.available_width());
                widgets::empty_state(
                    ui,
                    p,
                    "框",
                    "还没有桌面框框",
                    "点「分类并生成框框」，或手动新建一个分类框。",
                );
                ui.horizontal(|ui| {
                    if widgets::primary_button(ui, p, "生成分类框框", 130.0).clicked() {
                        generate_clicked = true;
                    }
                    if widgets::secondary_button(ui, p, "新建框框", 100.0).clicked() {
                        new_clicked = true;
                    }
                });
            });
        } else {
            let mut delete_id = None;
            let mut open_category = None;
            let mut dirty = false;
            let fallback = FenceData::default();
            for index in 0..self.config.fences.len() {
                let mut fence = self.config.fences[index].clone();
                let data = self.fence_data.get(&fence.category).unwrap_or(&fallback);
                let before_theme = fence.theme;
                let before_opacity = fence.opacity;
                let before_blur = fence.blur;
                let (open, delete) = fence_card(ui, p, &mut fence, data, showing);
                if open {
                    open_category = Some(fence.category);
                }
                if delete {
                    delete_id = Some(fence.id);
                } else {
                    if fence.theme != before_theme
                        || (fence.opacity - before_opacity).abs() > 0.0001
                        || fence.blur != before_blur
                    {
                        self.config.fences[index] = fence;
                        dirty = true;
                    }
                }
            }
            if dirty {
                self.config_dirty = true;
                if self.config.fences.iter().any(|fence| fence.blur) {
                    self.reload_frost(ui.ctx());
                } else {
                    self.frost = None;
                    self.frost_job = None;
                    self.frost_source.clear();
                }
            }
            if let Some(category) = open_category {
                self.open_fence_folder(category);
            }
            if let Some(id) = delete_id {
                self.delete_fence(id, ui.ctx());
            }
        }

        if toggle_clicked {
            if self.config.fences_enabled {
                self.toggle_icons();
            } else {
                self.enable_fences(ui.ctx());
            }
        }
        if exit_clicked {
            self.disable_fences(ui.ctx());
        }
        if new_clicked {
            self.open_new_fence_modal();
        }
        if generate_clicked || only_generate_clicked {
            let created = self.ensure_fences_for_populated_categories();
            if created == 0 && self.config.fences.is_empty() {
                self.notify("桌面上还没有可以分类的项目", NoticeKind::Info);
            } else {
                self.enable_fences(ui.ctx());
                if self.config.fences_enabled {
                    self.notify(
                        if created > 0 {
                            format!("已生成 {created} 个桌面框框")
                        } else {
                            "桌面框框已就绪".to_owned()
                        },
                        NoticeKind::Success,
                    );
                }
            }
        }
        if organize_clicked {
            self.config.fences_after_organize = true;
            self.selected = self
                .scan
                .items
                .iter()
                .map(|item| item.path.clone())
                .collect();
            self.request_organize(true);
        }
    }

    fn open_fence_folder(&mut self, category: Category) {
        let path = self.target().join(category.folder_name());
        let _ = std::fs::create_dir_all(&path);
        if let Err(error) = shell::open_in_explorer(&path) {
            self.notify(error, NoticeKind::Error);
        }
    }

    fn open_new_fence_modal(&mut self) {
        let missing = Category::ALL
            .into_iter()
            .find(|category| !self.config.fences.iter().any(|f| f.category == *category));
        if let Some(category) = missing {
            self.new_fence_category = category;
            self.new_fence_theme = FenceTheme::default_for(category);
            self.new_fence_open = true;
        } else {
            self.notify("每个分类都已经有框框了", NoticeKind::Info);
        }
    }

    fn new_fence_modal(&mut self, ctx: &egui::Context, p: &Palette) {
        if !self.new_fence_open {
            return;
        }
        let missing: Vec<Category> = Category::ALL
            .into_iter()
            .filter(|category| !self.config.fences.iter().any(|f| f.category == *category))
            .collect();
        let mut create = false;
        let mut cancel = false;

        egui::Modal::new(Id::unique("deskorg-new-fence"))
            .frame(theme::card_frame(*p).inner_margin(egui::Margin::same(22)))
            .show(ctx, |ui| {
                ui.set_width(510.0);
                ui.label(
                    RichText::new("新建桌面框框")
                        .size(19.0)
                        .strong()
                        .color(p.text),
                );
                ui.label(
                    RichText::new("每个分类只能有一个框框；稍后可以单独设置主题、透明度与毛玻璃。")
                        .size(11.0)
                        .color(p.faint),
                );
                ui.add_space(8.0);
                if missing.is_empty() {
                    widgets::empty_state(
                        ui,
                        p,
                        "✓",
                        "所有分类都有框框了",
                        "可以删除旧框框，或调整现有框框的主题。",
                    );
                } else {
                    ui.horizontal_wrapped(|ui| {
                        for category in &missing {
                            let selected = self.new_fence_category == *category;
                            let count = self.fence_count(*category);
                            let text = format!("{}  {count}", category.label());
                            let button = egui::Button::new(
                                RichText::new(text).size(11.5).color(if selected {
                                    p.accent_ink
                                } else {
                                    p.text
                                }),
                            )
                            .fill(if selected {
                                p.accent_soft
                            } else {
                                p.panel_soft
                            })
                            .stroke(Stroke::new(1.0, if selected { p.accent } else { p.border }))
                            .corner_radius(9)
                            .min_size(Vec2::new(132.0, 32.0));
                            if ui.add(button).clicked() {
                                self.new_fence_category = *category;
                                self.new_fence_theme = FenceTheme::default_for(*category);
                            }
                        }
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("主题").size(11.5).color(p.muted));
                        for theme in FenceTheme::ALL {
                            if fences::theme_chip(ui, theme, self.new_fence_theme == theme)
                                .clicked()
                            {
                                self.new_fence_theme = theme;
                            }
                        }
                    });
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if widgets::secondary_button(ui, p, "取消", 92.0).clicked() {
                        cancel = true;
                    }
                    if !missing.is_empty()
                        && widgets::primary_button(ui, p, "创建框框", 110.0).clicked()
                    {
                        create = true;
                    }
                });
            });

        if cancel {
            self.new_fence_open = false;
        }
        if create && missing.contains(&self.new_fence_category) {
            let id = self.next_fence_id();
            let index = self.config.fences.len();
            let mut fence = Fence::new(
                id,
                self.new_fence_category,
                index,
                self.desktop_size,
                self.desktop_origin,
            );
            let (x, y) = self.next_fence_position();
            fence.x = x;
            fence.y = y;
            fence.theme = self.new_fence_theme;
            self.config.fences.push(fence);
            self.new_fence_open = false;
            let _ = self.config.save();
            self.notify("框框已创建；显示框框后会出现在桌面上", NoticeKind::Success);
        }
    }

    fn show_fence_viewports(&mut self, ctx: &egui::Context) {
        let showing = self.config.fences_enabled && self.fences_visible;
        if !showing && self.fence_viewports_created.is_empty() {
            self.fences_were_showing = false;
            return;
        }
        let restore_geometry = showing && !self.fences_were_showing;
        let just_hidden = !showing && self.fences_were_showing;
        self.fences_were_showing = showing;
        if showing {
            self.ensure_fence_icons(ctx, 8);
            if self.frost.is_none()
                && self.frost_job.is_none()
                && self.config.fences.iter().any(|fence| fence.blur)
            {
                self.reload_frost(ctx);
            }
        }

        let desktop = egui::Rect::from_min_size(
            egui::pos2(self.desktop_origin.x, self.desktop_origin.y),
            self.desktop_size,
        );
        let fallback = FenceData::default();
        let mut actions = Vec::new();
        let mut layout_updates = Vec::new();
        let created = self.fence_viewports_created.clone();
        let mut newly_created = Vec::new();

        for fence in &self.config.fences {
            let data = self.fence_data.get(&fence.category).unwrap_or(&fallback);
            let include_geometry = !created.contains(&fence.id) || restore_geometry;
            let result = ctx.show_viewport_immediate(
                fence_viewport_id(fence.id),
                fence_viewport_builder(fence, include_geometry),
                |ui, _class| {
                    if !showing {
                        return (None, None, None);
                    }
                    let action = fences::draw_fence(
                        ui,
                        fence,
                        data,
                        self.frost.as_ref(),
                        &self.icon_cache,
                        desktop,
                    );
                    let outer = ui.ctx().input(|input| input.viewport().outer_rect);
                    let inner = ui.ctx().input(|input| input.viewport().inner_rect);
                    (action, outer, inner)
                },
            );
            if include_geometry {
                newly_created.push(fence.id);
                ctx.send_viewport_cmd_to(
                    fence_viewport_id(fence.id),
                    egui::ViewportCommand::Visible(true),
                );
                if showing {
                    ctx.send_viewport_cmd_to(
                        fence_viewport_id(fence.id),
                        egui::ViewportCommand::OuterPosition(egui::Pos2::new(fence.x, fence.y)),
                    );
                }
            }
            if let Some(action) = result.0 {
                actions.push(action);
            }
            if showing {
                layout_updates.push((fence.id, result.1, result.2));
            } else if just_hidden {
                // Keep the immediate viewport alive, but park it far away. This
                // avoids the egui "callback was never called" panic caused by
                // hide/re-show lifecycle changes. Only do this on the frame the
                // fences become hidden, not every frame.
                ctx.send_viewport_cmd_to(
                    fence_viewport_id(fence.id),
                    egui::ViewportCommand::OuterPosition(egui::Pos2::new(-24000.0, -24000.0)),
                );
            }
        }

        for id in &newly_created {
            if let Some(fence) = self.config.fences.iter().find(|fence| fence.id == *id) {
                shell::make_tool_window(&fence.window_title());
            }
        }
        self.fence_viewports_created.extend(newly_created);

        // 由独立守护线程处理框框层级：普通应用在前台时压到底层，
        // Win+D 显示桌面时临时抬到桌面之上。
        shell::set_fence_guard(showing);
        let mut dirty = false;
        for (id, outer, inner) in layout_updates {
            if let Some(fence) = self.config.fences.iter_mut().find(|f| f.id == id) {
                if let Some(rect) = outer {
                    if (rect.min.x - fence.x).abs() > 1.0 || (rect.min.y - fence.y).abs() > 1.0 {
                        fence.x = rect.min.x;
                        fence.y = rect.min.y;
                        dirty = true;
                    }
                }
                if let Some(rect) = inner {
                    if (rect.width() - fence.w).abs() > 1.0 || (rect.height() - fence.h).abs() > 1.0
                    {
                        fence.w = rect.width();
                        fence.h = rect.height();
                        dirty = true;
                    }
                }
                fence.clamp_size();
            }
        }
        if dirty {
            self.config_dirty = true;
        }

        for action in actions {
            match action {
                FenceAction::Delete(id) => self.delete_fence(id, ctx),
                FenceAction::OpenFolder(id) => {
                    if let Some(category) = self
                        .config
                        .fences
                        .iter()
                        .find(|fence| fence.id == id)
                        .map(|fence| fence.category)
                    {
                        self.open_fence_folder(category);
                    }
                }
            }
        }
    }

    fn show_beauty(&mut self, ui: &mut Ui, p: &Palette) {
        if self.page_header(ui, p) {
            self.start_background_scan();
        }
        // 壁纸预览只在进入美化页时加载，启动时不再解码大图。
        if self.wallpaper.is_none()
            && self.wallpaper_job.is_none()
            && self.wallpaper_error.is_none()
            && !self.config.wallpaper_path.trim().is_empty()
        {
            let _ = self.start_wallpaper_preview_load(ui.ctx());
        }

        let texture = self.wallpaper.clone();
        let mut pick = false;
        let mut apply = false;
        theme::card_frame(*p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("桌面壁纸").size(16.0).strong().color(p.text));
                    ui.label(
                        RichText::new("换一张喜欢的图，让清爽模式更有氛围")
                            .size(11.0)
                            .color(p.faint),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::primary_button(ui, p, "应用为壁纸", 116.0).clicked() {
                        apply = true;
                    }
                    if widgets::secondary_button(ui, p, "选择图片", 96.0).clicked() {
                        pick = true;
                    }
                });
            });
            ui.add_space(8.0);

            let width = ui.available_width();
            let preview_size = Vec2::new(width, (width * 0.36).clamp(190.0, 330.0));
            if let Some(texture) = &texture {
                let image =
                    egui::Image::new(egui::load::SizedTexture::new(texture.id(), preview_size))
                        .fit_to_exact_size(preview_size)
                        .corner_radius(14);
                ui.add(image);
            } else {
                wallpaper_placeholder(ui, p, preview_size);
            }

            ui.add_space(6.0);
            if let Some(error) = &self.wallpaper_error {
                ui.label(RichText::new(error).size(10.5).color(p.danger));
            } else if self.config.wallpaper_path.is_empty() {
                ui.label(
                    RichText::new("支持 PNG / JPG / BMP / WebP")
                        .size(10.0)
                        .color(p.faint),
                );
            } else {
                ui.add(
                    egui::Label::new(
                        RichText::new(&self.config.wallpaper_path)
                            .monospace()
                            .size(10.0)
                            .color(p.faint),
                    )
                    .truncate(),
                );
            }
        });

        if pick {
            self.pick_wallpaper(ui.ctx());
        }
        if apply {
            self.apply_wallpaper(ui.ctx());
        }

        ui.add_space(14.0);

        let mut accent_choice = None;
        let mut theme_choice = None;
        theme::card_frame(*p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("主题与配色")
                    .size(16.0)
                    .strong()
                    .color(p.text),
            );
            ui.label(
                RichText::new("界面会记住你的选择")
                    .size(11.0)
                    .color(p.faint),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("强调色").size(12.0).color(p.muted));
                ui.add_space(4.0);
                for index in 0..theme::ACCENTS.len() {
                    if widgets::accent_chip(ui, p, index, self.config.accent == index).clicked() {
                        accent_choice = Some(index);
                    }
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("界面").size(12.0).color(p.muted));
                if segmented(ui, p, "深色", !self.config.light_theme).clicked()
                    && self.config.light_theme
                {
                    theme_choice = Some(false);
                }
                if segmented(ui, p, "浅色", self.config.light_theme).clicked()
                    && !self.config.light_theme
                {
                    theme_choice = Some(true);
                }
            });
        });

        if let Some(index) = accent_choice {
            self.config.accent = index;
            self.save_config();
            theme::apply_style(ui.ctx(), self.config.light_theme, self.config.accent);
            self.applied_window_theme = None;
        }
        if let Some(light) = theme_choice {
            self.config.light_theme = light;
            self.save_config();
            theme::apply_style(ui.ctx(), self.config.light_theme, self.config.accent);
            self.applied_window_theme = None;
        }

        ui.add_space(14.0);

        let mut clean_toggle = false;
        let mut changed = false;
        theme::card_frame(*p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("桌面清爽").size(16.0).strong().color(p.text));
                    ui.label(
                        RichText::new("隐藏图标不会关闭任何程序，也不会删除文件")
                            .size(11.0)
                            .color(p.faint),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::primary_button(
                        ui,
                        p,
                        if self.icons_visible {
                            "立即隐藏图标"
                        } else {
                            "让图标回来"
                        },
                        128.0,
                    )
                    .clicked()
                    {
                        clean_toggle = true;
                    }
                });
            });
            ui.add_space(4.0);
            changed |= widgets::toggle_row(
                ui,
                p,
                "整理后自动生成桌面框框",
                "把分类结果装进透明框框，并隐藏桌面图标",
                &mut self.config.fences_after_organize,
            );
            changed |= widgets::toggle_row(
                ui,
                p,
                "整理后自动进入清爽模式",
                "未生成框框时，移动完成后隐藏桌面图标",
                &mut self.config.auto_hide_after_organize,
            );
            changed |= widgets::toggle_row(
                ui,
                p,
                "双击桌面空白处切换",
                "双击文件或应用图标不会触发，只响应桌面背景",
                &mut self.config.desktop_double_click,
            );
            changed |= widgets::toggle_row(
                ui,
                p,
                "退出时恢复桌面图标",
                "即使 DeskOrg 退出，也不会把图标留在隐藏状态",
                &mut self.config.restore_on_exit,
            );
        });

        if clean_toggle {
            self.toggle_icons();
        }
        if changed {
            self.save_config();
            shell::set_double_click_enabled(self.config.desktop_double_click);
        }
    }

    fn show_settings(&mut self, ui: &mut Ui, p: &Palette) {
        if self.page_header(ui, p) {
            self.start_background_scan();
        }

        let mut browse = false;
        let mut default_target = false;
        let mut open = false;
        let mut commit_target = false;
        theme::card_frame(*p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("归档位置").size(16.0).strong().color(p.text));
            ui.label(
                RichText::new("文件会被移动到该目录下的分类文件夹里")
                    .size(11.0)
                    .color(p.faint),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.config.target_dir)
                        .hint_text(default_target_hint())
                        .desired_width(390.0),
                );
                if response.lost_focus()
                    || (response.has_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                {
                    commit_target = true;
                }
                if widgets::secondary_button(ui, p, "浏览", 62.0).clicked() {
                    browse = true;
                }
                if widgets::secondary_button(ui, p, "恢复默认", 82.0).clicked() {
                    default_target = true;
                }
                if widgets::secondary_button(ui, p, "打开", 62.0).clicked() {
                    open = true;
                }
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new("留空时使用桌面上的 DeskOrg 文件夹；也可以用绝对路径放到其他磁盘。")
                    .size(10.0)
                    .color(p.faint),
            );
        });

        if browse {
            if let Some(folder) = rfd::FileDialog::new()
                .set_title("选择归档位置")
                .pick_folder()
            {
                self.config.target_dir = folder.to_string_lossy().to_string();
                commit_target = true;
            }
        }
        if default_target {
            self.config.target_dir.clear();
            commit_target = true;
        }
        if commit_target {
            self.save_config();
            self.rescan(false);
        }
        if open {
            self.open_target();
        }

        ui.add_space(14.0);

        let mut changed = false;
        theme::card_frame(*p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("扫描范围").size(16.0).strong().color(p.text));
            ui.label(
                RichText::new("公共桌面通常放着浏览器、播放器等应用的快捷方式")
                    .size(11.0)
                    .color(p.faint),
            );
            ui.add_space(2.0);
            changed |= widgets::toggle_row(
                ui,
                p,
                "同时整理公共桌面",
                "路径：C:\\Users\\Public\\Desktop（移动时可能需要管理员权限）",
                &mut self.config.include_public,
            );
        });
        if changed {
            self.save_config();
            self.rescan(false);
        }

        ui.add_space(14.0);

        theme::card_frame(*p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("手势与快捷键")
                    .size(16.0)
                    .strong()
                    .color(p.text),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                widgets::key_cap(ui, p, "Ctrl");
                ui.label(RichText::new("+").size(11.0).color(p.faint));
                widgets::key_cap(ui, p, "Alt");
                ui.label(RichText::new("+").size(11.0).color(p.faint));
                widgets::key_cap(ui, p, "D");
                ui.label(
                    RichText::new("全局切换桌面图标（DeskOrg 运行时有效）")
                        .size(11.5)
                        .color(p.muted),
                );
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new("双击桌面空白处同样可以切换；双击桌面上的文件或应用不会触发。")
                    .size(11.0)
                    .color(p.faint),
            );
        });

        ui.add_space(14.0);

        let mut auto_start_changed = false;
        let mut background_changed = false;
        let mut restart_elevated = false;
        let mut exit_clicked = false;
        theme::card_frame(*p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("启动与退出")
                    .size(16.0)
                    .strong()
                    .color(p.text),
            );
            ui.label(
                RichText::new("登录 Windows 或关闭主窗口后的行为")
                    .size(11.0)
                    .color(p.faint),
            );
            ui.add_space(2.0);
            auto_start_changed = widgets::toggle_row(
                ui,
                p,
                "开机自动启动",
                "登录 Windows 后在后台启动，并使用当前 exe 路径",
                &mut self.config.auto_start,
            );
            background_changed = widgets::toggle_row(
                ui,
                p,
                "关闭主窗口后继续在后台运行",
                "框框和快捷键继续工作；从右下角托盘图标可以重新打开控制中心",
                &mut self.config.close_to_background,
            );
            if !self.tray_available {
                ui.label(
                    RichText::new("当前会话托盘图标不可用，关闭窗口会直接退出。")
                        .size(10.0)
                        .color(p.danger),
                );
            }
            ui.add_space(6.0);
            if shell::is_elevated() {
                ui.label(
                    RichText::new("已以管理员身份运行")
                        .size(10.5)
                        .color(p.success),
                );
            } else {
                ui.label(
                    RichText::new(
                        "移动公共桌面或受保护目录需要管理员权限；重启后 DeskOrg 将以管理员身份运行。",
                    )
                    .size(10.0)
                    .color(p.faint),
                );
                if widgets::secondary_button(ui, p, "以管理员身份重启", 0.0).clicked() {
                    restart_elevated = true;
                }
            }
            ui.add_space(4.0);
            if widgets::secondary_button(ui, p, "退出 DeskOrg", 0.0).clicked() {
                exit_clicked = true;
            }
        });
        if auto_start_changed {
            if let Err(error) = shell::set_auto_start(self.config.auto_start) {
                self.config.auto_start = !self.config.auto_start;
                self.notify(error, NoticeKind::Error);
            }
        }
        if auto_start_changed || background_changed {
            self.save_config();
        }
        if restart_elevated {
            match shell::relaunch_elevated() {
                Ok(()) => {
                    self.force_quit = true;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Err(error) => self.notify(error, NoticeKind::Error),
            }
        }
        if exit_clicked {
            self.force_quit = true;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }

        ui.add_space(14.0);

        theme::card_frame(*p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("关于 DeskOrg").size(16.0).strong().color(p.text));
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "DeskOrg 是一个本地优先的 Windows 桌面整理工具。没有账号、没有云同步、没有后台服务；所有移动都在本机完成。",
                )
                .size(11.5)
                .color(p.muted),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new("发行者：陕西十方数联科技有限公司")
                    .size(11.0)
                    .color(p.text),
            );
            ui.label(
                RichText::new(format!("版本：{}", env!("CARGO_PKG_VERSION")))
                    .size(10.5)
                    .color(p.faint),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let _ = widgets::pill(ui, p, "Rust + egui", p.accent_ink);
                let _ = widgets::pill(ui, p, "默认无需管理员", p.success);
                let _ = widgets::pill(ui, p, "可撤销", p.warning);
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!(
                    "配置文件：{}",
                    crate::config::config_path().display()
                ))
                .size(10.0)
                .color(p.faint),
            );
        });
    }

    fn confirm_modal(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(kind) = self.confirm else {
            return;
        };
        let mut decision: Option<bool> = None;
        let target = self.target();
        let target_text = target.to_string_lossy().to_string();
        let mut make_fences = self.confirm_make_fences;
        let mut do_archive = self.confirm_archive;

        egui::Modal::new(Id::unique("deskorg-confirm"))
            .frame(theme::card_frame(*p).inner_margin(egui::Margin::same(22)))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                let (title, body, action) = match kind {
                    Confirm::Organize { count, .. } => (
                        format!("准备整理 {count} 个项目"),
                        "勾选“同时归档文件”会把选中文件移动到 DeskOrg/<分类>/；取消勾选则只生成框框，文件保持原位。".to_owned(),
                        "开始整理",
                    ),
                    Confirm::Undo { count } => (
                        "撤销上次整理？".to_owned(),
                        format!(
                            "会尝试把 {count} 个项目从分类文件夹放回原桌面位置。\n如果桌面已有同名文件，会自动加序号，不会覆盖。"
                        ),
                        "放回桌面",
                    ),
                };
                ui.label(RichText::new(title).size(19.0).strong().color(p.text));
                ui.add_space(4.0);
                ui.label(RichText::new(body).size(12.0).color(p.muted));
                if matches!(kind, Confirm::Organize { .. }) {
                    ui.add_space(10.0);
                    ui.checkbox(
                        &mut make_fences,
                        "生成透明桌面框框（整理完成后隐藏原生图标）",
                    );
                    ui.add_space(4.0);
                    ui.checkbox(
                        &mut do_archive,
                        "同时归档文件到 DeskOrg/<分类>/（快捷方式保留在桌面，可撤销）",
                    );
                    ui.label(
                        RichText::new(format!(
                            "归档目标：\n{target_text}"
                        ))
                        .size(10.0)
                        .color(p.faint),
                    );
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if widgets::secondary_button(ui, p, "取消", 92.0).clicked() {
                        decision = Some(false);
                    }
                    if widgets::primary_button(ui, p, action, 120.0).clicked() {
                        decision = Some(true);
                    }
                });
            });

        if matches!(kind, Confirm::Organize { .. }) {
            self.confirm_make_fences = make_fences;
            self.confirm_archive = do_archive;
            if self.config.fences_after_organize != make_fences {
                self.config.fences_after_organize = make_fences;
                self.config_dirty = true;
            }
        }
        match decision {
            Some(true) => {
                self.confirm = None;
                match kind {
                    Confirm::Organize { .. } => self.perform_organize(ctx, do_archive),
                    Confirm::Undo { .. } => self.perform_undo(),
                }
            }
            Some(false) => self.confirm = None,
            None => {}
        }
    }

    fn job_overlay(&self, ctx: &egui::Context, p: &Palette) {
        let Some(job) = &self.job else {
            return;
        };
        let done = job.progress.load(Ordering::Relaxed).min(job.total);
        let fraction = if job.total == 0 {
            0.0
        } else {
            done as f32 / job.total as f32
        };
        egui::Modal::new(Id::unique("deskorg-job"))
            .backdrop_color(Color32::from_black_alpha(60))
            .frame(theme::card_frame(*p).inner_margin(egui::Margin::same(22)))
            .show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new(&job.label).size(18.0).strong().color(p.text));
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "{done} / {} 项 · 已用时 {} 秒",
                        job.total,
                        job.started.elapsed().as_secs()
                    ))
                    .size(11.0)
                    .color(p.muted),
                );
                ui.add_space(6.0);
                ui.add(egui::ProgressBar::new(fraction).show_percentage());
                ui.add_space(6.0);
                if job.cancel.load(Ordering::Relaxed) {
                    ui.label(
                        RichText::new("正在停止…当前文件完成后生效")
                            .size(10.0)
                            .color(p.warning),
                    );
                } else if widgets::secondary_button(ui, p, "停止", 92.0).clicked() {
                    job.cancel.store(true, Ordering::Relaxed);
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new("正在后台处理；移动文件期间请勿强制结束进程。")
                        .size(10.0)
                        .color(p.faint),
                );
            });
    }

    fn toast(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(notice) = &self.notice else {
            return;
        };
        let (color, glyph) = match notice.kind {
            NoticeKind::Success => (p.success, "✓"),
            NoticeKind::Info => (p.accent_ink, "i"),
            NoticeKind::Error => (p.danger, "!"),
        };
        let text = notice.text.clone();

        egui::Area::new(Id::unique("deskorg-toast"))
            .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -18.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                widgets::toast_frame(p).show(ui, |ui| {
                    ui.set_max_width(560.0);
                    ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::new(20.0, 20.0), Sense::hover());
                        ui.painter()
                            .circle_filled(rect.center(), 10.0, with_alpha(color, 28));
                        ui.painter().text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            glyph,
                            FontId::proportional(11.0),
                            color,
                        );
                        ui.label(RichText::new(text).size(12.0).color(p.text));
                    });
                });
            });
    }
}

impl eframe::App for DeskOrgApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if ui.ctx().input(|input| input.viewport().close_requested())
            && self.config.close_to_background
            && self.tray_available
            && !self.force_quit
        {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide_frames = 12;
            shell::hide_main_window();
        }

        self.poll(ui.ctx());
        self.ensure_window_theme(frame);
        if self.hide_frames > 0 {
            // Hide for a few frames: eframe can briefly re-show the root on its
            // first paint, and the taskbar button must not linger.
            shell::hide_main_window();
            self.hide_frames -= 1;
            ui.ctx().request_repaint();
        }

        let p = self.palette();
        widgets::draw_background(ui, &p);

        egui::Panel::left("deskorg-sidebar")
            .exact_size(228.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(p.sidebar)
                    .inner_margin(egui::Margin {
                        left: 18,
                        right: 14,
                        top: 20,
                        bottom: 14,
                    }),
            )
            .show(ui, |ui| {
                self.sidebar(ui, &p);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(egui::Margin {
                        left: 28,
                        right: 50,
                        top: 20,
                        bottom: 18,
                    }),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(("deskorg-page", self.page as u8))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        match self.page {
                            Page::Overview => self.show_overview(ui, &p),
                            Page::Organize => self.show_organize(ui, &p),
                            Page::Fences => self.show_fences_page(ui, &p),
                            Page::Beauty => self.show_beauty(ui, &p),
                            Page::Settings => self.show_settings(ui, &p),
                        }
                        ui.add_space(22.0);
                    });
            });

        self.new_fence_modal(ui.ctx(), &p);
        self.confirm_modal(ui.ctx(), &p);
        self.job_overlay(ui.ctx(), &p);
        if self.config.fences_enabled || self.config.close_to_background || self.start_background {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        }
        self.show_fence_viewports(ui.ctx());
        self.toast(ui.ctx(), &p);
        self.flush_config_if_due();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // 退出前把防抖中的配置改动落盘，避免拖完滑杆立刻退出时丢设置。
        if self.config_dirty {
            let _ = self.config.save();
        }
        // Never move files back automatically on exit: that would silently undo
        // the user's organization. Only restore icons this process hid.
        if self.hidden_by_app && self.config.restore_on_exit {
            let _ = shell::set_desktop_icons_visible(true);
        }
        shell::stop_tray();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }
}

fn fence_viewport_id(id: u64) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("deskorg-fence", id))
}

fn fence_viewport_builder(fence: &Fence, include_geometry: bool) -> egui::ViewportBuilder {
    let mut builder = egui::ViewportBuilder::default()
        .with_title(fence.window_title())
        .with_min_inner_size([240.0, 160.0])
        .with_decorations(false)
        .with_resizable(false)
        .with_transparent(true)
        .with_taskbar(false)
        .with_active(false)
        .with_visible(true)
        // 框框属于桌面层：默认沉到普通窗口下面，避免一打开 DeskOrg
        // 就盖住正在使用的其他应用。
        .with_window_level(egui::WindowLevel::AlwaysOnBottom);
    if include_geometry {
        builder = builder
            .with_position([fence.x, fence.y])
            .with_inner_size([fence.w, fence.h]);
    }
    builder
}

fn fence_card(
    ui: &mut Ui,
    p: &Palette,
    fence: &mut Fence,
    data: &FenceData,
    showing: bool,
) -> (bool, bool) {
    let mut open = false;
    let mut delete = false;
    theme::card_frame(*p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(184.0, 140.0),
                Layout::top_down(Align::Min),
                |ui| {
                    fences::preview(ui, fence, data);
                    ui.add_space(2.0);
                    let (label, color) = if showing {
                        ("显示中", p.success)
                    } else {
                        ("已全部隐藏", p.faint)
                    };
                    let _ = widgets::pill(ui, p, label, color);
                },
            );
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    widgets::category_badge(ui, p, fence.category);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(&fence.title)
                                .size(15.0)
                                .strong()
                                .color(p.text),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{} 项 · {} 项已归档 · {}",
                                data.items.len(),
                                data.archived,
                                fence.theme.label()
                            ))
                            .size(10.0)
                            .color(p.faint),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(RichText::new("删除").size(11.5).color(p.danger))
                                    .frame(false),
                            )
                            .clicked()
                        {
                            delete = true;
                        }
                    });
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("主题").size(11.0).color(p.muted));
                    for theme in FenceTheme::ALL {
                        if fences::theme_chip(ui, theme, fence.theme == theme).clicked() {
                            fence.theme = theme;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("透明度").size(11.0).color(p.muted));
                    ui.spacing_mut().slider_width = 130.0;
                    ui.add(
                        egui::Slider::new(&mut fence.opacity, 0.0..=0.8)
                            .show_value(false)
                            .trailing_fill(true),
                    );
                    ui.label(
                        RichText::new(format!("{:.0}%", fence.opacity * 100.0))
                            .size(10.5)
                            .color(p.faint),
                    );
                    ui.add_space(8.0);
                    ui.checkbox(&mut fence.blur, "毛玻璃");
                });
                ui.horizontal(|ui| {
                    if widgets::secondary_button(ui, p, "打开分类文件夹", 126.0).clicked() {
                        open = true;
                    }
                    ui.label(
                        RichText::new(if fence.blur {
                            "毛玻璃会采样并模糊当前壁纸"
                        } else {
                            "背景完全透明，只保留边框"
                        })
                        .size(9.5)
                        .color(p.faint),
                    );
                });
            });
        });
    });
    (open, delete)
}

fn load_frost_texture(ctx: &egui::Context, path: &str) -> Result<egui::TextureHandle, String> {
    if path.trim().is_empty() {
        return Err("没有系统壁纸".to_owned());
    }
    let image = image::open(path).map_err(|error| format!("无法读取壁纸：{error}"))?;
    let thumbnail = image.thumbnail(480, 300);
    let rgba = thumbnail.to_rgba8();
    let blurred = image::imageops::blur(&rgba, 5.0);
    let size = [blurred.width() as usize, blurred.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, blurred.as_raw());
    Ok(ctx.load_texture("deskorg-frost", color, egui::TextureOptions::LINEAR))
}

fn stat_tile(ui: &mut Ui, p: &Palette, value: &str, label: &str, tint: Color32) {
    theme::soft_frame(*p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(58.0);
        ui.label(RichText::new(value).size(21.0).strong().color(tint));
        ui.label(RichText::new(label).size(10.5).color(p.faint));
    });
}

fn category_tile(ui: &mut Ui, p: &Palette, category: Category, count: usize) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 88.0), Sense::click());
    let color = theme::category_color(category);
    let ink = theme::category_ink(category, p.light);
    let fill = if response.hovered() {
        with_alpha(color, if p.light { 18 } else { 22 })
    } else {
        p.panel
    };
    ui.painter().rect_filled(rect, 14, fill);
    ui.painter().rect_stroke(
        rect,
        14,
        Stroke::new(1.0, if response.hovered() { ink } else { p.border }),
        egui::StrokeKind::Inside,
    );
    let badge = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 24.0, rect.top() + 25.0),
        Vec2::splat(28.0),
    );
    ui.painter()
        .rect_filled(badge, 9, with_alpha(color, if p.light { 30 } else { 34 }));
    ui.painter().text(
        badge.center(),
        Align2::CENTER_CENTER,
        category.glyph(),
        FontId::proportional(12.0),
        ink,
    );
    ui.painter().text(
        egui::pos2(rect.right() - 14.0, rect.top() + 21.0),
        Align2::RIGHT_CENTER,
        count.to_string(),
        FontId::proportional(18.0),
        p.text,
    );
    ui.painter().text(
        egui::pos2(rect.left() + 12.0, rect.bottom() - 17.0),
        Align2::LEFT_CENTER,
        category.label(),
        FontId::proportional(11.5),
        p.muted,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn filter_chip(
    ui: &mut Ui,
    p: &Palette,
    label: &str,
    count: usize,
    color: Color32,
    selected: bool,
) -> egui::Response {
    let text = format!("{label} {count}");
    let galley = ui
        .painter()
        .layout_no_wrap(text, FontId::proportional(11.5), p.text);
    let desired = Vec2::new(galley.size().x + 24.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
    let ink = if p.light {
        theme::blend(color, Color32::BLACK, 0.38)
    } else {
        color
    };
    let fill = if selected {
        with_alpha(color, if p.light { 34 } else { 38 })
    } else if response.hovered() {
        p.panel_soft
    } else {
        Color32::TRANSPARENT
    };
    if fill.a() > 0 {
        ui.painter().rect_filled(rect, 14, fill);
    }
    ui.painter().rect_stroke(
        rect,
        14,
        Stroke::new(1.0, if selected { ink } else { p.border }),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0),
        galley,
        if selected { ink } else { p.muted },
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn recent_items(items: &[DesktopItem]) -> Vec<DesktopItem> {
    let mut recent: Vec<&DesktopItem> = items.iter().collect();
    recent.sort_by_key(|item| Reverse(item.modified));
    recent.into_iter().take(5).cloned().collect()
}

fn filtered_items<'a>(
    items: &'a [DesktopItem],
    category_filter: Option<Category>,
    search: &str,
) -> Vec<&'a DesktopItem> {
    let search = search.trim().to_lowercase();
    items
        .iter()
        .filter(|item| {
            category_filter
                .map(|category| category == item.category)
                .unwrap_or(true)
                && (search.is_empty() || item.name.to_lowercase().contains(&search))
        })
        .collect()
}

fn item_row(ui: &mut Ui, p: &Palette, item: &DesktopItem, with_checkbox: bool, checked: &mut bool) {
    ui.horizontal(|ui| {
        if with_checkbox {
            ui.add(egui::Checkbox::without_text(checked));
        }
        widgets::category_badge(ui, p, item.category);
        ui.vertical(|ui| {
            let response = ui.add(
                egui::Label::new(RichText::new(&item.name).size(13.0).color(p.text)).truncate(),
            );
            if response.double_clicked() {
                let _ = shell::open_in_explorer(&item.path);
            }
            let detail = if item.is_dir {
                format!("{} · {}", item.category.label(), item.origin.label())
            } else {
                format!(
                    "{} · {} · {}",
                    item.category.label(),
                    item.extension().to_uppercase(),
                    item.origin.label()
                )
            };
            ui.label(RichText::new(detail).size(10.0).color(p.faint));
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let size = if item.is_dir {
                "—".to_owned()
            } else {
                desktop::format_size(item.size)
            };
            ui.label(RichText::new(size).size(10.5).color(p.muted));
        });
    });
    let (line, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(
        line,
        0,
        with_alpha(p.border, if p.light { 130 } else { 90 }),
    );
}

fn segmented(ui: &mut Ui, p: &Palette, label: &str, selected: bool) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), FontId::proportional(11.5), p.text);
    let desired = Vec2::new(galley.size().x + 24.0, 30.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
    let fill = if selected {
        p.accent_soft
    } else {
        p.panel_soft
    };
    ui.painter().rect_filled(rect, 9, fill);
    ui.painter().rect_stroke(
        rect,
        9,
        Stroke::new(1.0, if selected { p.accent } else { p.border }),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0),
        galley,
        if selected { p.accent_ink } else { p.muted },
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn wallpaper_placeholder(ui: &mut Ui, p: &Palette, size: Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), with_alpha(p.accent, 40));
    mesh.colored_vertex(rect.right_top(), with_alpha(p.accent, 12));
    mesh.colored_vertex(rect.right_bottom(), p.panel_soft);
    mesh.colored_vertex(rect.left_bottom(), p.panel_soft);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(egui::Shape::mesh(mesh));
    ui.painter().rect_stroke(
        rect,
        14,
        Stroke::new(1.0, p.border),
        egui::StrokeKind::Inside,
    );
    let center = rect.center();
    ui.painter().circle_filled(
        center + Vec2::new(-12.0, -6.0),
        27.0,
        with_alpha(p.accent, 120),
    );
    ui.painter().circle_filled(
        center + Vec2::new(2.0, -14.0),
        24.0,
        with_alpha(p.bg_bottom, 220),
    );
    ui.painter().text(
        center + Vec2::new(0.0, 44.0),
        Align2::CENTER_CENTER,
        "还没有选择壁纸",
        FontId::proportional(12.0),
        p.muted,
    );
}

fn default_target_hint() -> String {
    crate::config::default_target_dir()
        .to_string_lossy()
        .to_string()
}

fn load_wallpaper_texture(ctx: &egui::Context, path: &str) -> Result<egui::TextureHandle, String> {
    if path.trim().is_empty() {
        return Err("还没有选择壁纸".to_owned());
    }
    let image = image::open(path).map_err(|error| format!("无法读取图片：{error}"))?;
    let thumbnail = image.thumbnail(1600, 900);
    let rgba = thumbnail.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Ok(ctx.load_texture("deskorg-wallpaper", image, egui::TextureOptions::LINEAR))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
