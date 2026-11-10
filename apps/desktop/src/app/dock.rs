//! Bottom dock (captures and activity log) and toast notifications.

use egui::{Align2, Color32, Context, CornerRadius, FontId, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, vec2};

use super::widgets::{self, Icon, Kind};
use super::{DockTab, LumenApp, theme};
use crate::engine::Level;

pub fn level_color(level: Level) -> Color32 {
    match level {
        Level::Info => theme::MUTED,
        Level::Success => theme::OK,
        Level::Warn => theme::WARN,
        Level::Error => theme::DANGER,
    }
}

pub fn show(app: &mut LumenApp, ui: &mut Ui) {
    egui::Frame::new()
        .fill(theme::PANEL)
        .stroke(Stroke::new(1.0, theme::STROKE))
        .corner_radius(theme::radius())
        .inner_margin(egui::Margin::symmetric(14, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                tab(ui, app, DockTab::Captures, &format!("Captures ({})", app.captures.len()));
                tab(ui, app, DockTab::Activity, "Activity");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, icon) = if app.dock_open { ("Collapse", Icon::ChevronRight) } else { ("Expand", Icon::ChevronLeft) };
                    if widgets::button(ui, label, Some(icon), Kind::Ghost).clicked() {
                        app.dock_open = !app.dock_open;
                    }
                    let show_folder = app.dock_tab == DockTab::Captures && !app.captures.is_empty();
                    if show_folder && widgets::button(ui, "Open folder", Some(Icon::Folder), Kind::Ghost).clicked() {
                        LumenApp::open_in_explorer(&app.cfg.output_dir);
                    }
                });
            });
            if !app.dock_open {
                return;
            }
            ui.add_space(4.0);
            match app.dock_tab {
                DockTab::Captures => captures(app, ui),
                DockTab::Activity => activity(app, ui),
            }
        });
}

fn tab(ui: &mut Ui, app: &mut LumenApp, which: DockTab, label: &str) {
    let active = app.dock_tab == which;
    let color = if active { theme::TEXT } else { theme::MUTED };
    let galley = ui.painter().layout_no_wrap(label.to_owned(), FontId::proportional(13.0), color);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 24.0, 30.0), Sense::click());
    if active {
        ui.painter().rect_filled(rect, CornerRadius::same(8), theme::CARD);
        ui.painter().rect_filled(egui::Rect::from_min_size(rect.left_bottom() - vec2(-10.0, 2.0), vec2(rect.width() - 20.0, 2.0)), CornerRadius::same(1), theme::ACCENT);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(8), theme::CARD.gamma_multiply(0.5));
    }
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    if resp.clicked() {
        app.dock_tab = which;
        app.dock_open = true;
    }
}

fn captures(app: &mut LumenApp, ui: &mut Ui) {
    if app.captures.is_empty() {
        ui.add_space(26.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("No captures yet").size(14.0).strong());
            ui.label(RichText::new("Press the shutter button or Space. Images land in your output folder.").size(12.0).color(theme::MUTED));
        });
        return;
    }
    let mut open = None;
    let mut select = None;
    ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal(|ui| {
            for (i, item) in app.captures.iter().enumerate().rev() {
                let selected = app.selected_capture == Some(i);
                let (rect, resp) = ui.allocate_exact_size(vec2(148.0, 112.0), Sense::click());
                let hover = resp.hovered();
                ui.painter().rect_filled(rect, CornerRadius::same(10), if hover { theme::CARD_HOVER } else { theme::CARD });
                let thumb_rect = rect.shrink2(vec2(6.0, 6.0)).with_max_y(rect.bottom() - 26.0);
                match &item.texture {
                    Some(tex) => {
                        let s = tex.size_vec2();
                        let k = (thumb_rect.width() / s.x).min(thumb_rect.height() / s.y);
                        let r = egui::Rect::from_center_size(thumb_rect.center(), s * k);
                        ui.painter().image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    None => widgets::paint_icon(ui.painter(), Icon::Image, thumb_rect.center(), 30.0, theme::FAINT),
                }
                ui.painter().text(egui::pos2(rect.center().x, rect.bottom() - 13.0), Align2::CENTER_CENTER, &item.label, FontId::proportional(11.5), theme::MUTED);
                ui.painter().rect_stroke(rect, CornerRadius::same(10), Stroke::new(if selected { 1.6 } else { 1.0 }, if selected { theme::ACCENT } else { theme::STROKE }), StrokeKind::Inside);
                if resp.clicked() {
                    select = Some(i);
                }
                if resp.double_clicked() {
                    open = item.path.clone();
                }
                resp.on_hover_text(if item.path.is_some() { "Double-click to open" } else { "Stored on the camera card" });
            }
        });
    });
    if let Some(i) = select {
        app.selected_capture = Some(i);
    }
    if let Some(p) = open {
        LumenApp::open_file(&p);
    }
}

fn activity(app: &LumenApp, ui: &mut Ui) {
    ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        if app.logs.is_empty() {
            ui.label(RichText::new("Nothing has happened yet.").color(theme::FAINT));
        }
        for line in &app.logs {
            ui.horizontal(|ui| {
                let s = line.at.as_secs();
                ui.label(RichText::new(format!("{:02}:{:02}", s / 60, s % 60)).monospace().size(11.5).color(theme::FAINT));
                ui.label(RichText::new(&line.text).size(12.5).color(level_color(line.level)));
            });
        }
    });
}

pub fn toasts(app: &LumenApp, ctx: &Context) {
    for (i, t) in app.toasts.iter().rev().take(4).enumerate() {
        let age = t.born.elapsed().as_secs_f32();
        let alpha = ((4.0 - age) * 2.0).clamp(0.0, 1.0).min((age * 8.0).clamp(0.0, 1.0));
        egui::Area::new(egui::Id::NULL.with(("toast", i)))
            .order(egui::Order::Tooltip)
            .anchor(Align2::RIGHT_TOP, vec2(-22.0, 74.0 + i as f32 * 58.0))
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                egui::Frame::new()
                    .fill(theme::CARD_HOVER)
                    .stroke(Stroke::new(1.0, level_color(t.level).gamma_multiply(0.6)))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .shadow(egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(120) })
                    .show(ui, |ui| {
                        ui.set_max_width(380.0);
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 4.0, level_color(t.level));
                            ui.label(RichText::new(&t.text).size(13.0));
                        });
                    });
            });
    }
}
