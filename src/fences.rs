//! Desktop fences: free-floating, translucent category boxes rendered as
//! separate egui viewports.
//!
//! A fence never moves or deletes anything. It is a view over the desktop items
//! of one category plus the files already archived in `DeskOrg/<category>`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, Rect, Sense, Stroke,
    StrokeKind, Ui, UiBuilder, Vec2,
};
use serde::{Deserialize, Serialize};

use crate::desktop::{self, Category, DesktopItem};
use crate::shell;
use crate::theme::{self, with_alpha};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FenceTheme {
    Frost,
    Mint,
    Sunset,
    Ink,
    Sakura,
    Ocean,
}

impl FenceTheme {
    pub const ALL: [Self; 6] = [
        Self::Frost,
        Self::Mint,
        Self::Sunset,
        Self::Ink,
        Self::Sakura,
        Self::Ocean,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Frost => "霜白",
            Self::Mint => "薄荷",
            Self::Sunset => "落日",
            Self::Ink => "墨黑",
            Self::Sakura => "樱花",
            Self::Ocean => "深海",
        }
    }

    pub const fn accent(self) -> Color32 {
        match self {
            Self::Frost => Color32::from_rgb(226, 232, 240),
            Self::Mint => Color32::from_rgb(94, 234, 212),
            Self::Sunset => Color32::from_rgb(251, 146, 60),
            Self::Ink => Color32::from_rgb(203, 213, 225),
            Self::Sakura => Color32::from_rgb(244, 114, 182),
            Self::Ocean => Color32::from_rgb(96, 165, 250),
        }
    }

    pub const fn base(self) -> Color32 {
        match self {
            Self::Frost => Color32::from_rgb(20, 26, 38),
            Self::Mint => Color32::from_rgb(8, 32, 30),
            Self::Sunset => Color32::from_rgb(38, 22, 22),
            Self::Ink => Color32::from_rgb(15, 16, 20),
            Self::Sakura => Color32::from_rgb(38, 20, 31),
            Self::Ocean => Color32::from_rgb(10, 26, 44),
        }
    }

    pub const fn text(self) -> Color32 {
        Color32::from_rgb(244, 248, 255)
    }

    pub const fn muted(self) -> Color32 {
        Color32::from_rgb(170, 184, 204)
    }

    pub fn default_for(category: Category) -> Self {
        match category {
            Category::Image => Self::Mint,
            Category::Document => Self::Ocean,
            Category::Video => Self::Sakura,
            Category::Audio => Self::Sunset,
            Category::Archive => Self::Ink,
            Category::Installer => Self::Sunset,
            Category::Code => Self::Mint,
            Category::Folder => Self::Frost,
            Category::Shortcut => Self::Sakura,
            Category::Other => Self::Ink,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fence {
    pub id: u64,
    pub category: Category,
    pub title: String,
    pub theme: FenceTheme,
    /// 0.0 = fully transparent, 0.8 = quite solid.
    pub opacity: f32,
    pub blur: bool,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Fence {
    pub fn new(id: u64, category: Category, index: usize, desktop: Vec2, origin: Vec2) -> Self {
        let (x, y) = auto_position(index, desktop, origin);
        Self {
            id,
            category,
            title: category.label().to_owned(),
            theme: FenceTheme::default_for(category),
            opacity: 0.08,
            blur: false,
            x,
            y,
            w: FENCE_WIDTH,
            h: FENCE_HEIGHT,
        }
    }

    pub fn window_title(&self) -> String {
        format!("DeskOrgFence-{}", self.id)
    }

    pub fn clamp_size(&mut self) {
        self.w = self.w.clamp(240.0, 620.0);
        self.h = self.h.clamp(160.0, 620.0);
        self.x = self.x.max(-self.w + 80.0);
        self.y = self.y.max(-20.0);
        self.opacity = self.opacity.clamp(0.0, 0.8);
    }
}

pub const FENCE_WIDTH: f32 = 300.0;
pub const FENCE_HEIGHT: f32 = 216.0;
const GAP: f32 = 16.0;
const MARGIN: f32 = 40.0;

pub fn auto_position(index: usize, desktop: Vec2, origin: Vec2) -> (f32, f32) {
    let width = if desktop.x > 0.0 { desktop.x } else { 1920.0 };
    let usable = (width - MARGIN * 2.0 + GAP).max(FENCE_WIDTH + GAP);
    let columns = ((usable / (FENCE_WIDTH + GAP)).floor() as usize).clamp(2, 5);
    let col = index % columns;
    let row = index / columns;
    (
        origin.x + MARGIN + col as f32 * (FENCE_WIDTH + GAP),
        origin.y + MARGIN + row as f32 * (FENCE_HEIGHT + GAP),
    )
}

#[derive(Clone, Debug)]
pub struct FenceItem {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub archived: bool,
    pub category: Category,
}

impl FenceItem {
    pub fn extension(&self) -> String {
        self.path
            .extension()
            .map(|ext| ext.to_string_lossy().to_uppercase())
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default)]
pub struct FenceData {
    pub items: Vec<FenceItem>,
    pub archived: usize,
}

impl FenceData {
    pub fn len(&self) -> usize {
        self.items.len()
    }
}

pub fn collect_data(desktop_items: &[DesktopItem], target: &Path, category: Category) -> FenceData {
    let mut data = FenceData::default();
    let mut seen = HashSet::new();

    for item in desktop_items
        .iter()
        .filter(|item| item.category == category)
    {
        let key = item.path.to_string_lossy().to_lowercase();
        if seen.insert(key) {
            data.items.push(FenceItem {
                path: item.path.clone(),
                name: item.name.clone(),
                is_dir: item.is_dir,
                size: item.size,
                archived: false,
                category,
            });
        }
    }

    let category_dir = target.join(category.folder_name());
    if let Ok(read_dir) = std::fs::read_dir(&category_dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.eq_ignore_ascii_case("desktop.ini") || name.eq_ignore_ascii_case("thumbs.db") {
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink() || is_hidden_or_system(&metadata) {
                continue;
            }
            let key = path.to_string_lossy().to_lowercase();
            if !seen.insert(key) {
                continue;
            }
            data.archived += 1;
            data.items.push(FenceItem {
                path,
                name,
                is_dir: metadata.is_dir(),
                size: if metadata.is_dir() { 0 } else { metadata.len() },
                archived: true,
                category,
            });
        }
    }

    data.items.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    data
}

#[cfg(windows)]
fn is_hidden_or_system(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
    metadata.file_attributes() & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0
}

#[cfg(not(windows))]
fn is_hidden_or_system(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FenceAction {
    Delete(u64),
    OpenFolder(u64),
}

/// Render one fence inside its own transparent viewport.
pub fn draw_fence(
    ui: &mut Ui,
    fence: &Fence,
    data: &FenceData,
    frost: Option<&egui::TextureHandle>,
    icons: &std::collections::HashMap<PathBuf, Option<egui::TextureHandle>>,
    desktop: Rect,
) -> Option<FenceAction> {
    let full = ui.max_rect();
    if full.width() < 80.0 || full.height() < 60.0 {
        return None;
    }
    let panel = full.shrink(1.0);
    let palette = FencePalette::new(fence.theme);
    let mut action = None;

    if ui.input(|i| i.viewport().close_requested()) {
        return Some(FenceAction::Delete(fence.id));
    }

    // Frosted wallpaper crop.
    let mut painted_frost = false;
    if fence.blur {
        if let Some(texture) = frost {
            let uv = frost_uv(fence, desktop);
            let image = egui::Image::new(egui::load::SizedTexture::new(texture.id(), panel.size()))
                .uv(uv)
                .corner_radius(CornerRadius::same(18));
            image.paint_at(ui, panel);
            painted_frost = true;
        }
    }

    if painted_frost {
        // A slight dark veil keeps white text readable without killing the blur.
        ui.painter()
            .rect_filled(panel, 18, Color32::from_rgba_unmultiplied(5, 10, 18, 42));
    } else if fence.blur {
        // No wallpaper available: a soft mesh still reads as frosted glass.
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(panel.left_top(), with_alpha(palette.base, 210));
        mesh.colored_vertex(panel.right_top(), with_alpha(palette.accent, 36));
        mesh.colored_vertex(panel.right_bottom(), with_alpha(palette.base, 210));
        mesh.colored_vertex(panel.left_bottom(), with_alpha(palette.base, 150));
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        ui.painter().add(egui::Shape::mesh(mesh));
    }

    if fence.opacity > 0.001 {
        let alpha = (fence.opacity.clamp(0.0, 0.8) * 255.0).round() as u8;
        ui.painter()
            .rect_filled(panel, 18, with_alpha(palette.base, alpha));
    }

    // Border and a very subtle top highlight.
    ui.painter().rect_stroke(
        panel,
        18,
        Stroke::new(1.0, with_alpha(palette.accent, 150)),
        StrokeKind::Inside,
    );
    let highlight = Rect::from_min_size(
        Pos2::new(panel.left() + 18.0, panel.top() + 1.0),
        Vec2::new((panel.width() - 36.0).max(1.0), 1.0),
    );
    ui.painter()
        .rect_filled(highlight, 1, with_alpha(Color32::WHITE, 50));

    // ── Header ──────────────────────────────────────────────────────────
    let header = Rect::from_min_size(
        Pos2::new(panel.left() + 12.0, panel.top() + 10.0),
        Vec2::new((panel.width() - 24.0).max(10.0), 34.0),
    );
    // Keep the title readable even on a bright wallpaper, without making the
    // whole fence opaque.
    ui.painter().rect_filled(
        header.expand2(Vec2::new(5.0, 4.0)),
        11,
        with_alpha(palette.base, 145),
    );
    let close_rect = Rect::from_center_size(
        Pos2::new(header.right() - 12.0, header.center().y),
        Vec2::splat(24.0),
    );
    let folder_rect = close_rect.translate(Vec2::new(-28.0, 0.0));
    let drag_rect = Rect::from_min_max(
        header.min,
        Pos2::new(folder_rect.left() - 8.0, header.max.y),
    );

    let drag_response = ui.interact(
        drag_rect,
        ui.scope_id().with(("fence-drag", fence.id)),
        Sense::click_and_drag(),
    );
    if drag_response.drag_started() {
        // Let Windows run its native move loop. Sending OuterPosition per frame
        // fights the compositor and makes transparent fences flicker.
        crate::shell::start_window_drag(&fence.window_title());
    }
    if drag_response.hovered() || drag_response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }

    let badge = Rect::from_center_size(
        Pos2::new(header.left() + 14.0, header.center().y),
        Vec2::splat(28.0),
    );
    ui.painter().rect_filled(
        badge,
        9,
        with_alpha(theme::category_color(fence.category), 56),
    );
    ui.painter().text(
        badge.center(),
        Align2::CENTER_CENTER,
        fence.category.glyph(),
        FontId::proportional(12.0),
        theme::category_ink(fence.category, false),
    );

    let title_pos = Pos2::new(badge.right() + 9.0, header.center().y);
    ui.painter().text(
        title_pos,
        Align2::LEFT_CENTER,
        &fence.title,
        FontId::proportional(14.0),
        palette.text,
    );
    let count_text = format!("{}", data.items.len());
    ui.painter().text(
        Pos2::new(folder_rect.left() - 8.0, header.center().y),
        Align2::RIGHT_CENTER,
        &count_text,
        FontId::proportional(10.5),
        with_alpha(palette.text, 190),
    );

    let folder_response = ui.interact(
        folder_rect,
        ui.scope_id().with(("fence-folder", fence.id)),
        Sense::click(),
    );
    let close_response = ui.interact(
        close_rect,
        ui.scope_id().with(("fence-close", fence.id)),
        Sense::click(),
    );
    if folder_response.clicked() {
        action = Some(FenceAction::OpenFolder(fence.id));
    }
    if close_response.clicked() {
        action = Some(FenceAction::Delete(fence.id));
    }
    if folder_response.hovered() || close_response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().text(
        folder_rect.center(),
        Align2::CENTER_CENTER,
        "▸",
        FontId::proportional(13.0),
        with_alpha(palette.text, 150),
    );
    let close_hover = close_response.hovered();
    draw_close(
        ui,
        close_rect,
        if close_hover {
            Color32::from_rgb(248, 113, 113)
        } else {
            with_alpha(palette.text, 120)
        },
    );

    // ── Body ────────────────────────────────────────────────────────────
    let body = Rect::from_min_max(
        Pos2::new(panel.left() + 12.0, header.bottom() + 6.0),
        Pos2::new(panel.right() - 12.0, panel.bottom() - 12.0),
    );
    if body.height() > 20.0 {
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(body)
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                ui.set_clip_rect(body);
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                if data.items.is_empty() {
                    empty_fence(ui, body, &palette);
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt(("fence-scroll", fence.id))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(body.width());
                            ui.horizontal_wrapped(|ui| {
                                for item in &data.items {
                                    item_tile(ui, item, &palette, icons);
                                }
                            });
                        });
                }
            },
        );
    }

    // ── Resize handle ───────────────────────────────────────────────────
    let resize_rect = Rect::from_min_size(
        Pos2::new(panel.right() - 18.0, panel.bottom() - 18.0),
        Vec2::splat(16.0),
    );
    let resize = ui.interact(
        resize_rect,
        ui.scope_id().with(("fence-resize", fence.id)),
        Sense::drag(),
    );
    if resize.hovered() || resize.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
    }
    if resize.dragged() {
        let delta = ui.input(|i| i.pointer.delta());
        let current = ui.ctx().input(|i| {
            i.viewport()
                .inner_rect
                .map(|rect| rect.size())
                .unwrap_or(Vec2::new(fence.w, fence.h))
        });
        let new_size = (current + delta).max(Vec2::new(240.0, 160.0));
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::InnerSize(new_size));
    }
    ui.painter().line_segment(
        [
            Pos2::new(resize_rect.right() - 3.0, resize_rect.bottom() - 9.0),
            Pos2::new(resize_rect.right() - 9.0, resize_rect.bottom() - 3.0),
        ],
        Stroke::new(1.2, with_alpha(palette.text, 90)),
    );
    ui.painter().line_segment(
        [
            Pos2::new(resize_rect.right() - 3.0, resize_rect.bottom() - 15.0),
            Pos2::new(resize_rect.right() - 15.0, resize_rect.bottom() - 3.0),
        ],
        Stroke::new(1.2, with_alpha(palette.text, 60)),
    );

    action
}

