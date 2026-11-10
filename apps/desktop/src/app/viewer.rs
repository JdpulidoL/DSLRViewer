//! Centre column: live view with overlays and the shooting controls.

use egui::epaint::PathShape;
use egui::{Align, Align2, Color32, CornerRadius, FontId, Layout, Mesh, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, vec2};

use super::meters;
use super::widgets::{self, Icon, Kind};
use super::{Connection, LumenApp, theme};
use crate::camera::eos;
use crate::engine::Command;
use crate::imaging::HIST_BINS;
use crate::ptp::codes::prop;

const CONTROLS_HEIGHT: f32 = 136.0;

pub fn show(app: &mut LumenApp, ui: &mut Ui) {
    let area_h = (ui.available_height() - CONTROLS_HEIGHT - ui.spacing().item_spacing.y).max(120.0);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), area_h), Sense::click_and_drag());
    stage(app, ui, rect, &response);
    ui.add_space(2.0);
    controls(app, ui);
}

/// Edge-to-edge live view with an auto-hiding toolbar.
pub fn show_fullscreen(app: &mut LumenApp, ui: &mut Ui) {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    stage(app, ui, rect, &response);
    let ctx = ui.ctx().clone();
    let active = app.last_pointer_move.elapsed() < std::time::Duration::from_millis(2600);
    let fade = ctx.animate_bool_with_time(egui::Id::NULL.with("fs-toolbar"), active, 0.25);
    if !active && fade < 0.01 {
        ctx.set_cursor_icon(egui::CursorIcon::None);
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(300));
    let bottom = if app.cfg.show_osd { 100.0 } else { 36.0 };
    egui::Area::new(egui::Id::NULL.with("fs-toolbar-area"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -bottom))
        .show(&ctx, |ui| {
            ui.set_opacity(fade);
            egui::Frame::new()
                .fill(Color32::from_black_alpha(205))
                .stroke(Stroke::new(1.0, theme::STROKE))
                .corner_radius(CornerRadius::same(16))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| action_buttons(app, ui));
                });
        });
}

fn stage(app: &mut LumenApp, ui: &mut Ui, rect: Rect, response: &egui::Response) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(14), Color32::from_rgb(6, 8, 11));

    let show_image = app.frame_tex.is_some() && app.is_connected();
    if show_image {
        let image_rect = image_rect(app, rect);
        interact_zoom(app, ui, response, image_rect);
        let tex = app.frame_tex.as_ref().expect("checked above");
        let z = app.zoom;
        let centre = Pos2::new((0.5 + app.pan.x).clamp(0.5 / z, 1.0 - 0.5 / z), (0.5 + app.pan.y).clamp(0.5 / z, 1.0 - 0.5 / z));
        let uv = Rect::from_center_size(centre, Vec2::splat(1.0 / z));
        painter.image(tex.id(), image_rect, uv, Color32::WHITE);
        let clipped = painter.with_clip_rect(image_rect);
        if app.cfg.show_grid {
            grid(&clipped, image_rect);
        }
        if app.live {
            measure_region(&clipped, image_rect);
        }
        hud_top(app, ui, &painter, rect);
        let inset = if app.cfg.show_osd { 84.0 } else { 16.0 };
        if app.cfg.show_histogram && !app.histogram.is_empty() {
            histogram(&painter, rect, &app.histogram, inset);
        }
        sharpness_meter(app, &painter, rect, inset);
        if app.cfg.show_osd {
            osd(app, &painter, rect);
        }
        if app.audio_on {
            audio_meters(app, &painter, rect, inset);
        }
        if app.recording {
            let pulse = 0.55 + 0.45 * ((ui.input(|i| i.time) * 3.0).sin() as f32 * 0.5 + 0.5);
            painter.rect_stroke(rect.shrink(1.5), CornerRadius::same(14), Stroke::new(3.0, theme::DANGER.gamma_multiply(pulse)), StrokeKind::Inside);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
        }
    } else {
        empty_state(app, ui, rect);
    }
    painter.rect_stroke(rect, CornerRadius::same(14), Stroke::new(1.0, theme::STROKE), StrokeKind::Inside);
    if let Some((video, since)) = app.mode_switch {
        mode_switch_overlay(ui, &painter, rect, video, since.elapsed().as_secs_f32());
    }
}

