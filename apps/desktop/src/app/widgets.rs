//! Custom-painted widgets and vector icons.

use egui::epaint::PathShape;
use egui::{
    Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, vec2,
};

use super::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    ChevronLeft,
    ChevronRight,
    Target,
    Live,
    Grid,
    Peak,
    Histogram,
    Folder,
    Play,
    Stop,
    Refresh,
    Plug,
    Layers,
    Image,
    Record,
    Fullscreen,
    FullscreenExit,
    Info,
    Audio,
}

pub fn paint_icon(p: &Painter, icon: Icon, c: Pos2, size: f32, color: Color32) {
    let s = size;
    let stroke = Stroke::new(1.7, color);
    let pt = |x: f32, y: f32| Pos2::new(c.x + x * s, c.y + y * s);
    let line = |pts: &[Pos2]| {
        p.add(Shape::Path(PathShape::line(pts.to_vec(), stroke)));
    };
    match icon {
        Icon::ChevronLeft => line(&[pt(0.12, -0.34), pt(-0.18, 0.0), pt(0.12, 0.34)]),
        Icon::ChevronRight => line(&[pt(-0.12, -0.34), pt(0.18, 0.0), pt(-0.12, 0.34)]),
        Icon::Target => {
            p.circle_stroke(c, s * 0.28, stroke);
            p.circle_filled(c, s * 0.06, color);
            for (a, b) in [((0.0, -0.5), (0.0, -0.36)), ((0.0, 0.5), (0.0, 0.36)), ((-0.5, 0.0), (-0.36, 0.0)), ((0.5, 0.0), (0.36, 0.0))] {
                line(&[pt(a.0, a.1), pt(b.0, b.1)]);
            }
        }
        Icon::Live => {
            let r = Rect::from_center_size(c, vec2(s * 0.9, s * 0.64));
            p.rect_stroke(r, CornerRadius::same(3), stroke, StrokeKind::Inside);
            p.add(Shape::convex_polygon(vec![pt(-0.08, -0.14), pt(-0.08, 0.14), pt(0.15, 0.0)], color, Stroke::NONE));
        }
        Icon::Grid => {
            let r = Rect::from_center_size(c, vec2(s * 0.8, s * 0.8));
            p.rect_stroke(r, CornerRadius::same(2), stroke, StrokeKind::Inside);
            for f in [-0.133, 0.133] {
                line(&[pt(f, -0.4), pt(f, 0.4)]);
                line(&[pt(-0.4, f), pt(0.4, f)]);
            }
        }
        Icon::Peak => line(&[pt(-0.45, 0.3), pt(-0.15, -0.22), pt(0.05, 0.08), pt(0.2, -0.1), pt(0.45, 0.3)]),
        Icon::Histogram => {
            for (i, h) in [0.25, 0.55, 0.8, 0.4, 0.15].into_iter().enumerate() {
                let x = -0.36 + i as f32 * 0.18;
                line(&[pt(x, 0.4), pt(x, 0.4 - h * 0.8)]);
            }
        }
        Icon::Folder => {
            line(&[pt(-0.42, -0.28), pt(-0.12, -0.28), pt(0.0, -0.16), pt(0.42, -0.16), pt(0.42, 0.32), pt(-0.42, 0.32), pt(-0.42, -0.28)]);
        }
        Icon::Play => {
            p.add(Shape::convex_polygon(vec![pt(-0.22, -0.34), pt(-0.22, 0.34), pt(0.36, 0.0)], color, Stroke::NONE));
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_center_size(c, vec2(s * 0.56, s * 0.56)), CornerRadius::same(3), color);
        }
        Icon::Refresh => {
            let pts: Vec<Pos2> = (0..=22)
                .map(|i| {
                    let a = (i as f32 / 22.0) * 4.9 + 0.5;
                    Pos2::new(c.x + a.cos() * s * 0.32, c.y + a.sin() * s * 0.32)
                })
                .collect();
            let end = *pts.last().unwrap();
            p.add(Shape::Path(PathShape::line(pts, stroke)));
            line(&[Pos2::new(end.x + s * 0.02, end.y - s * 0.2), end, Pos2::new(end.x + s * 0.2, end.y - s * 0.02)]);
        }
        Icon::Plug => {
            line(&[pt(-0.14, -0.42), pt(-0.14, -0.14)]);
            line(&[pt(0.14, -0.42), pt(0.14, -0.14)]);
            p.rect_stroke(Rect::from_center_size(pt(0.0, 0.02), vec2(s * 0.6, s * 0.34)), CornerRadius::same(3), stroke, StrokeKind::Inside);
            line(&[pt(0.0, 0.2), pt(0.0, 0.44)]);
        }
        Icon::Layers => {
            for dy in [-0.2, 0.0, 0.2] {
                line(&[pt(-0.4, dy), pt(0.0, dy - 0.18), pt(0.4, dy), pt(0.0, dy + 0.18), pt(-0.4, dy)]);
            }
        }
        Icon::Record => {
            p.circle_filled(c, s * 0.3, color);
        }
        Icon::Fullscreen => {
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                line(&[pt(0.12 * sx, 0.4 * sy), pt(0.4 * sx, 0.4 * sy), pt(0.4 * sx, 0.12 * sy)]);
            }
        }
        Icon::FullscreenExit => {
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                line(&[pt(0.4 * sx, 0.12 * sy), pt(0.12 * sx, 0.12 * sy), pt(0.12 * sx, 0.4 * sy)]);
            }
        }
        Icon::Audio => {
            line(&[pt(-0.38, -0.12), pt(-0.2, -0.12), pt(0.02, -0.34), pt(0.02, 0.34), pt(-0.2, 0.12), pt(-0.38, 0.12), pt(-0.38, -0.12)]);
            for k in 1..=2 {
                let r = 0.14 * k as f32;
                let arc: Vec<Pos2> = (-4..=4).map(|i| {
                    let a = i as f32 * 0.17;
                    pt(0.12 + r * a.cos(), r * 1.4 * a.sin())
                }).collect();
                line(&arc);
            }
        }
        Icon::Info => {
            p.circle_stroke(c, s * 0.38, stroke);
            line(&[pt(0.0, -0.02), pt(0.0, 0.2)]);
            p.circle_filled(pt(0.0, -0.16), s * 0.04, color);
        }
        Icon::Image => {
            p.rect_stroke(Rect::from_center_size(c, vec2(s * 0.86, s * 0.68)), CornerRadius::same(3), stroke, StrokeKind::Inside);
            line(&[pt(-0.38, 0.22), pt(-0.1, -0.06), pt(0.08, 0.12), pt(0.2, 0.0), pt(0.38, 0.22)]);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Primary,
    Secondary,
    Ghost,
    Danger,
    /// Solid red, used while a recording is running.
    Recording,
}

