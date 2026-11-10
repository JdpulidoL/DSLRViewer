//! Top bar: brand, device picker and connection state.

use egui::{Align, Layout, Pos2, RichText, Stroke, Ui, vec2};

use super::widgets::{self, Icon, Kind};
use super::{Connection, LumenApp, theme};
use crate::engine::Command;

pub fn show(app: &mut LumenApp, ui: &mut Ui) {
    ui.horizontal_centered(|ui| {
        brand(ui);
        ui.add_space(22.0);

        let connected = app.is_connected();
        let busy = matches!(app.connection, Connection::Connecting);

        ui.add_enabled_ui(!connected && !busy, |ui| {
            let selected = app.devices.get(app.selected_device).map(|d| d.name.clone()).unwrap_or_else(|| "No cameras found".into());
            egui::ComboBox::from_id_salt("device")
                .width(250.0)
                .truncate()
                .height(34.0)
                .selected_text(RichText::new(selected).size(13.5))
                .show_ui(ui, |ui| {
                    for (i, d) in app.devices.iter().enumerate() {
                        ui.selectable_value(&mut app.selected_device, i, format!("{}  ·  {}", d.name, d.detail));
                    }
                });
            if widgets::icon_button(ui, Icon::Refresh, Kind::Secondary).on_hover_text("Rescan for cameras").clicked() {
                app.scan();
            }
        });

        if connected {
            if widgets::button(ui, "Disconnect", Some(Icon::Plug), Kind::Secondary).clicked() {
                app.engine.send(Command::Disconnect);
            }
        } else {
            ui.add_enabled_ui(!busy && !app.devices.is_empty(), |ui| {
                let label = if busy { "Connecting…" } else { "Connect" };
                if widgets::button(ui, label, Some(Icon::Plug), Kind::Primary).clicked() {
                    app.connect_selected();
                }
            });
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            match &app.connection {
                Connection::Connected { identity, .. } => {
                    widgets::pill(ui, &identity.title(), theme::OK);
                }
                Connection::Connecting => widgets::pill(ui, "Connecting", theme::WARN),
                Connection::Disconnected => widgets::pill(ui, "No camera", theme::FAINT),
            }
            if let Some(level) = app.battery() {
                ui.add_space(4.0);
                ui.label(RichText::new(format!("{level}%")).monospace().color(theme::MUTED));
                let (rect, _) = ui.allocate_exact_size(vec2(30.0, 14.0), egui::Sense::hover());
                widgets::battery_glyph(ui.painter(), rect, level);
            }
            if let Some(elapsed) = app.rec_elapsed() {
                widgets::pill(ui, &format!("REC {}", super::format_clock(elapsed)), theme::DANGER);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            }
            if app.live && app.fps > 0.0 {
                ui.label(RichText::new(format!("{:.0} fps", app.fps)).monospace().color(theme::MUTED));
            }
        });
    });
}

fn brand(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(34.0, 34.0), egui::Sense::hover());
    let c = rect.center();
    let p = ui.painter();
    p.circle_filled(c, 16.0, theme::ACCENT.gamma_multiply(0.14));
    p.circle_stroke(c, 14.0, Stroke::new(2.0, theme::ACCENT));
    for k in 0..6 {
        let a = k as f32 * std::f32::consts::TAU / 6.0;
        let (a0, a1) = (a, a + 1.9);
        p.line_segment(
            [Pos2::new(c.x + a0.cos() * 5.0, c.y + a0.sin() * 5.0), Pos2::new(c.x + a1.cos() * 12.5, c.y + a1.sin() * 12.5)],
            Stroke::new(1.5, theme::ACCENT),
        );
    }
    ui.add_space(2.0);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.label(RichText::new("Lumen").size(18.0).strong());
        ui.label(RichText::new("TETHER STUDIO").size(9.5).color(theme::MUTED));
    });
}
