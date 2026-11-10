//! Audio level meters: ballistics and drawing.

use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Stroke, vec2};

use super::theme;

/// Scale shown by the meters, in dBFS.
const RANGE_DB: f32 = 60.0;
const FALL_DB_PER_S: f32 = 28.0;
const HOLD_SECONDS: f32 = 1.2;

#[derive(Clone, Copy)]
pub struct Meter {
    /// Smoothed level shown as the bar.
    pub level_db: f32,
    /// Held peak marker.
    pub hold_db: f32,
    hold_age: f32,
}

impl Default for Meter {
    fn default() -> Self {
        Self { level_db: crate::audio::FLOOR_DB, hold_db: crate::audio::FLOOR_DB, hold_age: 0.0 }
    }
}

impl Meter {
    /// Instant attack, steady release; the peak marker holds before falling.
    pub fn feed(&mut self, peak_db: f32, dt: f32) {
        self.level_db = peak_db.max(self.level_db - FALL_DB_PER_S * dt);
        if peak_db >= self.hold_db {
            self.hold_db = peak_db;
            self.hold_age = 0.0;
        } else {
            self.hold_age += dt;
            if self.hold_age > HOLD_SECONDS {
                self.hold_db = (self.hold_db - 2.0 * FALL_DB_PER_S * dt).max(self.level_db);
            }
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// 0..=1 position of a level on the meter.
pub fn fraction(db: f32) -> f32 {
    ((db + RANGE_DB) / RANGE_DB).clamp(0.0, 1.0)
}

fn zone(db: f32) -> Color32 {
    if db > -6.0 {
        theme::DANGER
    } else if db > -18.0 {
        theme::WARN
    } else {
        theme::OK
    }
}

/// Segmented bar: each lit segment takes the colour of its own level zone.
fn segments(painter: &Painter, rect: Rect, meter: &Meter, vertical: bool) {
    const N: usize = 30;
    let lit = (fraction(meter.level_db) * N as f32).round() as usize;
    let hold = ((fraction(meter.hold_db) * N as f32).round() as usize).clamp(1, N) - 1;
    for i in 0..N {
        let t0 = i as f32 / N as f32;
        let t1 = (i as f32 + 0.82) / N as f32;
        let r = if vertical {
            Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - rect.height() * t1), Pos2::new(rect.right(), rect.bottom() - rect.height() * t0))
        } else {
            Rect::from_min_max(Pos2::new(rect.left() + rect.width() * t0, rect.top()), Pos2::new(rect.left() + rect.width() * t1, rect.bottom()))
        };
        let db = t0 * RANGE_DB - RANGE_DB;
        let on = i < lit || (i == hold && meter.hold_db > crate::audio::FLOOR_DB + 1.0);
        let color = if on { zone(db) } else { Color32::from_white_alpha(14) };
        painter.rect_filled(r, CornerRadius::same(1), color);
    }
}

pub fn draw_horizontal(painter: &Painter, rect: Rect, meter: &Meter) {
    segments(painter, rect, meter, false);
}

pub fn draw_vertical(painter: &Painter, rect: Rect, meter: &Meter) {
    segments(painter, rect, meter, true);
}

/// Tick labels under a horizontal meter (-60 .. 0 dB).
pub fn draw_scale(painter: &Painter, rect: Rect) {
    for db in [-60.0, -40.0, -20.0, -10.0, 0.0] {
        let x = rect.left() + rect.width() * fraction(db);
        painter.line_segment([Pos2::new(x, rect.bottom() + 1.0), Pos2::new(x, rect.bottom() + 4.0)], Stroke::new(1.0, theme::FAINT));
        painter.text(Pos2::new(x, rect.bottom() + 6.0) + vec2(0.0, 0.0), egui::Align2::CENTER_TOP, format!("{db:.0}"), egui::FontId::proportional(9.5), theme::FAINT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_attacks_instantly_and_releases_slowly() {
        let mut m = Meter::default();
        m.feed(-6.0, 0.016);
        assert_eq!(m.level_db, -6.0);
        m.feed(crate::audio::FLOOR_DB, 0.1);
        assert!((m.level_db - (-6.0 - FALL_DB_PER_S * 0.1)).abs() < 1e-3);
        assert_eq!(m.hold_db, -6.0, "peak marker holds");
        for _ in 0..200 {
            m.feed(crate::audio::FLOOR_DB, 0.016);
        }
        assert!(m.hold_db < -40.0, "peak marker eventually falls");
    }

    #[test]
    fn scale_maps_db_to_fraction() {
        assert_eq!(fraction(0.0), 1.0);
        assert_eq!(fraction(-60.0), 0.0);
        assert!((fraction(-30.0) - 0.5).abs() < 1e-6);
    }
}
