//! Right column: focus control, focus bracketing and stacking.

use std::time::{Duration, Instant};

use egui::{RichText, ScrollArea, Sense, Ui, vec2};

use super::widgets::{self, Icon, Kind};
use super::{FOCUS_STEPS, LumenApp, theme};
use crate::camera::FocusDirection;
use crate::engine::Command;

pub fn show(app: &mut LumenApp, ui: &mut Ui) {
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 12.0;
        focus_card(app, ui);
        audio_card(app, ui);
        bracket_card(app, ui);
        stack_card(app, ui);
        output_card(app, ui);
    });
}

fn focus_card(app: &mut LumenApp, ui: &mut Ui) {
    let enabled = app.is_connected() && app.caps().focus_drive && app.bracket.is_none();
    widgets::card(ui, "Focus", |ui| {
        if app.is_connected() && !app.caps().focus_drive {
            ui.label(RichText::new("This camera does not expose focus drive over USB.").size(12.0).color(theme::MUTED));
        }
        ui.add_enabled_ui(enabled, |ui| {
            let mut idx = app.cfg.focus_step_index;
            let labels: Vec<&str> = FOCUS_STEPS.iter().map(|(l, _)| *l).collect();
            if widgets::segmented(ui, &labels, &mut idx) {
                app.cfg.focus_step_index = idx;
            }
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let half = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
                for (dir, label, icon) in [(FocusDirection::Near, "Nearer", Icon::ChevronLeft), (FocusDirection::Far, "Farther", Icon::ChevronRight)] {
                    ui.scope(|ui| {
                        ui.set_width(half);
                        hold_button(app, ui, dir, label, icon, half);
                    });
                }
            });
            ui.label(RichText::new("Hold to keep driving  ·  ← → keys").size(11.5).color(theme::FAINT));
        });
    });
}

/// Button that fires once on press and repeats while held.
fn hold_button(app: &mut LumenApp, ui: &mut Ui, dir: FocusDirection, label: &str, icon: Icon, width: f32) {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 40.0), Sense::click());
    let enabled = ui.is_enabled();
    let down = resp.is_pointer_button_down_on() && enabled;
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() && enabled, 0.12);
    let bg = if down { theme::ACCENT_DIM } else { theme::CARD_HOVER.gamma_multiply(0.5 + 0.5 * t) };
    ui.painter().rect_filled(rect, egui::CornerRadius::same(10), bg);
    ui.painter().rect_stroke(rect, egui::CornerRadius::same(10), egui::Stroke::new(1.0, if down { theme::ACCENT } else { theme::STROKE }), egui::StrokeKind::Inside);
    let color = if !enabled { theme::FAINT } else if down { theme::ACCENT } else { theme::TEXT };
    let galley = ui.painter().layout_no_wrap(label.to_owned(), egui::FontId::proportional(13.5), color);
    let total = galley.size().x + 24.0;
    let x0 = rect.center().x - total / 2.0;
    let icon_x = if dir == FocusDirection::Near { x0 + 9.0 } else { x0 + total - 9.0 };
    let text_x = if dir == FocusDirection::Near { x0 + 24.0 } else { x0 };
    widgets::paint_icon(ui.painter(), icon, egui::Pos2::new(icon_x, rect.center().y), 18.0, color);
    ui.painter().galley(egui::Pos2::new(text_x, rect.center().y - galley.size().y / 2.0), galley, color);
    if down && app.focus_hold.is_none() {
        app.drive_focus(dir);
        app.focus_hold = Some((dir, Instant::now() + Duration::from_millis(420)));
    }
}

