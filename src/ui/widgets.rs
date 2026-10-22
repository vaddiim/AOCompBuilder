//! Reusable pieces: icons, tooltips, spell rows, swap strips, the item picker and the icon buttons.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Margin, Rect, RichText, Sense, Stroke, Vec2,
};

use crate::build::{Build, Equipped};
use crate::catalog::{Catalog, Item, Slot, Spell, category_label};

use super::theme::{
    BORDER, CARD, CARD_HOVER, CARD_RADIUS, COPPER, DANGER, GOLD, HIGHLIGHT, MUTED, ON_ACCENT,
    PICK_ICON, SPELL_ICON, TEXT, enchant_color, tier_color,
};

#[derive(Default)]
pub(super) struct Filter {
    /// Global search; while it is not empty the category row is bypassed.
    pub(super) search: String,
    /// Last category chosen for each slot, so switching slots and back keeps the browsing place.
    pub(super) category: HashMap<Slot, String>,
}

/// Small icons of an equipped item's swaps, each with a tooltip naming the swap and its note.
pub(super) fn swap_icons(
    ui: &mut egui::Ui,
    catalog: &Catalog,
    icons: &Path,
    equipped: &Equipped,
    size: f32,
) {
    for swap in &equipped.swaps {
        let resp = ui.add(
            item_icon(catalog, icons, &swap.item, swap.enchant)
                .fit_to_exact_size(Vec2::splat(size)),
        );
        resp.on_hover_ui(|ui| {
            ui.weak("Swap");
            match catalog.item(&swap.item) {
                Some(item) => item_tooltip(ui, item, swap.enchant),
                None => {
                    ui.label(&swap.item);
                }
            }
            if !swap.note.trim().is_empty() {
                ui.separator();
                ui.label(&swap.note);
            }
        });
    }
}

/// Icons of every spell and passive the build picked, each with its tooltip.
pub(super) fn spell_strip(
    ui: &mut egui::Ui,
    catalog: &Catalog,
    icons: &Path,
    build: &Build,
    size: f32,
) {
    ui.horizontal_wrapped(|ui| {
        let chosen = build
            .slots
            .values()
            .flat_map(|e| e.spells.iter().chain(&e.passives))
            .flatten();
        for id in chosen {
            if let Some(icon) = spell_icon(catalog, icons, id) {
                ui.add(icon.fit_to_exact_size(Vec2::splat(size)))
                    .on_hover_ui(|ui| spell_tooltip(ui, catalog, id));
            }
        }
    });
}

/// Icon of an item by its catalog id; the renderer may know it under another name (`Item.icon`).
pub(super) fn item_icon(
    catalog: &Catalog,
    icons: &Path,
    id: &str,
    enchant: u8,
) -> egui::Image<'static> {
    icon_image(
        icons,
        catalog.item(id).map_or(id, |i| i.icon.as_str()),
        enchant,
    )
}

pub(super) fn icon_image(icons: &Path, stem: &str, enchant: u8) -> egui::Image<'static> {
    let stem = if enchant > 0 {
        format!("{stem}_e{enchant}")
    } else {
        stem.to_owned()
    };
    file_image(icons.join("items").join(format!("{stem}.png")))
}

pub(super) fn spell_icon(
    catalog: &Catalog,
    icons: &Path,
    id: &str,
) -> Option<egui::Image<'static>> {
    let spell = catalog.spell(id)?;
    Some(file_image(
        icons.join("spells").join(format!("{}.png", spell.icon)),
    ))
}

pub(super) fn item_tooltip(ui: &mut egui::Ui, item: &Item, enchant: u8) {
    ui.set_max_width(300.0);
    ui.label(RichText::new(item_name(item)).strong().size(16.0));
    ui.horizontal(|ui| {
        tier_badge(ui, item.tier);
        enchant_badge(ui, enchant);
    });
    if !item.category.is_empty() {
        ui.label(RichText::new(category_label(item.slot, &item.category)).color(MUTED));
    }
}

pub(super) fn spell_name(spell: &Spell) -> &str {
    if spell.name_fr.is_empty() {
        &spell.name
    } else {
        &spell.name_fr
    }
}