/// Loading screen shown while the camera re-initialises after the mode dial moved.
fn mode_switch_overlay(ui: &mut Ui, painter: &egui::Painter, rect: Rect, video: bool, secs: f32) {
    painter.rect_filled(rect, CornerRadius::same(14), Color32::from_black_alpha(215));
    let centre = rect.center();
    ui.put(Rect::from_center_size(centre - vec2(0.0, 26.0), Vec2::splat(44.0)), egui::Spinner::new().size(40.0).color(theme::ACCENT));
    let _ = video;
    let (title, sub) = ("Camera mode changed", "Reading the position of the mode dial");
    painter.text(centre + vec2(0.0, 24.0), Align2::CENTER_CENTER, title, egui::FontId::proportional(18.0), Color32::WHITE);
    let dots = ".".repeat(1 + (secs * 2.5) as usize % 3);
    painter.text(centre + vec2(0.0, 48.0), Align2::CENTER_CENTER, format!("{sub}{dots}"), egui::FontId::proportional(13.0), theme::MUTED);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
}

/// Largest rectangle with the frame's aspect ratio inside `rect`.
fn image_rect(app: &LumenApp, rect: Rect) -> Rect {
    let inner = rect.shrink(10.0);
    let (w, h) = (app.frame_size.0.max(1) as f32, app.frame_size.1.max(1) as f32);
    let scale = (inner.width() / w).min(inner.height() / h);
    Rect::from_center_size(inner.center(), vec2(w * scale, h * scale))
}

fn interact_zoom(app: &mut LumenApp, ui: &Ui, response: &egui::Response, image_rect: Rect) {
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            app.zoom = (app.zoom * (1.0 + scroll * 0.0016)).clamp(1.0, 8.0);
        }
    }
    if response.dragged() && app.zoom > 1.0 {
        let d = response.drag_delta();
        app.pan -= vec2(d.x / image_rect.width(), d.y / image_rect.height()) / app.zoom;
    }
    if response.double_clicked() {
        app.zoom = 1.0;
        app.pan = Vec2::ZERO;
    }
    if app.zoom <= 1.0 {
        app.pan = Vec2::ZERO;
    }
}

fn grid(p: &egui::Painter, r: Rect) {
    let s = Stroke::new(1.0, Color32::from_white_alpha(46));
    for f in [1.0 / 3.0, 2.0 / 3.0] {
        p.line_segment([Pos2::new(r.left() + r.width() * f, r.top()), Pos2::new(r.left() + r.width() * f, r.bottom())], s);
        p.line_segment([Pos2::new(r.left(), r.top() + r.height() * f), Pos2::new(r.right(), r.top() + r.height() * f)], s);
    }
}

/// Corner brackets marking the area the sharpness meter measures.
fn measure_region(p: &egui::Painter, r: Rect) {
    let m = Rect::from_min_max(r.min + r.size() * 0.25, r.min + r.size() * 0.75);
    let s = Stroke::new(1.5, theme::LIVE.gamma_multiply(0.75));
    let t = 16.0;
    for (corner, dx, dy) in [(m.left_top(), 1.0, 1.0), (m.right_top(), -1.0, 1.0), (m.left_bottom(), 1.0, -1.0), (m.right_bottom(), -1.0, -1.0)] {
        p.line_segment([corner, corner + vec2(t * dx, 0.0)], s);
        p.line_segment([corner, corner + vec2(0.0, t * dy)], s);
    }
}

fn hud_chip(p: &egui::Painter, pos: Pos2, text: &str, color: Color32, dot: Option<Color32>) -> f32 {
    let galley = p.layout_no_wrap(text.to_owned(), FontId::proportional(12.0), color);
    let pad = if dot.is_some() { 26.0 } else { 18.0 };
    let r = Rect::from_min_size(pos, vec2(galley.size().x + pad, 26.0));
    p.rect_filled(r, CornerRadius::same(13), Color32::from_black_alpha(160));
    let mut x = r.left() + 9.0;
    if let Some(d) = dot {
        p.circle_filled(Pos2::new(r.left() + 13.0, r.center().y), 3.6, d);
        x += 10.0;
    }
    p.galley(Pos2::new(x, r.center().y - galley.size().y / 2.0), galley, color);
    r.width() + 6.0
}

