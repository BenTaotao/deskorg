//! Visual language for DeskOrg: palette, fonts and egui style.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Visuals,
};

pub struct Accent {
    pub name: &'static str,
    pub color: Color32,
}

pub const ACCENTS: [Accent; 6] = [
    Accent {
        name: "薄荷",
        color: Color32::from_rgb(94, 234, 212),
    },
    Accent {
        name: "星蓝",
        color: Color32::from_rgb(96, 165, 250),
    },
    Accent {
        name: "薰衣草",
        color: Color32::from_rgb(167, 139, 250),
    },
    Accent {
        name: "蜜桃",
        color: Color32::from_rgb(251, 146, 60),
    },
    Accent {
        name: "樱花",
        color: Color32::from_rgb(244, 114, 182),
    },
    Accent {
        name: "青柠",
        color: Color32::from_rgb(74, 222, 128),
    },
];

pub fn accent(index: usize) -> &'static Accent {
    &ACCENTS[index.min(ACCENTS.len() - 1)]
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub light: bool,
    pub bg_top: Color32,
    pub bg_bottom: Color32,
    pub sidebar: Color32,
    pub panel: Color32,
    pub panel_soft: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    /// High-contrast variant of the accent, for text on panels.
    pub accent_ink: Color32,
    pub accent_soft: Color32,
    pub accent_text: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
}

pub fn palette(light: bool, accent_index: usize) -> Palette {
    let accent = accent(accent_index);
    if light {
        Palette {
            light,
            bg_top: Color32::from_rgb(246, 248, 253),
            bg_bottom: Color32::from_rgb(232, 238, 249),
            sidebar: Color32::from_rgba_unmultiplied(255, 255, 255, 225),
            panel: Color32::from_rgb(255, 255, 255),
            panel_soft: Color32::from_rgb(243, 246, 252),
            border: Color32::from_rgb(222, 229, 240),
            border_strong: Color32::from_rgb(202, 213, 230),
            text: Color32::from_rgb(24, 32, 51),
            muted: Color32::from_rgb(92, 106, 132),
            faint: Color32::from_rgb(142, 154, 176),
            accent: accent.color,
            accent_ink: blend(accent.color, Color32::from_rgb(10, 18, 22), 0.42),
            accent_soft: blend(accent.color, Color32::WHITE, 0.82),
            accent_text: Color32::from_rgb(8, 20, 24),
            success: Color32::from_rgb(10, 125, 90),
            warning: Color32::from_rgb(170, 96, 8),
            danger: Color32::from_rgb(196, 44, 55),
        }
    } else {
        Palette {
            light,
            bg_top: Color32::from_rgb(9, 12, 20),
            bg_bottom: Color32::from_rgb(16, 22, 37),
            sidebar: Color32::from_rgba_unmultiplied(15, 20, 32, 235),
            panel: Color32::from_rgb(20, 27, 43),
            panel_soft: Color32::from_rgb(25, 33, 52),
            border: Color32::from_rgb(39, 49, 72),
            border_strong: Color32::from_rgb(55, 68, 96),
            text: Color32::from_rgb(240, 245, 253),
            muted: Color32::from_rgb(148, 161, 184),
            faint: Color32::from_rgb(101, 114, 139),
            accent: accent.color,
            accent_ink: accent.color,
            accent_soft: blend(accent.color, Color32::from_rgb(12, 17, 28), 0.78),
            accent_text: Color32::from_rgb(7, 18, 22),
            success: Color32::from_rgb(52, 211, 153),
            warning: Color32::from_rgb(251, 191, 36),
            danger: Color32::from_rgb(248, 113, 113),
        }
    }
}

pub fn card_frame(palette: Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.panel)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(18)
        .inner_margin(egui::Margin::same(18))
}

pub fn soft_frame(palette: Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.panel_soft)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(14)
        .inner_margin(egui::Margin::same(12))
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let candidates = [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msyh.ttf",
        r"C:\Windows\Fonts\msyhbd.ttc",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
        r"C:\Windows\Fonts\Deng.ttf",
    ];

    let loaded = candidates
        .iter()
        .find_map(|path| std::fs::read(path).ok())
        .map(|bytes| FontData::from_blob(Arc::new(bytes), 0));

    if let Some(font) = loaded {
        let name = "deskorg-cjk".to_owned();
        fonts.font_data.insert(name.clone(), Arc::new(font));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            if let Some(list) = fonts.families.get_mut(&family) {
                if list.len() > 1 {
                    list.insert(1, name.clone());
                } else {
                    list.push(name.clone());
                }
            }
        }
    }

    ctx.set_fonts(fonts);
}

pub fn apply_style(ctx: &egui::Context, light: bool, accent_index: usize) {
    let p = palette(light, accent_index);
    let theme = if light {
        egui::Theme::Light
    } else {
        egui::Theme::Dark
    };
    ctx.set_theme(theme);

    let mut visuals = if light {
        Visuals::light()
    } else {
        Visuals::dark()
    };

    visuals.override_text_color = Some(p.text);
    visuals.panel_fill = p.panel;
    visuals.window_fill = p.panel;
    visuals.extreme_bg_color = p.panel_soft;
    visuals.faint_bg_color = p.panel_soft;
    visuals.window_stroke = Stroke::new(1.0, p.border);
    visuals.window_corner_radius = egui::CornerRadius::same(16);
    visuals.menu_corner_radius = egui::CornerRadius::same(12);
    visuals.selection.bg_fill = p.accent.gamma_multiply(0.30);
    visuals.selection.stroke = Stroke::new(1.0, p.accent);
    visuals.hyperlink_color = p.accent;
    visuals.warn_fg_color = p.warning;
    visuals.error_fg_color = p.danger;

    visuals.widgets.noninteractive.bg_fill = p.panel;
    visuals.widgets.noninteractive.weak_bg_fill = p.panel;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(12);

    visuals.widgets.inactive.bg_fill = p.panel_soft;
    visuals.widgets.inactive.weak_bg_fill = p.panel_soft;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(10);

    visuals.widgets.hovered.bg_fill = p.panel_soft;
    visuals.widgets.hovered.weak_bg_fill = blend(p.accent, p.panel, 0.86);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.7));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(10);

    visuals.widgets.active.bg_fill = p.accent_soft;
    visuals.widgets.active.weak_bg_fill = p.accent_soft;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, p.text);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(10);

    visuals.widgets.open.bg_fill = p.panel_soft;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, p.border_strong);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, p.text);
    visuals.widgets.open.corner_radius = egui::CornerRadius::same(10);

    let mut style = (*ctx.global_style()).clone();
    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(15.0, 8.0);
    style.spacing.interact_size.y = 36.0;
    style.spacing.scroll.bar_width = 6.0;
    style.animation_time = 0.16;
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(12.0, FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(14.5, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(14.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(27.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(13.0, FontFamily::Monospace),
    );

    ctx.set_style_of(theme, style);
}

pub fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
        mix(a.a(), b.a()),
    )
}

pub fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

pub fn category_color(category: crate::desktop::Category) -> Color32 {
    let [r, g, b] = category.rgb();
    Color32::from_rgb(r, g, b)
}

pub fn category_ink(category: crate::desktop::Category, light: bool) -> Color32 {
    let color = category_color(category);
    if light {
        blend(color, Color32::BLACK, 0.38)
    } else {
        color
    }
}
