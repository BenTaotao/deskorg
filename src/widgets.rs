//! Hand-painted egui widgets that give DeskOrg its own visual language.

use eframe::egui::{
    self, Align, Align2, Color32, FontId, Layout, Response, RichText, Sense, Stroke, StrokeKind,
    Ui, Vec2,
};

use crate::theme::{blend, with_alpha, Palette};

pub fn draw_background(ui: &Ui, p: &Palette) {
    let rect = ui.max_rect();
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), p.bg_top);
    mesh.colored_vertex(rect.right_top(), blend(p.bg_top, p.bg_bottom, 0.35));
    mesh.colored_vertex(rect.right_bottom(), p.bg_bottom);
    mesh.colored_vertex(rect.left_bottom(), blend(p.bg_top, p.bg_bottom, 0.55));
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(egui::Shape::mesh(mesh));

    let glow = with_alpha(p.accent, if p.light { 18 } else { 12 });
    ui.painter()
        .circle_filled(rect.right_top() + Vec2::new(-96.0, 120.0), 310.0, glow);
    ui.painter().circle_filled(
        rect.left_bottom() + Vec2::new(220.0, -140.0),
        260.0,
        with_alpha(p.accent, if p.light { 8 } else { 7 }),
    );
}

pub fn nav_item(ui: &mut Ui, p: &Palette, glyph: &str, label: &str, selected: bool) -> Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 44.0), Sense::click());

    let fill = if selected {
        blend(p.accent, p.sidebar, 0.84)
    } else if response.hovered() {
        with_alpha(p.text, if p.light { 9 } else { 12 })
    } else {
        Color32::TRANSPARENT
    };
    if fill.a() > 0 {
        ui.painter().rect_filled(rect, 12, fill);
    }
    if selected {
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 1.0, rect.center().y - 9.0),
            Vec2::new(3.0, 18.0),
        );
        ui.painter().rect_filled(bar, 2, p.accent);
    }

    let icon_color = if selected { p.accent_ink } else { p.muted };
    ui.painter().text(
        rect.left_center() + Vec2::new(20.0, 0.0),
        Align2::LEFT_CENTER,
        glyph,
        FontId::proportional(13.0),
        icon_color,
    );
    ui.painter().text(
        rect.left_center() + Vec2::new(48.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.0),
        if selected { p.text } else { p.muted },
    );

    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

pub fn primary_button(ui: &mut Ui, p: &Palette, label: &str, width: f32) -> Response {
    let width = if width <= 0.0 {
        ui.available_width()
    } else {
        width
    };
    let button = egui::Button::new(
        RichText::new(label)
            .size(14.0)
            .strong()
            .color(p.accent_text),
    )
    .fill(p.accent)
    .stroke(Stroke::NONE)
    .corner_radius(12)
    .min_size(Vec2::new(width, 42.0));
    ui.add(button)
}

pub fn secondary_button(ui: &mut Ui, p: &Palette, label: &str, width: f32) -> Response {
    let width = if width <= 0.0 {
        ui.available_width()
    } else {
        width
    };
    let button = egui::Button::new(RichText::new(label).size(13.5).color(p.text))
        .fill(p.panel_soft)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(11)
        .min_size(Vec2::new(width, 38.0));
    ui.add(button)
}

pub fn ghost_button(ui: &mut Ui, p: &Palette, label: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(13.0).color(p.muted))
            .frame(false)
            .min_size(Vec2::new(0.0, 30.0)),
    )
}

pub fn pill(ui: &mut Ui, p: &Palette, text: &str, color: Color32) -> Response {
    let font = FontId::proportional(11.5);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let desired = Vec2::new(galley.size().x + 18.0, 24.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::hover());
    ui.painter()
        .rect_filled(rect, 12, with_alpha(color, if p.light { 22 } else { 26 }));
    ui.painter().galley(
        egui::pos2(rect.left() + 9.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    let _ = p;
    response
}

pub fn switch(ui: &mut Ui, p: &Palette, on: &mut bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(46.0, 26.0), Sense::click());
    if response.clicked() {
        *on = !*on;
    }
    let t = ui
        .ctx()
        .animate_bool_with_time(response.id.with("switch"), *on, 0.13);
    let track = if *on { p.accent } else { p.border_strong };
    ui.painter().rect_filled(rect, 13, track);
    let radius = 9.5;
    let x = egui::lerp((rect.left() + 13.0)..=(rect.right() - 13.0), t);
    ui.painter().circle_filled(
        egui::pos2(x, rect.center().y),
        radius,
        if *on { p.accent_text } else { p.panel },
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

pub fn toggle_row(ui: &mut Ui, p: &Palette, title: &str, description: &str, on: &mut bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(title).size(14.0).color(p.text));
            ui.label(RichText::new(description).size(11.5).color(p.faint));
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            changed = switch(ui, p, on);
        });
    });
    changed
}