fn hud_top(app: &LumenApp, ui: &Ui, p: &egui::Painter, rect: Rect) {
    // The camera OSD puts its mode badge in the top-left corner; start the chips after it.
    let mut pos = rect.left_top() + vec2(if app.cfg.show_osd { 70.0 } else { 16.0 }, 16.0);
    if app.live {
        let pulse = 0.55 + 0.45 * ((ui.input(|i| i.time) * 3.0).sin() as f32 * 0.5 + 0.5);
        pos.x += hud_chip(p, pos, "LIVE", theme::TEXT, Some(theme::DANGER.gamma_multiply(pulse)));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    } else {
        pos.x += hud_chip(p, pos, "PAUSED", theme::MUTED, Some(theme::FAINT));
    }
    // With the camera OSD on, the recording clock is shown there instead.
    if let Some(elapsed) = app.rec_elapsed().filter(|_| !app.cfg.show_osd) {
        let pulse = 0.55 + 0.45 * ((ui.input(|i| i.time) * 3.0).sin() as f32 * 0.5 + 0.5);
        pos.x += hud_chip(p, pos, &format!("REC  {}", super::format_clock(elapsed)), theme::TEXT, Some(theme::DANGER.gamma_multiply(pulse)));
    }
    if let Connection::Connected { identity, caps } = &app.connection
        && identity.vendor == crate::camera::Vendor::Canon
    {
        // Which position of the mode dial the body is in, so a switch never feels like a freeze.
        let (text, color) = if caps.record { ("VIDEO", theme::ACCENT) } else { ("PHOTO", theme::LIVE) };
        pos.x += hud_chip(p, pos, text, color, Some(color));
    }
    pos.x += hud_chip(p, pos, &format!("{}×{}", app.frame_size.0, app.frame_size.1), theme::MUTED, None);
    let busy = app.capture_pending || app.bracket.is_some();
    if app.live && !busy && app.last_frame_at.is_some_and(|t| t.elapsed() > std::time::Duration::from_millis(1800)) {
        pos.x += hud_chip(p, pos, "Waiting for the camera video feed…", theme::WARN, Some(theme::WARN));
    }
    if app.zoom > 1.01 {
        hud_chip(p, pos, &format!("{:.1}×", app.zoom), theme::ACCENT, None);
    }
}

fn histogram(p: &egui::Painter, rect: Rect, bins: &[u32], inset: f32) {
    let box_rect = Rect::from_min_size(Pos2::new(rect.left() + 16.0, rect.bottom() - inset - 84.0), vec2(212.0, 84.0));
    p.rect_filled(box_rect, CornerRadius::same(10), Color32::from_black_alpha(150));
    let plot = box_rect.shrink2(vec2(10.0, 8.0));
    let peak = (1..HIST_BINS - 1).flat_map(|i| (0..4).map(move |c| bins[c * HIST_BINS + i])).max().unwrap_or(1).max(1) as f32;
    let colors = [Color32::from_rgba_unmultiplied(255, 90, 90, 95), Color32::from_rgba_unmultiplied(90, 235, 120, 95), Color32::from_rgba_unmultiplied(90, 150, 255, 95)];
    let bw = plot.width() / HIST_BINS as f32;
    for (c, color) in colors.into_iter().enumerate() {
        let mut mesh = Mesh::default();
        for i in 0..HIST_BINS {
            let h = (bins[c * HIST_BINS + i] as f32 / peak).min(1.0) * plot.height();
            if h > 0.5 {
                mesh.add_colored_rect(Rect::from_min_max(Pos2::new(plot.left() + i as f32 * bw, plot.bottom() - h), Pos2::new(plot.left() + (i + 1) as f32 * bw + 0.5, plot.bottom())), color);
            }
        }
        p.add(Shape::mesh(mesh));
    }
    let luma: Vec<Pos2> = (0..HIST_BINS)
        .map(|i| Pos2::new(plot.left() + i as f32 * bw, plot.bottom() - (bins[3 * HIST_BINS + i] as f32 / peak).min(1.0) * plot.height()))
        .collect();
    p.add(Shape::Path(PathShape::line(luma, Stroke::new(1.2, Color32::from_white_alpha(170)))));
}