fn bracket_card(app: &mut LumenApp, ui: &mut Ui) {
    let connected = app.is_connected();
    let ready = connected && app.caps().focus_drive;
    widgets::card(ui, "Focus bracketing", |ui| {
        ui.add_enabled_ui(ready && app.bracket.is_none(), |ui| {
            egui::Grid::new("bracket-grid").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                widgets::field_label(ui, "Shots");
                ui.add(egui::DragValue::new(&mut app.cfg.bracket_shots).range(2..=300).speed(0.2));
                ui.end_row();
                widgets::field_label(ui, "Step");
                ui.add(egui::DragValue::new(&mut app.cfg.bracket_step).range(1..=2000).speed(0.5).suffix(" steps"));
                ui.end_row();
                widgets::field_label(ui, "Settle");
                ui.add(egui::DragValue::new(&mut app.cfg.settle_ms).range(0..=5000).speed(5.0).suffix(" ms"));
                ui.end_row();
            });
            let mut dir = usize::from(app.cfg.bracket_far_first);
            if widgets::segmented(ui, &["Near → Far", "Far → Near"], &mut dir) {
                app.cfg.bracket_far_first = dir == 1;
            }
            ui.checkbox(&mut app.cfg.auto_stack, "Stack automatically when finished");
        });
        match app.bracket {
            Some((done, total)) => {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Shot {done} of {total}")).monospace());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::button(ui, "Cancel", Some(Icon::Stop), Kind::Danger).clicked() {
                            app.engine.send(Command::CancelBracket);
                        }
                    });
                });
                widgets::progress_bar(ui, done as f32 / total.max(1) as f32, theme::ACCENT);
            }
            None => {
                ui.add_enabled_ui(ready, |ui| {
                    let label = format!("Start bracket ({} shots)", app.cfg.bracket_shots);
                    if widgets::button(ui, &label, Some(Icon::Layers), Kind::Primary).clicked() {
                        app.start_bracket();
                    }
                });
                if !connected {
                    ui.label(RichText::new("Connect a camera with focus drive to enable bracketing.").size(11.5).color(theme::FAINT));
                }
            }
        }
    });
}

fn stack_card(app: &mut LumenApp, ui: &mut Ui) {
    widgets::card(ui, "Focus stacking", |ui| {
        let mut method = usize::from(app.cfg.stack_sharpest);
        if widgets::segmented(ui, &["Weighted", "Sharpest"], &mut method) {
            app.cfg.stack_sharpest = method == 1;
        }
        ui.horizontal(|ui| {
            widgets::field_label(ui, "Smoothing");
            ui.add(egui::Slider::new(&mut app.cfg.stack_smoothing, 0..=10).show_value(true));
        });
        let seq = app.last_sequence.len();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(seq >= 2 && !app.stack.running, |ui| {
                if widgets::button(ui, &format!("Stack last sequence ({seq})"), Some(Icon::Layers), Kind::Secondary).clicked() {
                    let paths = app.last_sequence.clone();
                    app.start_stack(paths);
                }
            });
            let pick = widgets::button(ui, "Files…", Some(Icon::Folder), Kind::Ghost).on_hover_text("Choose images to stack").clicked();
            if pick && let Some(files) = rfd::FileDialog::new().add_filter("Images", &["jpg", "jpeg", "png", "tif", "tiff"]).pick_files() {
                app.start_stack(files);
            }
        });
        if app.stack.running {
            widgets::progress_bar(ui, app.stack.progress, theme::LIVE);
            ui.label(RichText::new("Merging…").size(12.0).color(theme::MUTED));
        }
        if let Some((path, tex)) = &app.stack.result {
            ui.add_space(2.0);
            if let Some(tex) = tex {
                let w = ui.available_width();
                let size = tex.size_vec2();
                let r = ui.add(egui::Image::new((tex.id(), vec2(w, w * size.y / size.x))).corner_radius(8).sense(Sense::click()));
                if r.on_hover_text("Open result").clicked() {
                    LumenApp::open_file(path);
                }
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).size(12.0).color(theme::MUTED));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::button(ui, "Show", Some(Icon::Folder), Kind::Ghost).clicked() {
                        LumenApp::open_in_explorer(path);
                    }
                });
            });
        }
    });
}

