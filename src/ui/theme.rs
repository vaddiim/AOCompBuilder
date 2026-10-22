//! Look of the app: palette, fonts, egui style, and the colour/frame helpers the screens share.
//!
//! Dark warm greys with gold and copper accents, after the game's own interface.

use std::path::Path;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke,
    TextStyle, Visuals,
};

pub(super) const SLOT_ICON: f32 = 56.0;
pub(super) const PICK_ICON: f32 = 48.0;
pub(super) const SPELL_ICON: f32 = 44.0;

// Surfaces, darkest to lightest.
pub(super) const BG_INPUT: Color32 = Color32::from_rgb(13, 12, 11);
pub(super) const BG: Color32 = Color32::from_rgb(19, 17, 15);
pub(super) const PANEL: Color32 = Color32::from_rgb(26, 23, 20);
pub(super) const CARD: Color32 = Color32::from_rgb(35, 31, 27);
pub(super) const CARD_HOVER: Color32 = Color32::from_rgb(48, 42, 35);
pub(super) const BORDER: Color32 = Color32::from_rgb(70, 60, 48);

// Accents.
pub(super) const GOLD: Color32 = Color32::from_rgb(232, 184, 86);
pub(super) const COPPER: Color32 = Color32::from_rgb(184, 115, 51);
pub(super) const HIGHLIGHT: Color32 = Color32::from_rgb(240, 180, 60);
pub(super) const DANGER: Color32 = Color32::from_rgb(222, 92, 78);

// Text.
pub(super) const TEXT: Color32 = Color32::from_rgb(235, 227, 212);
pub(super) const MUTED: Color32 = Color32::from_rgb(158, 147, 131);
/// Dark text for use on gold/copper fills.
pub(super) const ON_ACCENT: Color32 = Color32::from_rgb(30, 22, 12);

pub(super) const CARD_RADIUS: u8 = 8;
pub(super) const BUTTON_RADIUS: u8 = 6;

/// Installs fonts and the egui style. Call once at startup.
pub fn install(ctx: &egui::Context, assets: &Path) {
    install_fonts(ctx, assets);
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(style);
}

/// Prefers `assets/fonts/UI-Regular.ttf` (drop any TTF there to change the look), then Segoe UI on
/// Windows; without either the egui default font stays. The defaults remain as fallback for symbols.
fn install_fonts(ctx: &egui::Context, assets: &Path) {
    let candidates = [
        assets.join("fonts").join("UI-Regular.ttf"),
        Path::new(r"C:\Windows\Fonts\segoeui.ttf").to_path_buf(),
    ];
    let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) else {
        return;
    };

    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("ui".to_owned(), Arc::new(FontData::from_owned(bytes)));
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "ui".to_owned());
    ctx.set_fonts(fonts);
}

fn style(style: &mut egui::Style) {
    style.visuals = visuals();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 28.0;
    style.spacing.window_margin = Margin::same(14);
    style.spacing.menu_margin = Margin::same(8);

    let sizes = [
        (TextStyle::Small, 12.0),
        (TextStyle::Body, 15.0),
        (TextStyle::Button, 15.0),
        (TextStyle::Heading, 22.0),
        (TextStyle::Monospace, 13.5),
    ];
    for (text_style, size) in sizes {
        style
            .text_styles
            .insert(text_style, FontId::new(size, FontFamily::Proportional));
    }
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(13.5, FontFamily::Monospace),
    );
}

fn visuals() -> Visuals {
    let mut v = Visuals::dark();
    let button = CornerRadius::same(BUTTON_RADIUS);

    v.override_text_color = Some(TEXT);
    v.panel_fill = PANEL;
    v.window_fill = CARD;
    v.extreme_bg_color = BG_INPUT;
    v.faint_bg_color = Color32::from_rgb(30, 27, 23);
    v.code_bg_color = BG_INPUT;
    v.hyperlink_color = GOLD;
    v.warn_fg_color = GOLD;
    v.error_fg_color = DANGER;

    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(140),
    };
    v.window_shadow = v.popup_shadow;

    v.selection.bg_fill = COPPER.gamma_multiply(0.55);
    v.selection.stroke = Stroke::new(1.0, GOLD);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER.gamma_multiply(0.7));
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = button;

    w.inactive.bg_fill = CARD;
    w.inactive.weak_bg_fill = CARD;
    w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = button;

    w.hovered.bg_fill = CARD_HOVER;
    w.hovered.weak_bg_fill = CARD_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, COPPER);
    w.hovered.fg_stroke = Stroke::new(1.5, GOLD);
    w.hovered.corner_radius = button;

    w.active.bg_fill = COPPER.gamma_multiply(0.6);
    w.active.weak_bg_fill = COPPER.gamma_multiply(0.6);
    w.active.bg_stroke = Stroke::new(1.0, GOLD);
    w.active.fg_stroke = Stroke::new(1.5, TEXT);
    w.active.corner_radius = button;

    w.open.bg_fill = CARD_HOVER;
    w.open.weak_bg_fill = CARD_HOVER;
    w.open.bg_stroke = Stroke::new(1.0, COPPER);
    w.open.corner_radius = button;
    v
}

/// Frame of a card: rounded, with a gold outline when `selected`.
pub(super) fn card_frame(selected: bool) -> egui::Frame {
    let stroke = if selected {
        Stroke::new(1.5, GOLD)
    } else {
        Stroke::new(1.0, BORDER)
    };
    egui::Frame::new()
        .fill(CARD)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::same(10))
}

/// In-game enchantment colours: .1 green, .2 blue, .3 purple, .4 gold; .0 has no badge.
pub(super) fn enchant_color(enchant: u8) -> Option<Color32> {
    match enchant {
        1 => Some(Color32::from_rgb(88, 168, 84)),
        2 => Some(Color32::from_rgb(70, 140, 214)),
        3 => Some(Color32::from_rgb(160, 96, 210)),
        4 => Some(Color32::from_rgb(222, 170, 52)),
        _ => None,
    }
}

/// Tier colours, from the plain greys of low tiers to the gold of T8.
pub(super) fn tier_color(tier: u8) -> Color32 {
    match tier {
        0..=2 => Color32::from_rgb(120, 112, 100),
        3 => Color32::from_rgb(100, 150, 96),
        4 => Color32::from_rgb(76, 130, 190),
        5 => Color32::from_rgb(196, 84, 70),
        6 => Color32::from_rgb(214, 130, 52),
        7 => Color32::from_rgb(222, 192, 70),
        _ => Color32::from_rgb(236, 236, 228),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_enchanted_items_get_an_enchant_colour() {
        assert!(enchant_color(0).is_none());
        let colours: Vec<_> = (1..=4).filter_map(enchant_color).collect();
        assert_eq!(colours.len(), 4);
        assert!(
            colours.windows(2).all(|w| w[0] != w[1]),
            "each level has its own colour"
        );
        assert!(enchant_color(9).is_none());
    }
}