pub(super) fn spell_tooltip(ui: &mut egui::Ui, catalog: &Catalog, id: &str) {
    let Some(spell) = catalog.spell(id) else {
        ui.label(RichText::new(id).strong());
        return;
    };
    ui.set_max_width(320.0);
    ui.label(RichText::new(spell_name(spell)).strong());

    let category = match spell.category.as_str() {
        "damage" => "Dégâts",
        "crowdcontrol" => "Contrôle",
        "heal" => "Soin",
        "buff" => "Amélioration",
        "mobility" => "Mobilité",
        _ => "",
    };
    let stats: Vec<String> = [
        (spell.cooldown, "Recharge", "s"),
        (spell.energy, "Énergie", ""),
        (spell.range, "Portée", " m"),
        (spell.cast_time, "Incantation", "s"),
    ]
    .into_iter()
    .filter(|(v, ..)| *v > 0.0)
    .map(|(v, label, unit)| format!("{label} : {v}{unit}"))
    .collect();
    let kind = if spell.passive { "Passif" } else { category };
    if !kind.is_empty() {
        ui.weak(kind);
    }
    if !stats.is_empty() {
        ui.label(stats.join("  ·  "));
    }
    let description = if spell.description_fr.is_empty() {
        &spell.description
    } else {
        &spell.description_fr
    };
    if !description.is_empty() {
        ui.separator();
        ui.label(description);
    }
}

pub(super) fn enchant_row(ui: &mut egui::Ui, slot: Slot, enchant: &mut u8) {
    if !slot.enchantable() {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Enchantement").color(MUTED));
        for n in 0..=4u8 {
            let color = enchant_color(n).unwrap_or(MUTED);
            let selected = *enchant == n;
            let text = RichText::new(format!(".{n}")).strong().color(if selected {
                contrast_text(color)
            } else {
                color
            });
            let button = egui::Button::new(text)
                .corner_radius(CornerRadius::same(10))
                .min_size(egui::vec2(34.0, 22.0));
            if ui
                .add(if selected { button.fill(color) } else { button })
                .clicked()
            {
                *enchant = n;
            }
        }
    });
}

/// Item chooser shared by every slot: a row of categories (icon of a sample item + label), then the
/// items of the chosen category. A non-empty search looks through all categories of the slot.
/// Returns the item clicked this frame.
pub(super) fn pick_item<'a>(
    ui: &mut egui::Ui,
    catalog: &'a Catalog,
    icons: &Path,
    slot: Slot,
    filter: &mut Filter,
    current: Option<&str>,
) -> Option<&'a Item> {
    let usable = |i: &Item| i.slot == slot;
    let keys = catalog.categories(slot);

    let selected = filter
        .category
        .get(&slot)
        .filter(|c| keys.contains(&c.as_str()))
        .cloned()
        .or_else(|| keys.first().map(|k| (*k).to_owned()))
        .unwrap_or_default();
    let needle = filter.search.trim().to_lowercase();
    let searching = !needle.is_empty();

    if keys.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            for key in &keys {
                let label = category_label(slot, key);
                let stroke = if !searching && selected == *key {
                    egui::Stroke::new(2.0, HIGHLIGHT)
                } else {
                    egui::Stroke::NONE
                };
                let sample = catalog
                    .items
                    .iter()
                    .find(|i| usable(i) && i.category == *key);
                let button = match sample {
                    Some(item) => egui::Button::image_and_text(
                        icon_image(icons, &item.icon, 0).fit_to_exact_size(Vec2::splat(28.0)),
                        label,
                    ),
                    None => egui::Button::new(label),
                };
                if ui.add(button.stroke(stroke)).clicked() {
                    filter.category.insert(slot, (*key).to_owned());
                    filter.search.clear();
                }
            }
        });
    }
    ui.horizontal(|ui| {
        ui.label("Recherche");
        ui.add(
            egui::TextEdit::singleline(&mut filter.search).hint_text("dans toutes les catégories"),
        );
    });
    ui.add_space(4.0);

    let candidates: Vec<&Item> = catalog
        .items
        .iter()
        .filter(|i| {
            usable(i)
                && if searching {
                    item_matches(i, &needle)
                } else {
                    i.category == selected
                }
        })
        .collect();
    if candidates.is_empty() {
        ui.weak("Aucun objet ne correspond à la recherche.");
        return None;
    }

    // Fixed-size grid so only the visible rows are laid out, however large the list is.
    let spacing = ui.spacing().item_spacing;
    let cell = PICK_ICON + 8.0;
    let width = ui.available_width() - ui.spacing().scroll.allocated_width();
    let columns = (((width + spacing.x) / (cell + spacing.x)) as usize).max(1);
    let rows = candidates.len().div_ceil(columns);
    let mut chosen = None;

    egui::ScrollArea::vertical()
        .id_salt("picker")
        .auto_shrink(false)
        .show_rows(ui, cell, rows, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    for item in candidates.iter().skip(row * columns).take(columns) {
                        let icon = Some(icon_image(icons, &item.icon, 0));
                        let badges = Badges {
                            tier: tier_to_show(item),
                            ..Badges::default()
                        };
                        let resp = slot_card(
                            ui,
                            icon,
                            cell,
                            current == Some(item.id.as_str()),
                            true,
                            "",
                            badges,
                        );
                        if resp.on_hover_ui(|ui| item_tooltip(ui, item, 0)).clicked() {
                            chosen = Some(*item);
                        }
                    }
                });
            }
        });
    chosen
}