fn sharpness_meter(app: &LumenApp, p: &egui::Painter, rect: Rect, inset: f32) {
    if !app.live || app.sharpness.is_empty() {
        return;
    }
    let box_rect = Rect::from_min_size(Pos2::new(rect.right() - 196.0, rect.bottom() - inset - 84.0), vec2(180.0, 84.0));
    p.rect_filled(box_rect, CornerRadius::same(10), Color32::from_black_alpha(150));
    let current = *app.sharpness.back().unwrap_or(&0.0);
    let rel = (current / app.sharp_peak).clamp(0.0, 1.0);
    p.text(box_rect.left_top() + vec2(12.0, 11.0), Align2::LEFT_TOP, "FOCUS", FontId::proportional(10.5), theme::MUTED);
    p.text(box_rect.right_top() + vec2(-12.0, 9.0), Align2::RIGHT_TOP, format!("{current:.1}"), FontId::monospace(13.0), if rel > 0.92 { theme::OK } else { theme::TEXT });
    let plot = Rect::from_min_max(box_rect.left_top() + vec2(12.0, 34.0), box_rect.right_bottom() - vec2(12.0, 12.0));
    let n = app.sharpness.len();
    let pts: Vec<Pos2> = app
        .sharpness
        .iter()
        .enumerate()
        .map(|(i, v)| Pos2::new(plot.left() + plot.width() * i as f32 / (n.max(2) - 1) as f32, plot.bottom() - plot.height() * (v / app.sharp_peak).clamp(0.0, 1.0)))
        .collect();
    p.add(Shape::Path(PathShape::line(pts, Stroke::new(1.6, if rel > 0.92 { theme::OK } else { theme::LIVE }))));
}

fn empty_state(app: &mut LumenApp, ui: &mut Ui, rect: Rect) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(30.0)).layout(Layout::top_down(Align::Center)));
    child.add_space((rect.height() / 2.0 - 110.0).max(10.0));
    let (title, body) = match &app.connection {
        Connection::Disconnected => ("Connect a camera to start shooting", "Live view, exposure control, focus drive and focus bracketing run over USB. Try the built-in virtual camera to explore every feature."),
        Connection::Connecting => ("Opening session…", "Negotiating with the camera."),
        Connection::Connected { caps, .. } if !caps.live_view => ("This camera has no live view over USB", "You can still change settings and capture; images are downloaded to your output folder."),
        Connection::Connected { .. } => ("Starting live view…", "Press Start live view if nothing appears."),
    };
    let (icon_rect, _) = child.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
    child.painter().circle_filled(icon_rect.center(), 28.0, theme::ACCENT.gamma_multiply(0.1));
    widgets::paint_icon(child.painter(), Icon::Live, icon_rect.center(), 30.0, theme::ACCENT);
    child.add_space(8.0);
    child.label(RichText::new(title).size(19.0).strong());
    child.add_space(2.0);
    child.scope(|ui| {
        ui.set_max_width(460.0);
        ui.label(RichText::new(body).color(theme::MUTED));
    });
    if matches!(app.connection, Connection::Disconnected) {
        child.add_space(12.0);
        if let Some(i) = app.devices.iter().position(|d| d.kind == crate::link::DeviceKind::Simulated)
            && widgets::button(&mut child, "Connect virtual camera", Some(Icon::Plug), Kind::Primary).clicked()
        {
            app.selected_device = i;
            app.connect_selected();
        }
    }
}