/// A fully custom button with optional icon.
pub fn button(ui: &mut Ui, label: &str, icon: Option<Icon>, kind: Kind) -> Response {
    let enabled = ui.is_enabled();
    let font = FontId::proportional(13.5);
    let (fg_idle, bg_idle, bg_hover, border) = match kind {
        Kind::Primary => (Color32::from_rgb(24, 17, 4), theme::ACCENT, Color32::from_rgb(255, 196, 82), theme::ACCENT),
        Kind::Secondary => (theme::TEXT, theme::CARD, theme::CARD_HOVER, theme::STROKE),
        Kind::Ghost => (theme::MUTED, Color32::TRANSPARENT, theme::CARD, Color32::TRANSPARENT),
        Kind::Danger => (theme::DANGER, Color32::from_rgb(44, 22, 26), Color32::from_rgb(64, 28, 34), Color32::from_rgb(94, 40, 48)),
        Kind::Recording => (Color32::WHITE, theme::DANGER, Color32::from_rgb(255, 128, 128), theme::DANGER),
    };
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, fg_idle);
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let gap = if icon.is_some() && !label.is_empty() { 6.0 } else { 0.0 };
    let size = if label.is_empty() { Vec2::splat(34.0) } else { vec2(galley.size().x + icon_w + gap + 26.0, 34.0) };
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() && enabled, 0.12);
    let pressed = resp.is_pointer_button_down_on() && enabled;
    let mix = |a: Color32, b: Color32, t: f32| Color32::from_rgba_premultiplied(
        (a.r() as f32 + (b.r() as f32 - a.r() as f32) * t) as u8,
        (a.g() as f32 + (b.g() as f32 - a.g() as f32) * t) as u8,
        (a.b() as f32 + (b.b() as f32 - a.b() as f32) * t) as u8,
        (a.a() as f32 + (b.a() as f32 - a.a() as f32) * t) as u8,
    );
    let mut bg = mix(bg_idle, bg_hover, hover);
    let mut fg = fg_idle;
    if pressed {
        bg = mix(bg, Color32::BLACK, 0.18);
    }
    if !enabled {
        bg = mix(bg, theme::PANEL, 0.6);
        fg = theme::FAINT;
    }
    ui.painter().rect_filled(rect, CornerRadius::same(9), bg);
    if border != Color32::TRANSPARENT {
        ui.painter().rect_stroke(rect, CornerRadius::same(9), Stroke::new(1.0, if enabled { border } else { theme::STROKE }), StrokeKind::Inside);
    }
    let total = galley.size().x + icon_w + gap;
    let mut x = rect.center().x - total / 2.0;
    if let Some(icon) = icon {
        paint_icon(ui.painter(), icon, Pos2::new(x + 10.0, rect.center().y), 18.0, fg);
        x += icon_w + gap;
    }
    if !label.is_empty() {
        ui.painter().galley(Pos2::new(x, rect.center().y - galley.size().y / 2.0), galley, fg);
    }
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// Square icon-only button.
pub fn icon_button(ui: &mut Ui, icon: Icon, kind: Kind) -> Response {
    button(ui, "", Some(icon), kind)
}

