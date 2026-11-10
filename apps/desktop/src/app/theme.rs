//! Visual identity: palette and egui style.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};

pub const BG: Color32 = Color32::from_rgb(11, 13, 17);
pub const PANEL: Color32 = Color32::from_rgb(18, 21, 27);
pub const CARD: Color32 = Color32::from_rgb(25, 29, 37);
pub const CARD_HOVER: Color32 = Color32::from_rgb(32, 37, 47);
pub const STROKE: Color32 = Color32::from_rgb(40, 46, 58);
pub const TEXT: Color32 = Color32::from_rgb(232, 236, 242);
pub const MUTED: Color32 = Color32::from_rgb(133, 143, 160);
pub const FAINT: Color32 = Color32::from_rgb(84, 93, 109);
pub const ACCENT: Color32 = Color32::from_rgb(255, 176, 32);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(92, 66, 14);
pub const LIVE: Color32 = Color32::from_rgb(61, 214, 198);
pub const OK: Color32 = Color32::from_rgb(88, 214, 141);
pub const WARN: Color32 = Color32::from_rgb(245, 197, 66);
pub const DANGER: Color32 = Color32::from_rgb(255, 99, 99);

pub const RADIUS: u8 = 10;

pub fn radius() -> CornerRadius {
    CornerRadius::same(RADIUS)
}

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (TextStyle::Heading, FontId::new(20.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.5, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.spacing.interact_size.y = 30.0;
    style.spacing.slider_width = 150.0;
    style.spacing.scroll.bar_width = 6.0;
    style.spacing.scroll.floating = true;

    let mut v = Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = CARD;
    v.extreme_bg_color = Color32::from_rgb(8, 10, 13);
    v.faint_bg_color = CARD;
    v.window_stroke = Stroke::new(1.0, STROKE);
    v.window_corner_radius = radius();
    v.menu_corner_radius = CornerRadius::same(8);
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = LIVE;
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = Stroke::new(1.0, ACCENT);

    let widget = |bg: Color32, stroke: Color32| egui::style::WidgetVisuals {
        bg_fill: bg,
        weak_bg_fill: bg,
        bg_stroke: Stroke::new(1.0, stroke),
        corner_radius: CornerRadius::same(8),
        fg_stroke: Stroke::new(1.0, TEXT),
        expansion: 0.0,
    };
    v.widgets.noninteractive = widget(PANEL, STROKE);
    v.widgets.inactive = widget(CARD, STROKE);
    v.widgets.hovered = widget(CARD_HOVER, FAINT);
    v.widgets.active = widget(CARD_HOVER, ACCENT);
    v.widgets.open = widget(CARD_HOVER, STROKE);
    style.visuals = v;
    ctx.set_global_style(style);
}