fn item_tile(
    ui: &mut Ui,
    item: &FenceItem,
    palette: &FencePalette,
    icons: &std::collections::HashMap<PathBuf, Option<egui::TextureHandle>>,
) {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(88.0, 80.0), Sense::click());

    let icon_rect = Rect::from_center_size(
        Pos2::new(rect.center().x, rect.top() + 25.0),
        Vec2::splat(32.0),
    );
    let cat_color = theme::category_color(item.category);
    if let Some(texture) = icons.get(&item.path).and_then(|texture| texture.as_ref()) {
        ui.painter().image(
            texture.id(),
            icon_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        let badge = Rect::from_center_size(icon_rect.center(), Vec2::splat(30.0));
        ui.painter()
            .rect_filled(badge, 9, with_alpha(cat_color, 95));
        let badge_text = if item.is_dir {
            item.category.glyph().to_owned()
        } else {
            let ext = item.extension();
            if ext.is_empty() {
                item.category.glyph().to_owned()
            } else if ext.chars().count() > 4 {
                ext.chars().take(4).collect()
            } else {
                ext
            }
        };
        ui.painter().text(
            badge.center(),
            Align2::CENTER_CENTER,
            badge_text,
            FontId::proportional(if item.is_dir { 12.0 } else { 8.5 }),
            Color32::WHITE,
        );
    }
    if item.archived {
        ui.painter().circle_filled(
            icon_rect.right_top() + Vec2::new(-1.0, 1.0),
            3.0,
            palette.accent,
        );
    }

    let clip = ui.painter().with_clip_rect(rect.shrink(4.0));
    let label = label_name(item);
    let max_chars = if label.is_ascii() { 11 } else { 6 };
    let display = short_name(&label, max_chars);
    let galley = ui
        .painter()
        .layout_no_wrap(display, FontId::proportional(12.0), Color32::WHITE);
    clip.galley(
        Pos2::new(
            (rect.center().x - galley.size().x * 0.5).round(),
            (rect.top() + 46.0).round(),
        ),
        galley,
        Color32::WHITE,
    );

    let detail = if item.is_dir {
        "文件夹".to_owned()
    } else {
        desktop::format_size(item.size)
    };
    let response = response.on_hover_text(format!(
        "{}\n{detail} · {}",
        item.path.display(),
        if item.archived {
            "已归档"
        } else {
            "桌面上"
        }
    ));
    if response.double_clicked() {
        let _ = shell::open_in_explorer(&item.path);
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // 右键弹出 Windows 资源管理器同款原生菜单。
    if response.secondary_clicked() {
        let _ = shell::show_shell_context_menu(&item.path);
    }
}

fn empty_fence(ui: &mut Ui, body: Rect, palette: &FencePalette) {
    ui.painter().text(
        body.center(),
        Align2::CENTER_CENTER,
        "空",
        FontId::proportional(22.0),
        with_alpha(palette.text, 80),
    );
    ui.painter().text(
        body.center() + Vec2::new(0.0, 26.0),
        Align2::CENTER_CENTER,
        "把文件放进分类文件夹，或点右上角打开",
        FontId::proportional(9.0),
        with_alpha(palette.muted, 160),
    );
}

fn draw_close(ui: &Ui, rect: Rect, color: Color32) {
    let c = rect.center();
    let r = 4.5;
    let stroke = Stroke::new(1.2, color);
    ui.painter()
        .line_segment([c + Vec2::new(-r, -r), c + Vec2::new(r, r)], stroke);
    ui.painter()
        .line_segment([c + Vec2::new(r, -r), c + Vec2::new(-r, r)], stroke);
}

fn frost_uv(fence: &Fence, desktop: Rect) -> Rect {
    let w = desktop.width().max(1.0);
    let h = desktop.height().max(1.0);
    let x0 = ((fence.x - desktop.min.x) / w).clamp(0.0, 1.0);
    let y0 = ((fence.y - desktop.min.y) / h).clamp(0.0, 1.0);
    let x1 = ((fence.x + fence.w - desktop.min.x) / w).clamp(0.0, 1.0);
    let y1 = ((fence.y + fence.h - desktop.min.y) / h).clamp(0.0, 1.0);
    Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1))
}