fn controls(app: &mut LumenApp, ui: &mut Ui) {
    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0, theme::STROKE))
        .corner_radius(theme::radius())
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_height(CONTROLS_HEIGHT - 20.0);
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.horizontal(|ui| {
                action_buttons(app, ui);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let ctx = ui.ctx().clone();
                    let (label, icon) = if app.fullscreen { ("Exit", Icon::FullscreenExit) } else { ("Fullscreen", Icon::Fullscreen) };
                    if widgets::button(ui, label, Some(icon), Kind::Secondary).on_hover_text("Full-screen live view (F)").clicked() {
                        app.toggle_fullscreen(&ctx);
                    }
                });
            });
            ui.horizontal(|ui| {
                view_toggles(app, ui);
                if app.audio_on {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| bar_meters(app, ui));
                }
            });
        });
}

/// Live / record / shutter / autofocus, shared by the control bar and the fullscreen toolbar.
fn action_buttons(app: &mut LumenApp, ui: &mut Ui) {
    let connected = app.is_connected();
    let caps = app.caps();
    ui.add_enabled_ui(connected && caps.live_view, |ui| {
        let (label, icon) = if app.live { ("Stop", Icon::Stop) } else { ("Live view", Icon::Play) };
        if widgets::button(ui, label, Some(icon), Kind::Secondary).clicked() {
            app.toggle_live();
        }
    });
    ui.add_enabled_ui(connected && caps.record, |ui| {
        let (label, kind) = if app.recording { ("Stop rec", Kind::Recording) } else { ("Record", Kind::Danger) };
        let hint = if caps.record { "Start / stop video recording (R). The camera must be in video mode." } else { "Turn the camera mode dial to video to record (the camera currently reports photo mode)" };
        if widgets::button(ui, label, Some(Icon::Record), kind).on_hover_text(hint).clicked() {
            app.toggle_record();
        }
    });
    ui.add_enabled_ui(connected && app.bracket.is_none() && !app.recording, |ui| {
        if widgets::shutter_button(ui, app.capture_pending).on_hover_text("Capture (Space)").clicked() {
            app.capture();
        }
    });
    ui.add_enabled_ui(connected && caps.autofocus, |ui| {
        if widgets::button(ui, "Autofocus", Some(Icon::Target), Kind::Secondary).on_hover_text("Trigger autofocus (A)").clicked() {
            app.engine.send(Command::Autofocus);
        }
    });
}

fn view_toggles(app: &mut LumenApp, ui: &mut Ui) {
    if widgets::chip(ui, "Peaking", Icon::Peak, app.cfg.show_peaking).on_hover_text("Highlight in-focus edges (P)").clicked() {
        app.cfg.show_peaking = !app.cfg.show_peaking;
        app.push_peaking();
    }
    if widgets::chip(ui, "Grid", Icon::Grid, app.cfg.show_grid).on_hover_text("Rule of thirds (G)").clicked() {
        app.cfg.show_grid = !app.cfg.show_grid;
    }
    if widgets::chip(ui, "Histogram", Icon::Histogram, app.cfg.show_histogram).clicked() {
        app.cfg.show_histogram = !app.cfg.show_histogram;
    }
    let audio_hint = if app.audio_on { "Stop the audio monitor" } else { "Start the audio level monitor" };
    if widgets::chip(ui, "Audio", Icon::Audio, app.audio_on).on_hover_text(audio_hint).clicked() {
        app.toggle_audio();
    }
    if widgets::chip(ui, "Camera info", Icon::Info, app.cfg.show_osd).on_hover_text("Show the camera's on-screen display (I)").clicked() {
        app.cfg.show_osd = !app.cfg.show_osd;
    }
}

/// Compact stereo meters for the control bar (drawn right-to-left, so R first).
fn bar_meters(app: &LumenApp, ui: &mut Ui) {
    let channels = app.audio_channels.clamp(1, 2);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        for i in 0..channels {
            let (rect, _) = ui.allocate_exact_size(vec2(150.0, 9.0), Sense::hover());
            meters::draw_horizontal(ui.painter(), rect, &app.meters[i]);
        }
    });
    if app.audio_silent() {
        ui.label(RichText::new("no signal").size(11.0).color(theme::WARN));
    } else if app.audio_clip {
        ui.label(RichText::new("CLIP").size(11.0).color(theme::DANGER).strong());
    }
}