/// One row of spell icons; `needle` (lowercase) hides non-matching spells except the chosen one.
pub(super) fn spell_row(
    ui: &mut egui::Ui,
    catalog: &Catalog,
    icons: &Path,
    label: &str,
    choices: &[&str],
    chosen: Option<&str>,
    needle: &str,
) -> Option<String> {
    let mut picked = None;
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.add_sized(
            [48.0, SPELL_ICON],
            egui::Label::new(RichText::new(label).strong()),
        );
        for &id in choices {
            let is_chosen = chosen == Some(id);
            let name = catalog.spell(id).map_or(id, spell_name);
            if !is_chosen && !needle.is_empty() && !name.to_lowercase().contains(needle) {
                continue;
            }
            let resp = slot_button(
                ui,
                spell_icon(catalog, icons, id),
                is_chosen,
                SPELL_ICON,
                true,
            );
            if resp
                .on_hover_ui(|ui| spell_tooltip(ui, catalog, id))
                .clicked()
            {
                picked = Some(id.to_owned());
            }
        }
    });
    picked
}

/// `file://` URIs need forward slashes, and on Windows a third one before the drive letter
/// (`file:///C:/…`); without it egui reads `C:` as a UNC host name and the load fails.
pub(super) fn file_image(path: PathBuf) -> egui::Image<'static> {
    let path = path.to_string_lossy().replace('\\', "/");
    let uri = if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    };
    egui::Image::new(uri)
}

/// Square icon button with a highlighted frame when selected; an empty slot shows a bare frame.
pub(super) fn slot_button(
    ui: &mut egui::Ui,
    icon: Option<egui::Image<'static>>,
    selected: bool,
    size: f32,
    enabled: bool,
) -> egui::Response {
    let stroke = if selected {
        egui::Stroke::new(2.0, HIGHLIGHT)
    } else {
        egui::Stroke::NONE
    };
    let button = match icon {
        Some(img) => egui::Button::image(img.fit_to_exact_size(Vec2::splat(size))),
        None => egui::Button::new(""),
    };
    ui.add_enabled(
        enabled,
        button.min_size(Vec2::splat(size + 8.0)).stroke(stroke),
    )
}

// ---- styled building blocks ----------------------------------------------------------------

/// Name shown for an item: the French one (the catalog falls back to English when the game has
/// none). Every place that displays or searches an item name goes through here and `item_matches`.
pub(super) fn item_name(item: &Item) -> &str {
    if item.name_fr.is_empty() {
        &item.name
    } else {
        &item.name_fr
    }
}

/// Whether `needle` (already lowercase) matches the item by its French or English name, so both
/// "épée" and "sword" find a sword.
pub(super) fn item_matches(item: &Item, needle: &str) -> bool {
    item_name(item).to_lowercase().contains(needle) || item.name.to_lowercase().contains(needle)
}

/// The tier worth printing on a card: food and potions come at the best tier of their family
/// (T6, T7…), so it varies; equipment is all T8 and needs no badge.
pub(super) fn tier_to_show(item: &Item) -> Option<u8> {
    (matches!(item.slot, Slot::Food | Slot::Potion) || item.tier != crate::catalog::TIER)
        .then_some(item.tier)
}

/// Black or white text, whichever reads better on `fill`.
pub(super) fn contrast_text(fill: Color32) -> Color32 {
    let luma =
        0.299 * f32::from(fill.r()) + 0.587 * f32::from(fill.g()) + 0.114 * f32::from(fill.b());
    if luma > 140.0 {
        ON_ACCENT
    } else {
        Color32::WHITE
    }
}