pub fn hero_toggle(ui: &mut Ui, p: &Palette, visible: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(236.0, 236.0), Sense::click());
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id.with("hero-hover"), response.hovered(), 0.16);
    let center = rect.center();
    let accent = p.accent;

    for i in 0..10 {
        let radius = 76.0 + i as f32 * 7.0 + hover * 4.0;
        let alpha = if p.light { 5 } else { 4 };
        ui.painter()
            .circle_filled(center, radius, with_alpha(accent, alpha));
    }
    ui.painter()
        .circle_filled(center, 80.0 + hover * 3.0, with_alpha(accent, 18));
    ui.painter().circle_stroke(
        center,
        80.0 + hover * 3.0,
        Stroke::new(1.0, with_alpha(accent, 78)),
    );
    ui.painter()
        .circle_filled(center, 64.0, blend(p.bg_bottom, p.sidebar, 0.55));
    ui.painter()
        .circle_stroke(center, 64.0, Stroke::new(1.0, with_alpha(accent, 60)));

    if visible {
        // A warm sun.
        for i in 0..8 {
            let angle = i as f32 * std::f32::consts::TAU / 8.0;
            let inner = 39.0;
            let outer = 48.0;
            ui.painter().line_segment(
                [
                    center + Vec2::new(angle.cos(), angle.sin()) * inner,
                    center + Vec2::new(angle.cos(), angle.sin()) * outer,
                ],
                Stroke::new(2.0, with_alpha(accent, 190)),
            );
        }
        ui.painter().circle_filled(center, 31.0, accent);
        ui.painter().circle_filled(
            center + Vec2::new(-9.0, -9.0),
            10.0,
            with_alpha(Color32::WHITE, 48),
        );
    } else {
        // A crescent moon made from two circles.
        ui.painter()
            .circle_filled(center + Vec2::new(-4.0, 0.0), 31.0, accent);
        ui.painter().circle_filled(
            center + Vec2::new(15.0, -13.0),
            30.0,
            blend(p.bg_bottom, p.sidebar, 0.55),
        );
        for (offset, radius, alpha) in [
            (Vec2::new(38.0, -28.0), 2.4, 210),
            (Vec2::new(46.0, 10.0), 1.5, 140),
            (Vec2::new(24.0, 35.0), 1.2, 100),
        ] {
            ui.painter()
                .circle_filled(center + offset, radius, with_alpha(accent, alpha));
        }
    }

    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.on_hover_text("双击切换桌面图标")
}

pub fn category_badge(ui: &mut Ui, p: &Palette, category: crate::desktop::Category) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(30.0, 30.0), Sense::hover());
    let color = crate::theme::category_color(category);
    let ink = crate::theme::category_ink(category, p.light);
    ui.painter()
        .rect_filled(rect, 9, with_alpha(color, if p.light { 26 } else { 30 }));
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        category.glyph(),
        FontId::proportional(13.0),
        ink,
    );
}

pub fn section_header(ui: &mut Ui, p: &Palette, title: &str, hint: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(16.0).strong().color(p.text));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(hint).size(11.5).color(p.faint));
        });
    });
}

pub fn key_cap(ui: &mut Ui, p: &Palette, text: &str) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(11.0), p.muted);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(galley.size().x + 14.0, 22.0), Sense::hover());
    ui.painter().rect_filled(rect, 6, p.panel_soft);
    ui.painter()
        .rect_stroke(rect, 6, Stroke::new(1.0, p.border), StrokeKind::Inside);
    ui.painter().galley(
        egui::pos2(rect.left() + 7.0, rect.center().y - galley.size().y / 2.0),
        galley,
        p.muted,
    );
}

pub fn toast_frame(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(if p.light {
            Color32::from_rgba_unmultiplied(255, 255, 255, 246)
        } else {
            Color32::from_rgba_unmultiplied(26, 34, 53, 248)
        })
        .stroke(Stroke::new(1.0, p.border_strong))
        .corner_radius(14)
        .shadow(egui::epaint::Shadow {
            offset: [0, 6],
            blur: 22,
            spread: 0,
            color: Color32::from_black_alpha(if p.light { 35 } else { 80 }),
        })
        .inner_margin(egui::Margin::symmetric(18, 12))
}

pub fn empty_state(ui: &mut Ui, p: &Palette, glyph: &str, title: &str, subtitle: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(18.0);
        ui.label(RichText::new(glyph).size(30.0).color(p.accent_ink));
        ui.add_space(4.0);
        ui.label(RichText::new(title).size(15.0).color(p.text));
        ui.label(RichText::new(subtitle).size(12.0).color(p.faint));
        ui.add_space(14.0);
    });
}

pub fn accent_chip(ui: &mut Ui, p: &Palette, index: usize, selected: bool) -> Response {
    let accent = crate::theme::accent(index);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(38.0, 38.0), Sense::click());
    if selected {
        ui.painter()
            .circle_filled(rect.center(), 18.0, with_alpha(accent.color, 28));
        ui.painter()
            .circle_stroke(rect.center(), 17.0, Stroke::new(1.5, accent.color));
    }
    ui.painter()
        .circle_filled(rect.center(), 11.5, accent.color);
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let _ = p;
    response.on_hover_text(accent.name)
}

pub fn app_logo(ui: &mut Ui, p: &Palette, texture: Option<&egui::TextureHandle>) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(40.0, 40.0), Sense::hover());
    ui.painter().rect_filled(
        rect,
        12,
        with_alpha(p.accent, if p.light { 30 } else { 34 }),
    );
    if let Some(texture) = texture {
        ui.painter().image(
            texture.id(),
            rect.shrink(2.0),
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        return;
    }
    ui.painter().rect_stroke(
        rect,
        12,
        Stroke::new(1.0, with_alpha(p.accent, 95)),
        StrokeKind::Inside,
    );
    let center = rect.center();
    ui.painter()
        .circle_filled(center + Vec2::new(-2.0, 0.0), 9.0, p.accent);
    ui.painter().circle_filled(
        center + Vec2::new(4.0, -4.0),
        8.0,
        with_alpha(p.accent, 255),
    );
    ui.painter().circle_filled(
        center + Vec2::new(2.0, -2.5),
        6.0,
        with_alpha(p.bg_bottom, 255),
    );
}

pub fn footer_hint(ui: &mut Ui, p: &Palette, text: &str) {
    ui.label(RichText::new(text).size(10.5).color(p.faint));
}