// ------------------------------------------------------------ camera OSD

/// First setting present among the candidate property codes (EOS or standard PTP).
fn setting<'a>(app: &'a LumenApp, codes: &[u16]) -> Option<&'a crate::camera::Setting> {
    codes.iter().find_map(|c| app.settings.iter().find(|s| s.prop == *c))
}

fn gradient_bar(p: &egui::Painter, rect: Rect, top_alpha: u8, bottom_alpha: u8) {
    let mut mesh = Mesh::default();
    let (a, b) = (Color32::from_black_alpha(top_alpha), Color32::from_black_alpha(bottom_alpha));
    mesh.colored_vertex(rect.left_top(), a);
    mesh.colored_vertex(rect.right_top(), a);
    mesh.colored_vertex(rect.right_bottom(), b);
    mesh.colored_vertex(rect.left_bottom(), b);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(Shape::mesh(mesh));
}

/// Short mode badge as printed on a camera's mode dial.
fn mode_badge(text: &str) -> String {
    match text {
        "Manual" => "M".into(),
        "Program" => "P".into(),
        "Shutter priority" => "Tv".into(),
        "Aperture priority" => "Av".into(),
        "Bulb" => "B".into(),
        other => other.chars().take(3).collect(),
    }
}

/// Camera-style information display: mode, recording clock and battery on top;
/// shutter, aperture, exposure meter, ISO and the rest along the bottom.
fn osd(app: &LumenApp, p: &egui::Painter, rect: Rect) {
    // ---- top bar
    let top = Rect::from_min_size(rect.min, vec2(rect.width(), 54.0));
    gradient_bar(p, top, 190, 0);
    let mode = setting(app, &[eos::AE_MODE, prop::EXPOSURE_PROGRAM]).map(|s| mode_badge(&s.text));
    if let Some(mode) = mode {
        let badge = Rect::from_min_size(rect.left_top() + vec2(16.0, 14.0), vec2(40.0, 28.0));
        p.rect_stroke(badge, CornerRadius::same(7), Stroke::new(1.6, theme::TEXT), StrokeKind::Inside);
        p.text(badge.center(), Align2::CENTER_CENTER, mode, FontId::proportional(15.0), theme::TEXT);
    }
    if let Some(elapsed) = app.rec_elapsed() {
        let c = Pos2::new(rect.center().x, rect.top() + 28.0);
        p.circle_filled(c - vec2(50.0, 0.0), 6.0, theme::DANGER);
        p.text(c - vec2(36.0, 0.0), Align2::LEFT_CENTER, super::format_clock(elapsed), FontId::monospace(22.0), theme::TEXT);
    }
    if app.audio_on {
        let x = rect.right() - 190.0;
        for i in 0..app.audio_channels.clamp(1, 2) {
            let bar = Rect::from_min_size(Pos2::new(x - 120.0, rect.top() + 19.0 + i as f32 * 9.0), vec2(110.0, 6.0));
            meters::draw_horizontal(p, bar, &app.meters[i]);
        }
        p.text(Pos2::new(x - 128.0, rect.top() + 28.0), Align2::RIGHT_CENTER, if app.audio_silent() { "no signal" } else { "audio" }, FontId::proportional(10.5), if app.audio_silent() { theme::WARN } else { theme::MUTED });
    }
    if let Some(level) = app.battery() {
        let right = rect.right() - 18.0;
        p.text(Pos2::new(right - 40.0, rect.top() + 28.0), Align2::RIGHT_CENTER, format!("{level}%"), FontId::monospace(15.0), theme::TEXT);
        widgets::battery_glyph(p, Rect::from_min_size(Pos2::new(right - 34.0, rect.top() + 20.0), vec2(34.0, 16.0)), level);
    }

    // ---- bottom bar
    let bottom = Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - 76.0), rect.right_bottom());
    gradient_bar(p, bottom, 0, 200);
    type Slot<'a> = (&'a str, Option<String>);
    let slots: [Slot; 6] = [
        ("SHUTTER", setting(app, &[eos::TV, prop::EXPOSURE_TIME]).map(|s| s.text.clone())),
        ("APERTURE", setting(app, &[eos::AV, prop::F_NUMBER]).map(|s| s.text.clone())),
        ("EXPOSURE", None), // drawn as a scale below
        ("ISO", setting(app, &[eos::ISO, prop::EXPOSURE_INDEX]).map(|s| s.text.trim_start_matches("ISO ").to_owned())),
        ("WB", setting(app, &[eos::WHITE_BALANCE, prop::WHITE_BALANCE]).map(|s| s.text.clone())),
        ("FOCUS", setting(app, &[eos::FOCUS_MODE, prop::FOCUS_MODE]).map(|s| s.text.clone())),
    ];
    let slot_w = (rect.width() - 32.0) / slots.len() as f32;
    for (i, (caption, value)) in slots.iter().enumerate() {
        let x = rect.left() + 16.0 + slot_w * (i as f32 + 0.5);
        p.text(Pos2::new(x, bottom.bottom() - 52.0), Align2::CENTER_CENTER, caption, FontId::proportional(10.0), theme::MUTED);
        if *caption == "EXPOSURE" {
            exposure_scale(app, p, Pos2::new(x, bottom.bottom() - 26.0), slot_w.min(190.0));
        } else if let Some(v) = value {
            p.text(Pos2::new(x, bottom.bottom() - 26.0), Align2::CENTER_CENTER, v, FontId::proportional(22.0), theme::TEXT);
        }
    }
}