/// Small rounded label, e.g. a tier or an enchantment level.
pub(super) fn badge(ui: &mut egui::Ui, text: &str, fill: Color32) {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.label(
                RichText::new(text)
                    .small()
                    .strong()
                    .color(contrast_text(fill)),
            );
        });
}

pub(super) fn tier_badge(ui: &mut egui::Ui, tier: u8) {
    badge(ui, &format!("T{tier}"), tier_color(tier));
}

pub(super) fn enchant_badge(ui: &mut egui::Ui, enchant: u8) {
    if let Some(color) = enchant_color(enchant) {
        badge(ui, &format!(".{enchant}"), color);
    }
}

fn corner_badge(
    painter: &egui::Painter,
    anchor: egui::Pos2,
    align: Align2,
    text: &str,
    fill: Color32,
) {
    let pad = egui::vec2(4.0, 1.0);
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        FontId::proportional(11.0),
        contrast_text(fill),
    );
    let rect = align.anchor_size(anchor, galley.size() + pad * 2.0);
    painter.rect_filled(rect, CornerRadius::same(4), fill);
    painter.galley(rect.min + pad, galley, contrast_text(fill));
}

/// What a slot card says about its item besides the icon.
#[derive(Default, Clone, Copy)]
pub(super) struct Badges {
    pub(super) enchant: u8,
    /// Printed top-left when worth showing (see `tier_to_show`).
    pub(super) tier: Option<u8>,
    /// Number of swaps, printed top-right.
    pub(super) swaps: usize,
}

#[allow(clippy::too_many_arguments)]
fn paint_slot_card(
    ui: &egui::Ui,
    rect: Rect,
    hovered: bool,
    selected: bool,
    enabled: bool,
    icon: Option<egui::Image<'static>>,
    empty_label: &str,
    badges: Badges,
) {
    let radius = CornerRadius::same(10);
    let stroke = if selected {
        Stroke::new(2.0, GOLD)
    } else if hovered {
        Stroke::new(1.0, COPPER)
    } else {
        Stroke::new(1.0, BORDER)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, radius, if hovered { CARD_HOVER } else { CARD });
    match icon {
        Some(image) => {
            let image = if enabled {
                image
            } else {
                image.tint(Color32::from_white_alpha(70))
            };
            image.paint_at(ui, rect.shrink(rect.width() * 0.08));
        }
        None if !empty_label.is_empty() => {
            let color = if enabled {
                MUTED
            } else {
                MUTED.gamma_multiply(0.5)
            };
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                empty_label,
                FontId::proportional(12.0),
                color,
            );
        }
        None => {}
    }
    painter.rect_stroke(rect, radius, stroke, egui::StrokeKind::Inside);
    let inset = egui::vec2(3.0, 3.0);
    if let Some(color) = enchant_color(badges.enchant) {
        corner_badge(
            painter,
            rect.right_bottom() - inset,
            Align2::RIGHT_BOTTOM,
            &format!(".{}", badges.enchant),
            color,
        );
    }
    if let Some(tier) = badges.tier {
        corner_badge(
            painter,
            rect.left_top() + inset,
            Align2::LEFT_TOP,
            &format!("T{tier}"),
            tier_color(tier),
        );
    }
    if badges.swaps > 0 {
        let anchor = egui::pos2(rect.right() - inset.x, rect.top() + inset.y);
        corner_badge(
            painter,
            anchor,
            Align2::RIGHT_TOP,
            &format!("⇄{}", badges.swaps),
            COPPER,
        );
    }
}

/// Square equipment slot: icon on a rounded card with its badges. An empty slot shows
/// `empty_label`; a disabled one is dimmed and not clickable.
pub(super) fn slot_card(
    ui: &mut egui::Ui,
    icon: Option<egui::Image<'static>>,
    size: f32,
    selected: bool,
    enabled: bool,
    empty_label: &str,
    badges: Badges,
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), sense);
    if ui.is_rect_visible(rect) {
        paint_slot_card(
            ui,
            rect,
            enabled && response.hovered(),
            selected,
            enabled,
            icon,
            empty_label,
            badges,
        );
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// Same card for display only (no click, no hover highlight).
pub(super) fn static_slot(
    ui: &mut egui::Ui,
    icon: Option<egui::Image<'static>>,
    size: f32,
    badges: Badges,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_slot_card(ui, rect, false, false, true, icon, "", badges);
    }
    response
}

