//! The floating desktop clock: a borderless, transparent viewport that shows
//! the local date and time. It never touches files or the desktop icons.

use std::time::Duration;

#[cfg(not(windows))]
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{
    self, Color32, FontFamily, FontId, Pos2, Stroke, StrokeKind, Ui, Vec2,
};
use serde::{Deserialize, Serialize};

use crate::shell;
use crate::theme::with_alpha;

pub const WINDOW_TITLE: &str = "DeskOrgClock";
pub const MIN_SCALE: f32 = 0.6;
pub const MAX_SCALE: f32 = 3.0;
pub const BASE_WIDTH: f32 = 340.0;
pub const BASE_HEIGHT: f32 = 136.0;
const BASE_TIME_FONT: f32 = 56.0;
const BASE_DATE_FONT: f32 = 15.0;

const TEXT: Color32 = Color32::from_rgb(246, 249, 255);
const BG: Color32 = Color32::from_rgb(10, 14, 23);
const SHADOW: Color32 = Color32::from_black_alpha(130);

pub fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("deskorg-clock")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Clock {
    pub enabled: bool,
    /// true = 最顶层（始终可见），false = 桌面层（普通窗口在前台时被遮住）。
    pub topmost: bool,
    pub show_weekday: bool,
    pub scale: f32,
    /// Background only; the digits stay opaque. 0 = fully transparent.
    pub opacity: f32,
    /// Outer position in egui points; `None` until the clock is first shown.
    pub pos: Option<[f32; 2]>,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            enabled: false,
            topmost: false,
            show_weekday: true,
            scale: 1.0,
            opacity: 0.35,
            pos: None,
        }
    }
}

impl Clock {
    pub fn clamp(&mut self) {
        self.scale = if self.scale.is_finite() {
            self.scale.clamp(MIN_SCALE, MAX_SCALE)
        } else {
            1.0
        };
        self.opacity = if self.opacity.is_finite() {
            self.opacity.clamp(0.0, 1.0)
        } else {
            0.35
        };
    }

    pub fn size(&self) -> Vec2 {
        Vec2::new(BASE_WIDTH, BASE_HEIGHT) * self.scale.clamp(MIN_SCALE, MAX_SCALE)
    }
}

/// Default spot: the top-right corner of the virtual desktop.
pub fn default_position(origin: Vec2, desktop: Vec2, size: Vec2) -> [f32; 2] {
    let width = if desktop.x > 0.0 { desktop.x } else { 1920.0 };
    [origin.x + width - size.x - 40.0, origin.y + 40.0]
}

/// Paint the clock face and start the native drag on mouse-down.
pub fn draw(ui: &mut Ui, clock: &Clock) {
    let rect = ui.max_rect();
    if rect.width() < 40.0 || rect.height() < 24.0 {
        return;
    }
    let now = LocalTime::now();
    // 每秒重绘一次，让秒数准时跳动。
    ui.ctx()
        .request_repaint_after(Duration::from_millis(now.millis_to_next_second().max(50)));

    let scale = clock.scale.clamp(MIN_SCALE, MAX_SCALE);

    if clock.opacity > 0.002 {
        let alpha = (clock.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        let corner = egui::CornerRadius::same((14.0 * scale).clamp(4.0, 30.0) as u8);
        ui.painter().rect_filled(rect, corner, with_alpha(BG, alpha));
        if clock.opacity > 0.05 {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                corner,
                Stroke::new(
                    1.0,
                    Color32::from_rgba_unmultiplied(255, 255, 255, 22),
                ),
                StrokeKind::Inside,
            );
        }
    }

    let time_text = now.time_text();
    let date_text = now.date_text(clock.show_weekday);
    let time_font = FontId::new(BASE_TIME_FONT * scale, FontFamily::Monospace);
    let date_font = FontId::new(BASE_DATE_FONT * scale, FontFamily::Proportional);

    let painter = ui.painter();
    let time = painter.layout_no_wrap(time_text.clone(), time_font.clone(), TEXT);
    let date = painter.layout_no_wrap(date_text.clone(), date_font.clone(), TEXT);
    let time_shadow = painter.layout_no_wrap(time_text, time_font, SHADOW);
    let date_shadow = painter.layout_no_wrap(date_text, date_font, SHADOW);

    let gap = 6.0 * scale;
    let block_h = date.size().y + gap + time.size().y;
    let top = rect.center().y - block_h * 0.5;
    let date_pos = Pos2::new(rect.center().x - date.size().x * 0.5, top);
    let time_pos = Pos2::new(
        rect.center().x - time.size().x * 0.5,
        top + date.size().y + gap,
    );
    let offset = Vec2::new(0.0, (1.5 * scale).max(1.0));

    painter.galley(date_pos + offset, date_shadow, SHADOW);
    painter.galley(time_pos + offset, time_shadow, SHADOW);
    painter.galley(date_pos, date, TEXT);
    painter.galley(time_pos, time, TEXT);

    let response = ui.interact(
        rect,
        ui.scope_id().with("deskorg-clock-drag"),
        egui::Sense::click_and_drag(),
    );
    if response.drag_started() {
        // 交给 Windows 原生移动循环，避免逐帧设置窗口位置造成抖动。
        shell::start_window_drag(WINDOW_TITLE);
    }
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
}