pub struct FencePalette {
    pub base: Color32,
    pub accent: Color32,
    pub text: Color32,
    pub muted: Color32,
}

impl FencePalette {
    pub fn new(theme: FenceTheme) -> Self {
        Self {
            base: theme.base(),
            accent: theme.accent(),
            text: theme.text(),
            muted: theme.muted(),
        }
    }
}

/// Small preview used by the fence management page.
pub fn preview(ui: &mut Ui, fence: &Fence, data: &FenceData) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 122.0), Sense::hover());
    let palette = FencePalette::new(fence.theme);
    ui.painter()
        .rect_filled(rect, 14, with_alpha(palette.base, 235));
    ui.painter().rect_stroke(
        rect,
        14,
        Stroke::new(1.0, with_alpha(palette.accent, 150)),
        StrokeKind::Inside,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 14.0, rect.top() + 18.0),
        Align2::LEFT_CENTER,
        fence.category.glyph(),
        FontId::proportional(12.0),
        palette.accent,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 34.0, rect.top() + 18.0),
        Align2::LEFT_CENTER,
        &fence.title,
        FontId::proportional(12.0),
        palette.text,
    );
    for index in 0..4 {
        let x = rect.left() + 14.0 + index as f32 * 38.0;
        let tile = Rect::from_min_size(Pos2::new(x, rect.top() + 38.0), Vec2::new(32.0, 32.0));
        ui.painter()
            .rect_filled(tile, 8, with_alpha(palette.accent, 30));
    }
    ui.painter().text(
        Pos2::new(rect.left() + 14.0, rect.bottom() - 15.0),
        Align2::LEFT_CENTER,
        format!("{} 项", data.items.len()),
        FontId::proportional(9.5),
        with_alpha(palette.muted, 220),
    );
}