/// A build laid out like the in-game equipment screen, with a tooltip per item. Empty slots stay
/// as faint cards so every build has the same silhouette.
pub(super) fn mannequin_preview(
    ui: &mut egui::Ui,
    catalog: &Catalog,
    icons: &Path,
    build: &Build,
    cell: f32,
) {
    ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
    for row in super::editor::MANNEQUIN {
        ui.horizontal(|ui| {
            for slot in row {
                let Some(slot) = slot else {
                    ui.add_space(cell);
                    continue;
                };
                let equipped = build.get(slot);
                let item = equipped.and_then(|e| catalog.item(&e.item));
                let badges = Badges {
                    enchant: equipped.map_or(0, |e| e.enchant),
                    tier: item.and_then(tier_to_show),
                    swaps: equipped.map_or(0, |e| e.swaps.len()),
                };
                let icon = equipped.map(|e| item_icon(catalog, icons, &e.item, e.enchant));
                let resp = static_slot(ui, icon, cell, badges);
                match (equipped, item) {
                    (Some(e), Some(item)) => {
                        resp.on_hover_ui(|ui| item_tooltip(ui, item, e.enchant));
                    }
                    (Some(e), None) => {
                        resp.on_hover_text(&e.item);
                    }
                    (None, _) => {}
                }
            }
        });
    }
}

/// Top-bar tab: muted text that turns gold with an underline when selected.
pub(super) fn nav_tab(ui: &mut egui::Ui, selected: bool, text: &str) -> egui::Response {
    let font = FontId::proportional(16.0);
    let pad = egui::vec2(14.0, 10.0);
    let size = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), TEXT)
        .size()
        + pad * 2.0;
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let color = if selected {
        GOLD
    } else if response.hovered() {
        TEXT
    } else {
        MUTED
    };
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(text.to_owned(), font, color);
    painter.galley(rect.min + pad, galley, color);
    if selected {
        let underline = Rect::from_min_max(
            egui::pos2(rect.left() + 8.0, rect.bottom() - 3.0),
            egui::pos2(rect.right() - 8.0, rect.bottom() - 1.0),
        );
        painter.rect_filled(underline, CornerRadius::same(1), GOLD);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Copper outline over a hovered card (see the clickable cards of the lists).
pub(super) fn hover_outline(ui: &egui::Ui, response: &egui::Response) {
    ui.painter().rect_stroke(
        response.rect,
        CornerRadius::same(CARD_RADIUS),
        Stroke::new(1.5, COPPER),
        egui::StrokeKind::Inside,
    );
}

/// Button for destructive confirmations (red fill).
pub(super) fn danger_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).strong().color(Color32::WHITE))
        .fill(DANGER)
        .stroke(Stroke::NONE)
}

/// Main call-to-action button (gold).
pub(super) fn primary_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).strong().color(ON_ACCENT))
        .fill(GOLD)
        .stroke(Stroke::NONE)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn catalog() -> Catalog {
        Catalog::load(Path::new("assets/data")).expect("catalog")
    }

    #[test]
    fn consumables_always_show_their_tier_and_equipment_only_when_unusual() {
        let c = catalog();
        for item in c
            .items
            .iter()
            .filter(|i| matches!(i.slot, Slot::Food | Slot::Potion))
        {
            assert_eq!(tier_to_show(item), Some(item.tier), "{}", item.id);
        }
        for item in c.items.iter().filter(|i| i.slot == Slot::MainHand) {
            assert_eq!(tier_to_show(item), None, "{} is T8 like the rest", item.id);
        }
    }

    #[test]
    fn badge_text_contrasts_with_its_fill() {
        assert_eq!(contrast_text(Color32::from_rgb(236, 236, 228)), ON_ACCENT);
        assert_eq!(contrast_text(Color32::from_rgb(40, 40, 40)), Color32::WHITE);
    }

    #[test]
    fn items_show_their_french_name_and_are_found_in_both_languages() {
        let c = catalog();
        let sword = c.item("T8_MAIN_SWORD").expect("T8_MAIN_SWORD");
        assert_eq!(item_name(sword), sword.name_fr);
        assert!(item_matches(sword, &sword.name_fr.to_lowercase()));
        assert!(
            item_matches(sword, &sword.name.to_lowercase()),
            "the English name still matches"
        );
        assert!(!item_matches(sword, "zzzz"));
    }
}