struct LocalTime {
    year: u16,
    month: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    millis: u16,
    /// 0 = Sunday.
    weekday: u16,
}

impl LocalTime {
    fn time_text(&self) -> String {
        format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
    }

    fn date_text(&self, show_weekday: bool) -> String {
        if show_weekday {
            format!(
                "{:04}-{:02}-{:02} 周{}",
                self.year,
                self.month,
                self.day,
                weekday_label(self.weekday)
            )
        } else {
            format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
        }
    }

    fn millis_to_next_second(&self) -> u64 {
        1000u64.saturating_sub(self.millis as u64)
    }

    #[cfg(windows)]
    fn now() -> Self {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;

        let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut st) };
        Self {
            year: st.wYear,
            month: st.wMonth,
            day: st.wDay,
            hour: st.wHour,
            minute: st.wMinute,
            second: st.wSecond,
            millis: st.wMilliseconds,
            weekday: st.wDayOfWeek,
        }
    }

    #[cfg(not(windows))]
    fn now() -> Self {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let secs = duration.as_secs();
        let days = (secs / 86_400) as i64;
        let rest = secs % 86_400;
        let (year, month, day) = civil_from_days(days);
        Self {
            year: year.clamp(0, u16::MAX as i64) as u16,
            month: month as u16,
            day: day as u16,
            hour: (rest / 3_600) as u16,
            minute: ((rest % 3_600) / 60) as u16,
            second: (rest % 60) as u16,
            millis: duration.subsec_millis() as u16,
            weekday: ((days + 4).rem_euclid(7)) as u16,
        }
    }
}

fn weekday_label(index: u16) -> &'static str {
    const LABELS: [&str; 7] = ["日", "一", "二", "三", "四", "五", "六"];
    LABELS[(index as usize) % 7]
}

/// Howard Hinnant's civil-from-days; only needed where Win32 is unavailable.
#[cfg(not(windows))]
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LocalTime {
        LocalTime {
            year: 2026,
            month: 11,
            day: 20,
            hour: 9,
            minute: 5,
            second: 3,
            millis: 250,
            weekday: 5,
        }
    }

    #[test]
    fn clock_defaults_are_desktop_layer_and_translucent() {
        let clock = Clock::default();
        assert!(!clock.enabled);
        assert!(!clock.topmost);
        assert!(clock.show_weekday);
        assert!((clock.scale - 1.0).abs() < 1e-6);
        assert!((clock.opacity - 0.35).abs() < 1e-6);
        assert!(clock.pos.is_none());
    }

    #[test]
    fn clamp_holds_scale_and_opacity_in_range() {
        let mut clock = Clock {
            scale: 9.0,
            opacity: 2.0,
            ..Clock::default()
        };
        clock.clamp();
        assert!((clock.scale - MAX_SCALE).abs() < 1e-6);
        assert!((clock.opacity - 1.0).abs() < 1e-6);

        clock.scale = 0.1;
        clock.opacity = -1.0;
        clock.clamp();
        assert!((clock.scale - MIN_SCALE).abs() < 1e-6);
        assert!((clock.opacity - 0.0).abs() < 1e-6);
    }

    #[test]
    fn size_scales_with_the_slider() {
        let clock = Clock {
            scale: 2.0,
            ..Clock::default()
        };
        let size = clock.size();
        assert!((size.x - BASE_WIDTH * 2.0).abs() < 1e-6);
        assert!((size.y - BASE_HEIGHT * 2.0).abs() < 1e-6);
    }

    #[test]
    fn default_position_sits_at_the_top_right() {
        let pos = default_position(
            Vec2::new(-1920.0, 0.0),
            Vec2::new(3840.0, 1080.0),
            Vec2::new(340.0, 136.0),
        );
        assert!((pos[0] - (-1920.0 + 3840.0 - 340.0 - 40.0)).abs() < 1e-6);
        assert!((pos[1] - 40.0).abs() < 1e-6);
    }

    #[test]
    fn formats_time_date_and_weekday() {
        let now = sample();
        assert_eq!(now.time_text(), "09:05:03");
        assert_eq!(now.date_text(true), "2026-11-20 周五");
        assert_eq!(now.date_text(false), "2026-11-20");
        assert_eq!(now.millis_to_next_second(), 750);
    }

    #[test]
    fn weekday_labels_wrap_safely() {
        assert_eq!(weekday_label(0), "日");
        assert_eq!(weekday_label(5), "五");
        assert_eq!(weekday_label(9), "二");
    }
}