/// Vertical L/R level bars along the right edge.
fn audio_meters(app: &LumenApp, p: &egui::Painter, rect: Rect, inset: f32) {
    let channels = app.audio_channels.clamp(1, 2);
    let top = rect.top() + 66.0;
    let bottom = rect.bottom() - inset - 100.0;
    if bottom - top < 60.0 {
        return;
    }
    let panel = Rect::from_min_max(Pos2::new(rect.right() - 20.0 - 18.0 * channels as f32 - 14.0, top - 8.0), Pos2::new(rect.right() - 14.0, bottom + 22.0));
    p.rect_filled(panel, CornerRadius::same(10), Color32::from_black_alpha(140));
    for i in 0..channels {
        let x = panel.left() + 12.0 + i as f32 * 18.0;
        let bar = Rect::from_min_max(Pos2::new(x, top), Pos2::new(x + 10.0, bottom));
        meters::draw_vertical(p, bar, &app.meters[i]);
        let name = if channels == 1 { "M" } else if i == 0 { "L" } else { "R" };
        p.text(Pos2::new(x + 5.0, bottom + 11.0), Align2::CENTER_CENTER, name, FontId::proportional(10.0), theme::MUTED);
    }
    if app.audio_clip {
        p.circle_filled(Pos2::new(panel.center().x, top - 1.0), 3.5, theme::DANGER);
    }
}

/// -3 .. +3 EV meter with a marker at the current exposure compensation.
fn exposure_scale(app: &LumenApp, p: &egui::Painter, centre: Pos2, width: f32) {
    let Some(s) = setting(app, &[eos::EXP_COMP, prop::EXPOSURE_BIAS]) else { return };
    let ev: f32 = s.text.trim_end_matches(" EV").parse().unwrap_or(0.0);
    let half = width / 2.0 - 6.0;
    p.line_segment([centre - vec2(half, 0.0), centre + vec2(half, 0.0)], Stroke::new(1.0, theme::FAINT));
    for k in -3..=3 {
        let x = centre.x + half * k as f32 / 3.0;
        let h = if k == 0 { 7.0 } else { 4.0 };
        p.line_segment([Pos2::new(x, centre.y - h), Pos2::new(x, centre.y + h)], Stroke::new(1.0, theme::MUTED));
    }
    let x = centre.x + half * (ev / 3.0).clamp(-1.0, 1.0);
    p.add(Shape::convex_polygon(vec![Pos2::new(x - 5.0, centre.y - 14.0), Pos2::new(x + 5.0, centre.y - 14.0), Pos2::new(x, centre.y - 6.0)], theme::ACCENT, Stroke::NONE));
}