//! Left column: camera identity and exposure settings.

use egui::{RichText, ScrollArea, Ui};

use super::widgets::{self, Icon, Kind};
use super::{Connection, LumenApp, theme};
use crate::camera::Setting;
use crate::engine::Command;
use crate::ptp::codes::prop;

pub fn show(app: &mut LumenApp, ui: &mut Ui) {
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 12.0;
        identity_card(app, ui);
        settings_card(app, ui);
    });
}

fn identity_card(app: &LumenApp, ui: &mut Ui) {
    widgets::card(ui, "Camera", |ui| match &app.connection {
        Connection::Connected { identity, caps } => {
            ui.label(RichText::new(identity.title()).size(16.0).strong());
            ui.label(RichText::new(format!("S/N {}", identity.serial)).size(12.0).color(theme::MUTED));
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                widgets::tag(ui, identity.vendor.label(), theme::LIVE);
                widgets::tag(ui, if caps.live_view { "Live view" } else { "No live view" }, if caps.live_view { theme::OK } else { theme::FAINT });
                widgets::tag(ui, if caps.focus_drive { "Focus drive" } else { "No focus drive" }, if caps.focus_drive { theme::OK } else { theme::FAINT });
                widgets::tag(ui, if caps.autofocus { "AF trigger" } else { "No AF trigger" }, if caps.autofocus { theme::OK } else { theme::FAINT });
            });
            ui.add_space(2.0);
            ui.label(RichText::new(&identity.transport).size(11.5).color(theme::FAINT));
        }
        Connection::Connecting => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Opening PTP session…").color(theme::MUTED));
            });
        }
        Connection::Disconnected => {
            ui.label(RichText::new("No camera connected").size(15.0).strong());
            ui.label(
                RichText::new("Plug in a camera over USB, set it to PTP/PC-connect mode and press Connect. The built-in virtual camera works without hardware.")
                    .size(12.0)
                    .color(theme::MUTED),
            );
        }
    });
}

fn settings_card(app: &mut LumenApp, ui: &mut Ui) {
    let connected = app.is_connected();
    widgets::card(ui, "Exposure", |ui| {
        if !connected {
            ui.label(RichText::new("Settings appear here once a camera is connected.").size(12.0).color(theme::FAINT));
            return;
        }
        if app.settings.is_empty() {
            ui.label(RichText::new("This camera does not expose adjustable settings over PTP.").size(12.0).color(theme::MUTED));
            return;
        }
        let mut change = None;
        for s in &app.settings {
            if s.prop == prop::BATTERY_LEVEL {
                continue;
            }
            let flash = app.hand_changed.get(&s.prop).map_or(0.0, |t| (1.0 - t.elapsed().as_secs_f32() / FLASH_SECS).clamp(0.0, 1.0));
            if flash > 0.0 {
                ui.ctx().request_repaint();
            }
            if let Some(v) = setting_row(ui, s, flash) {
                change = Some((s.prop, v));
            }
        }
        if let Some((prop, value)) = change {
            app.engine.send(Command::SetSetting { prop, value });
        }
        if let Some(b) = app.settings.iter().find(|s| s.prop == prop::BATTERY_LEVEL) {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                widgets::field_label(ui, "Battery");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(&b.text).monospace());
                });
            });
            let color = if b.current > 30 { theme::OK } else if b.current > 12 { theme::WARN } else { theme::DANGER };
            widgets::progress_bar(ui, b.current as f32 / 100.0, color);
        }
    });
}

/// How long a hand change on the camera stays highlighted.
const FLASH_SECS: f32 = 3.0;

/// One setting: label, stepper arrows and dropdown. Returns a newly chosen value.
/// `flash` (0..1) fades an accent marker for values the user just changed on the camera body.
fn setting_row(ui: &mut Ui, s: &Setting, flash: f32) -> Option<i64> {
    let mut chosen = None;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.horizontal(|ui| {
            widgets::field_label(ui, s.label);
            if flash > 0.0 {
                widgets::tag(ui, "changed on camera", theme::ACCENT.gamma_multiply(0.4 + 0.6 * flash));
            }
        });
        ui.horizontal(|ui| {
            ui.add_enabled_ui(s.writable, |ui| {
                if widgets::icon_button(ui, Icon::ChevronLeft, Kind::Secondary).clicked() {
                    chosen = s.neighbour(-1);
                }
                // Reserve room for the right-hand stepper (34 px) and two item gaps.
                let width = ui.available_width() - 34.0 - 2.0 * ui.spacing().item_spacing.x;
                egui::ComboBox::from_id_salt(("setting", s.prop))
                    .width(width.max(80.0))
                    .height(300.0)
                    .selected_text(RichText::new(&s.text).size(13.5))
                    .show_ui(ui, |ui| {
                        for (value, text) in &s.choices {
                            if ui.selectable_label(*value == s.current, text).clicked() {
                                chosen = Some(*value);
                            }
                        }
                    });
                if widgets::icon_button(ui, Icon::ChevronRight, Kind::Secondary).clicked() {
                    chosen = s.neighbour(1);
                }
            });
        });
    });
    chosen
}