#[allow(dead_code)]
pub fn category_summary(items: &[DesktopItem]) -> String {
    let counts = desktop::count_by_category(items);
    counts
        .iter()
        .filter(|(_, count)| **count > 0)
        .map(|(category, count)| format!("{} {count}", category.label()))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Theme helper for UI chips.
pub fn theme_chip(ui: &mut Ui, theme: FenceTheme, selected: bool) -> egui::Response {
    let accent = theme.accent();
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
    if selected {
        ui.painter()
            .circle_filled(rect.center(), 12.5, with_alpha(accent, 40));
        ui.painter()
            .circle_stroke(rect.center(), 11.5, Stroke::new(1.5, accent));
    }
    ui.painter().circle_filled(rect.center(), 7.5, accent);
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.on_hover_text(theme.label())
}

/// Desktop labels omit the extension, like Explorer does with "hide known file
/// extensions". Folders (including names such as `GodMode.{...}`) stay intact.
fn label_name(item: &FenceItem) -> String {
    if item.is_dir {
        return item.name.clone();
    }
    let path = Path::new(&item.name);
    let numeric_suffix = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.chars().all(|ch| ch.is_ascii_digit()))
        .unwrap_or(false);
    if numeric_suffix {
        // `v1.2` is usually part of the name, not an extension.
        return item.name.clone();
    }
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| item.name.clone())
}