/// Toggleable chip used for overlays.
pub fn chip(ui: &mut Ui, label: &str, icon: Icon, on: bool) -> Response {
    let font = FontId::proportional(12.5);
    let color = if on { theme::ACCENT } else { theme::MUTED };
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, color);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 44.0, 30.0), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id.with("on"), on, 0.15);
    let h = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered(), 0.12);
    let bg = if on { theme::ACCENT_DIM.gamma_multiply(0.55 + 0.0 * t) } else { theme::CARD.gamma_multiply(0.7 + 0.3 * h) };
    ui.painter().rect_filled(rect, CornerRadius::same(15), bg);
    ui.painter().rect_stroke(rect, CornerRadius::same(15), Stroke::new(1.0, if on { theme::ACCENT.gamma_multiply(0.55) } else { theme::STROKE }), StrokeKind::Inside);
    paint_icon(ui.painter(), icon, Pos2::new(rect.left() + 18.0, rect.center().y), 15.0, color);
    ui.painter().galley(Pos2::new(rect.left() + 31.0, rect.center().y - galley.size().y / 2.0), galley, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Segmented selector; returns true when the selection changed.
pub fn segmented(ui: &mut Ui, options: &[&str], selected: &mut usize) -> bool {
    let mut changed = false;
    let width = ui.available_width();
    let seg_w = (width - 6.0) / options.len() as f32;
    let (rect, _) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(9), theme::BG);
    ui.painter().rect_stroke(rect, CornerRadius::same(9), Stroke::new(1.0, theme::STROKE), StrokeKind::Inside);
    for (i, label) in options.iter().enumerate() {
        let r = Rect::from_min_size(rect.min + vec2(3.0 + seg_w * i as f32, 3.0), vec2(seg_w, 28.0));
        let resp = ui.interact(r, ui.make_persistent_id(("seg", i, label)), Sense::click());
        let active = *selected == i;
        let t = ui.ctx().animate_bool_with_time(resp.id, active, 0.15);
        if active {
            ui.painter().rect_filled(r, CornerRadius::same(7), theme::CARD_HOVER.gamma_multiply(0.6 + 0.4 * t));
            ui.painter().rect_stroke(r, CornerRadius::same(7), Stroke::new(1.0, theme::FAINT), StrokeKind::Inside);
        }
        let color = if active { theme::TEXT } else if resp.hovered() { theme::TEXT.gamma_multiply(0.8) } else { theme::MUTED };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(12.5), color);
        if resp.clicked() && !active {
            *selected = i;
            changed = true;
        }
    }
    changed
}