/// Level meters, device choice and monitor playback for a PC audio input.
fn audio_card(app: &mut LumenApp, ui: &mut Ui) {
    widgets::card(ui, "Audio monitor", |ui| {
        let default_label = "System default input";
        let selected = app.cfg.audio_device.clone().unwrap_or_else(|| default_label.to_owned());
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!app.audio_on, |ui| {
                if widgets::icon_button(ui, Icon::Refresh, Kind::Secondary).on_hover_text("Rescan audio inputs").clicked() {
                    app.refresh_audio_devices();
                }
                let width = ui.available_width();
                egui::ComboBox::from_id_salt("audio-device").width(width).truncate().selected_text(RichText::new(selected).size(13.0)).show_ui(ui, |ui| {
                    if ui.selectable_label(app.cfg.audio_device.is_none(), default_label).clicked() {
                        app.cfg.audio_device = None;
                    }
                    for name in app.audio_devices.clone() {
                        if ui.selectable_label(app.cfg.audio_device.as_deref() == Some(name.as_str()), &name).clicked() {
                            app.cfg.audio_device = Some(name);
                        }
                    }
                });
            });
        });
        let (label, icon, kind) = if app.audio_on { ("Stop monitoring", Icon::Stop, Kind::Secondary) } else { ("Start monitoring", Icon::Play, Kind::Primary) };
        if widgets::button(ui, label, Some(icon), kind).clicked() {
            app.toggle_audio();
        }
        if !app.audio_on {
            ui.label(
                RichText::new("Cameras do not send sound over USB. Pick the PC input that carries it: a microphone, or the camera\u{2019}s audio out into a line-in / capture device.")
                    .size(11.5)
                    .color(theme::FAINT),
            );
            return;
        }
        if let Some(info) = &app.audio_info {
            ui.label(RichText::new(info).size(11.5).color(theme::MUTED));
        }
        if app.audio_silent() {
            ui.label(
                RichText::new("No sound on this input. Check that the source is connected, not muted, and that Windows allows microphone access (Settings > Privacy > Microphone).")
                    .size(11.5)
                    .color(theme::WARN),
            );
        }
        let channels = app.audio_channels.clamp(1, 2);
        for (i, name) in ["L", "R"].into_iter().enumerate().take(channels) {
            ui.horizontal(|ui| {
                ui.label(RichText::new(if channels == 1 { "M" } else { name }).monospace().color(theme::MUTED));
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width() - 62.0, 12.0), Sense::hover());
                super::meters::draw_horizontal(ui.painter(), rect, &app.meters[i]);
                let db = app.meters[i].level_db;
                let text = if db <= crate::audio::FLOOR_DB + 1.0 { "-inf".to_owned() } else { format!("{db:.0} dB") };
                ui.label(RichText::new(text).monospace().size(11.5));
            });
        }
        let (scale, _) = ui.allocate_exact_size(vec2(ui.available_width() - 62.0, 14.0), Sense::hover());
        super::meters::draw_scale(ui.painter(), egui::Rect::from_min_size(scale.min + vec2(18.0, -2.0), vec2(scale.width() - 18.0, 0.0)));
        ui.horizontal(|ui| {
            let (led, resp) = ui.allocate_exact_size(vec2(60.0, 22.0), Sense::click());
            let color = if app.audio_clip { theme::DANGER } else { theme::FAINT };
            ui.painter().rect_filled(led, egui::CornerRadius::same(6), color.gamma_multiply(if app.audio_clip { 0.9 } else { 0.25 }));
            ui.painter().text(led.center(), egui::Align2::CENTER_CENTER, "CLIP", egui::FontId::proportional(11.0), if app.audio_clip { egui::Color32::WHITE } else { theme::MUTED });
            if resp.on_hover_text("Click to clear").clicked() {
                app.audio_clip = false;
                app.audio.levels.clear_clip();
            }
            let mut listen = app.cfg.audio_listen;
            if ui.checkbox(&mut listen, "Listen").on_hover_text("Play the input through your speakers or headphones").changed() {
                app.set_listen(listen);
            }
        });
        if app.cfg.audio_listen {
            ui.horizontal(|ui| {
                widgets::field_label(ui, "Volume");
                let mut v = app.cfg.audio_volume;
                if ui.add(egui::Slider::new(&mut v, 0.0..=1.5).show_value(false)).changed() {
                    app.cfg.audio_volume = v;
                    app.audio.send(crate::audio::Command::Volume(v));
                }
            });
            ui.label(RichText::new("Use headphones to avoid feedback when the microphone is near the speakers.").size(11.5).color(theme::WARN));
        }
    });
}

fn output_card(app: &mut LumenApp, ui: &mut Ui) {
    widgets::card(ui, "Output", |ui| {
        widgets::field_label(ui, "Folder");
        let mut dir = app.cfg.output_dir.display().to_string();
        ui.horizontal(|ui| {
            let browse = widgets::icon_button(ui, Icon::Folder, Kind::Secondary).on_hover_text("Choose folder").clicked();
            let w = ui.available_width();
            if ui.add_sized([w, 34.0], egui::TextEdit::singleline(&mut dir).margin(egui::Margin::symmetric(10, 8))).changed() {
                app.cfg.output_dir = dir.clone().into();
            }
            if browse && let Some(p) = rfd::FileDialog::new().set_directory(&app.cfg.output_dir).pick_folder() {
                app.cfg.output_dir = p;
            }
        });
        widgets::field_label(ui, "File prefix");
        let w = ui.available_width();
        ui.add_sized([w, 34.0], egui::TextEdit::singleline(&mut app.cfg.prefix).margin(egui::Margin::symmetric(10, 8)));
        ui.horizontal(|ui| {
            widgets::field_label(ui, "Peaking sensitivity");
        });
        let before = app.cfg.peaking_threshold;
        ui.add(egui::Slider::new(&mut app.cfg.peaking_threshold, 15.0..=200.0).show_value(false));
        if (before - app.cfg.peaking_threshold).abs() > f32::EPSILON {
            app.push_peaking();
        }
    });
}