#[allow(dead_code)]
pub fn short_name(name: &str, max_chars: usize) -> String {
    let count = name.chars().count();
    if count <= max_chars {
        name.to_owned()
    } else {
        format!(
            "{}…",
            name.chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::DesktopOrigin;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let unique = format!(
            "deskorg-fence-test-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(unique);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn desktop_item(path: PathBuf, name: &str, category: Category) -> DesktopItem {
        DesktopItem {
            path,
            name: name.to_owned(),
            category,
            is_dir: false,
            size: 3,
            modified: None,
            origin: DesktopOrigin::User,
        }
    }

    #[test]
    fn new_fence_is_transparent_and_unique() {
        let fence = Fence::new(
            7,
            Category::Image,
            0,
            Vec2::new(1920.0, 1080.0),
            Vec2::ZERO,
        );
        assert!(fence.opacity <= 0.1);
        assert!(!fence.blur);
        assert_eq!(fence.category, Category::Image);
        assert!(fence.x >= 0.0 && fence.y >= 0.0);
        assert_eq!(fence.window_title(), "DeskOrgFence-7");
    }

    #[test]
    fn clamp_size_caps_opacity_at_slider_max() {
        let mut fence = Fence::new(
            8,
            Category::Image,
            0,
            Vec2::new(1920.0, 1080.0),
            Vec2::ZERO,
        );
        fence.opacity = 0.95;
        fence.clamp_size();
        assert!((fence.opacity - 0.8).abs() < 1e-6);
    }

    #[test]
    fn collect_merges_desktop_and_archive() {
        let root = temp_dir("collect");
        let target = root.join("DeskOrg");
        let archive = target.join(Category::Document.folder_name());
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::write(archive.join("old.pdf"), b"pdf").unwrap();
        std::fs::write(archive.join("notes.txt"), b"txt").unwrap();

        let desktop_path = root.join("new.docx");
        std::fs::write(&desktop_path, b"docx").unwrap();
        let items = vec![desktop_item(desktop_path, "new.docx", Category::Document)];

        let data = collect_data(&items, &target, Category::Document);
        assert_eq!(data.items.len(), 3);
        assert_eq!(data.archived, 2);
        assert!(data.items.iter().any(|item| item.name == "old.pdf"));
        assert!(data
            .items
            .iter()
            .any(|item| item.name == "new.docx" && !item.archived));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn labels_hide_file_extensions_but_not_folder_names() {
        let item = |name: &str, is_dir: bool| FenceItem {
            path: PathBuf::from("X").join(name),
            name: name.to_owned(),
            is_dir,
            size: 0,
            archived: false,
            category: Category::Shortcut,
        };
        let mut file = item("Marvis.lnk", false);
        assert_eq!(label_name(&file), "Marvis");
        file.name = "archive.tar.gz".to_owned();
        assert_eq!(label_name(&file), "archive.tar");
        file.name = "no_extension".to_owned();
        assert_eq!(label_name(&file), "no_extension");
        file.name = "v1.2".to_owned();
        assert_eq!(label_name(&file), "v1.2");
        let folder = item("GodMode.{ED7BA470}", true);
        assert_eq!(label_name(&folder), "GodMode.{ED7BA470}");
    }

    #[test]
    fn auto_layout_keeps_boxes_on_screen_width() {
        let (x0, y0) = auto_position(0, Vec2::new(1366.0, 768.0), Vec2::ZERO);
        let (x1, y1) = auto_position(4, Vec2::new(1366.0, 768.0), Vec2::ZERO);
        assert!(x0 >= MARGIN);
        assert!(x1 >= 0.0);
        assert!(y1 > y0);
    }

    #[test]
    fn auto_position_offsets_by_virtual_desktop_origin() {
        let (x, y) = auto_position(0, Vec2::new(1920.0, 1080.0), Vec2::new(-1920.0, 0.0));
        assert_eq!(x, -1920.0 + MARGIN);
        assert_eq!(y, MARGIN);
    }

    #[test]
    fn frost_uv_uses_virtual_desktop_origin() {
        let mut fence = Fence::new(
            9,
            Category::Image,
            0,
            Vec2::new(3840.0, 1080.0),
            Vec2::new(-1920.0, 0.0),
        );
        fence.x = -1920.0;
        fence.y = 0.0;
        fence.w = 960.0;
        fence.h = 540.0;

        let desktop =
            Rect::from_min_size(Pos2::new(-1920.0, 0.0), Vec2::new(3840.0, 1080.0));
        let uv = frost_uv(&fence, desktop);

        assert!((uv.min.x - 0.0).abs() < 1e-6);
        assert!((uv.max.x - 0.25).abs() < 1e-6);
        assert!((uv.min.y - 0.0).abs() < 1e-6);
        assert!((uv.max.y - 0.5).abs() < 1e-6);
    }
}