pub fn card<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0, theme::STROKE))
        .corner_radius(theme::radius())
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !title.is_empty() {
                ui.label(RichText::new(title.to_uppercase()).size(11.0).color(theme::MUTED).strong());
                ui.add_space(2.0);
            }
            add(ui)
        })
        .inner
}

pub fn pill(ui: &mut Ui, text: &str, color: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(12.0), color);
    let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x + 28.0, 24.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(12), color.gamma_multiply(0.14));
    ui.painter().circle_filled(Pos2::new(rect.left() + 12.0, rect.center().y), 3.2, color);
    ui.painter().galley(Pos2::new(rect.left() + 21.0, rect.center().y - galley.size().y / 2.0), galley, color);
}

pub fn tag(ui: &mut Ui, text: &str, color: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(11.0), color);
    let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x + 14.0, 20.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(6), color.gamma_multiply(0.13));
    ui.painter().galley(Pos2::new(rect.left() + 7.0, rect.center().y - galley.size().y / 2.0), galley, color);
}

/// Large circular shutter release.
pub fn shutter_button(ui: &mut Ui, busy: bool) -> Response {
    let enabled = ui.is_enabled();
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(64.0), if enabled { Sense::click() } else { Sense::hover() });
    let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() && enabled, 0.12);
    let pressed = resp.is_pointer_button_down_on() && enabled;
    let c = rect.center();
    let accent = if enabled { theme::ACCENT } else { theme::FAINT };
    ui.painter().circle_stroke(c, 30.0 + h * 1.5, Stroke::new(2.5, accent));
    let inner = if pressed { 21.0 } else { 23.5 + h };
    ui.painter().circle_filled(c, inner, if busy { theme::ACCENT_DIM } else { accent });
    if busy {
        let t = ui.input(|i| i.time) as f32;
        for k in 0..3 {
            let a = t * 5.0 + k as f32 * 2.09;
            ui.painter().circle_filled(c + vec2(a.cos(), a.sin()) * 11.0, 2.6, theme::ACCENT);
        }
        ui.ctx().request_repaint();
    }
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// Thin progress bar with rounded ends.
pub fn progress_bar(ui: &mut Ui, fraction: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(3), theme::BG);
    let mut fill = rect;
    fill.set_width((rect.width() * fraction.clamp(0.0, 1.0)).max(if fraction > 0.0 { 6.0 } else { 0.0 }));
    ui.painter().rect_filled(fill, CornerRadius::same(3), color);
}

/// Battery glyph with a level fill, e.g. in the header and the camera OSD.
pub fn battery_glyph(p: &Painter, rect: Rect, percent: u8) {
    let color = if percent > 30 { theme::OK } else if percent > 12 { theme::WARN } else { theme::DANGER };
    let body = Rect::from_min_max(rect.min, Pos2::new(rect.max.x - 3.0, rect.max.y));
    p.rect_stroke(body, CornerRadius::same(3), Stroke::new(1.4, theme::MUTED), StrokeKind::Inside);
    p.rect_filled(Rect::from_center_size(Pos2::new(rect.max.x - 1.5, rect.center().y), vec2(3.0, rect.height() * 0.4)), CornerRadius::same(1), theme::MUTED);
    let inner = body.shrink(2.5);
    let fill = Rect::from_min_size(inner.min, vec2(inner.width() * percent.min(100) as f32 / 100.0, inner.height()));
    p.rect_filled(fill, CornerRadius::same(1), color);
}

pub fn field_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(theme::MUTED));
}
