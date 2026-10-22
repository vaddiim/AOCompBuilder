//! The build editor: slot column, details of the selected slot, item picker, and the pure rules
//! for two-handed weapons and swaps.

use std::path::Path;

use eframe::egui::{self, Margin, RichText, Vec2};

use crate::build::{Build, Swap};
use crate::catalog::{Catalog, Item, Slot};
use crate::export_image::Job;

use super::theme::{BG, HIGHLIGHT, MUTED, PANEL, SLOT_ICON};
use super::widgets::{
    Badges, Filter, enchant_row, item_icon, item_name, item_tooltip, pick_item, primary_button,
    slot_card, spell_row, swap_icons, tier_badge, tier_to_show,
};
use super::{App, Dialog, Pending, snapshot};

/// Slots as the game's equipment screen shows them; `None` leaves a gap.
pub(super) const MANNEQUIN: [[Option<Slot>; 3]; 4] = [
    [Some(Slot::Bag), Some(Slot::Head), Some(Slot::Cape)],
    [Some(Slot::MainHand), Some(Slot::Armor), Some(Slot::OffHand)],
    [Some(Slot::Food), Some(Slot::Shoes), Some(Slot::Potion)],
    [None, Some(Slot::Mount), None],
];

impl App {
    pub(super) fn save_build(&mut self) {
        if self.build.name.trim().is_empty() {
            self.status = "Donne un nom au build avant de l'enregistrer.".into();
            return;
        }
        self.build.name = self.build.name.trim().to_owned();
        let replaced = self.store.upsert(self.build.clone());
        self.status = match self.store.save() {
            Ok(()) => {
                self.saved = snapshot(&self.build);
                if replaced {
                    format!("Build « {} » mis à jour.", self.build.name)
                } else {
                    format!("Build « {} » enregistré dans Mes Builds.", self.build.name)
                }
            }
            Err(e) => format!("Échec de l'enregistrement : {e}"),
        };
    }

    pub(super) fn editor(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("slots")
            .resizable(true)
            .default_size(330.0)
            .size_range(280.0..=480.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::same(12)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("left_column")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.build_header(ui);
                        ui.add_space(10.0);
                        self.mannequin(ui);
                    });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::same(14)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("details")
                    .max_height(ui.available_height() * 0.45)
                    .auto_shrink([false, true])
                    .show(ui, |ui| self.details(ui));
                ui.separator();
                self.picker(ui);
            });
    }

    /// Name, actions, unsaved-changes marker, note and messages above the mannequin.
    fn build_header(&mut self, ui: &mut egui::Ui) {
        ui.add(
            egui::TextEdit::singleline(&mut self.build.name)
                .hint_text("Nom du build")
                .font(egui::TextStyle::Heading)
                .desired_width(f32::INFINITY),
        );
        let dirty = self.dirty();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(primary_button("Enregistrer"))
                .on_hover_text("Ctrl+S")
                .clicked()
            {
                self.save_build();
            }
            if ui.button("Nouveau").clicked() {
                self.request_replace(Pending::New);
            }
            if ui.button("Exporter").clicked() {
                self.open_export(self.build.clone());
            }
            if ui
                .button("Image")
                .on_hover_text("Exporter le build en image PNG")
                .clicked()
            {
                let job = Job::build(&self.build, &self.catalog, &self.export_paths());
                self.start_export(ui.ctx(), job);
            }
            if ui.button("Importer").clicked() {
                self.dialog = Dialog::Import {
                    text: String::new(),
                    path: String::new(),
                    error: String::new(),
                };
            }
        });
        if dirty {
            ui.label(
                RichText::new("● Modifications non enregistrées")
                    .small()
                    .color(HIGHLIGHT),
            );
        }
        let note_title = if self.build.note.trim().is_empty() {
            "Note du build"
        } else {
            "Note du build ●"
        };
        egui::CollapsingHeader::new(note_title)
            .id_salt("build_note")
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.build.note)
                        .hint_text("Rôle, rotation, consignes…")
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                );
            });
        if !self.status.is_empty() {
            ui.label(RichText::new(&self.status).small().color(MUTED));
        }
        self.report_ui(ui);
    }

    /// Equipment screen laid out like the game's: three columns, mount under the boots.
    fn mannequin(&mut self, ui: &mut egui::Ui) {
        let two_handed = is_two_handed(&self.build, &self.catalog);
        let swap_conflict = !two_handed_swap_conflicts(&self.build, &self.catalog).is_empty();

        let gap = ui.spacing().item_spacing.x;
        let available = ui.available_width();
        let cell = ((available - 2.0 * gap) / 3.0).clamp(60.0, 96.0);
        let left_pad = ((available - (3.0 * cell + 2.0 * gap)) / 2.0).max(0.0);

        for row in MANNEQUIN {
            ui.horizontal_top(|ui| {
                ui.add_space(left_pad);
                for slot in row {
                    match slot {
                        Some(slot) => self.slot_cell(ui, slot, cell, two_handed, swap_conflict),
                        None => {
                            ui.add_space(cell);
                        }
                    }
                }
            });
            ui.add_space(4.0);
        }
    }

    /// One slot of the mannequin: card, then the item's name and the icons of its swaps.
    fn slot_cell(
        &mut self,
        ui: &mut egui::Ui,
        slot: Slot,
        cell: f32,
        two_handed: bool,
        swap_conflict: bool,
    ) {
        let blocked = slot == Slot::OffHand && two_handed;
        ui.vertical(|ui| {
            ui.set_width(cell);
            let equipped = self.build.get(slot);
            let item = equipped.and_then(|e| self.catalog.item(&e.item));
            let icon = equipped.map(|e| item_icon(&self.catalog, &self.icons, &e.item, e.enchant));
            let enchant = equipped.map_or(0, |e| e.enchant);
            let tier = item.and_then(tier_to_show);

            let badges = Badges {
                enchant,
                tier,
                swaps: 0,
            };
            let mut resp = slot_card(
                ui,
                icon,
                cell,
                self.selected == slot,
                !blocked,
                slot.label(),
                badges,
            );
            if let Some(item) = item {
                resp = resp.on_hover_ui(|ui| item_tooltip(ui, item, enchant));
            } else if blocked {
                resp = resp.on_hover_text("Indisponible : l'arme équipée se tient à deux mains");
            }
            if resp.clicked() {
                self.selected = slot;
            }

            ui.vertical_centered(|ui| {
                let caption = match (equipped, blocked) {
                    (_, true) => RichText::new("Deux mains").small().color(MUTED),
                    (Some(e), _) => RichText::new(item.map_or(e.item.as_str(), item_name)).small(),
                    (None, _) => RichText::new(slot.label()).small().color(MUTED),
                };
                ui.add(egui::Label::new(caption).truncate());
                if let Some(e) = equipped.filter(|e| !e.swaps.is_empty()) {
                    ui.horizontal_wrapped(|ui| swap_icons(ui, &self.catalog, &self.icons, e, 22.0));
                }
                if slot == Slot::MainHand && swap_conflict {
                    ui.label(
                        RichText::new("⚠ swap à deux mains")
                            .small()
                            .color(HIGHLIGHT),
                    );
                }
            });
        });
    }

    /// Item, enchantment, spells and swaps of the selected slot.
    pub(super) fn details(&mut self, ui: &mut egui::Ui) {
        let slot = self.selected;
        ui.heading(slot.label());
        let conflicts = two_handed_swap_conflicts(&self.build, &self.catalog);
        // Borrow the catalog and the build as disjoint fields so nothing needs cloning per frame.
        let catalog = &self.catalog;
        let icons = &self.icons;
        let Some(equipped) = self.build.get_mut(slot) else {
            ui.weak("Aucun objet sélectionné. Choisis-en un ci-dessous.");
            return;
        };
        let Some(item) = catalog.item(&equipped.item) else {
            return;
        };

        let mut remove = false;
        ui.horizontal(|ui| {
            ui.add(
                item_icon(catalog, icons, &item.id, equipped.enchant)
                    .fit_to_exact_size(Vec2::splat(SLOT_ICON)),
            )
            .on_hover_ui(|ui| item_tooltip(ui, item, equipped.enchant));
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(item_name(item)).strong().size(18.0));
                    tier_badge(ui, item.tier);
                });
                enchant_row(ui, slot, &mut equipped.enchant);
                if ui.small_button("Retirer").clicked() {
                    remove = true;
                }
            });
        });

        if !item.spells.is_empty() || !item.passives.is_empty() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Recherche de sort");
                ui.add(egui::TextEdit::singleline(&mut self.spell_search).desired_width(160.0));
                if !self.spell_search.is_empty() && ui.small_button("✕").clicked() {
                    self.spell_search.clear();
                }
            });
        }
        let needle = self.spell_search.to_lowercase();
        spell_pickers(
            ui,
            catalog,
            icons,
            slot,
            item,
            &mut equipped.spells,
            &mut equipped.passives,
            &needle,
        );

        ui.add_space(8.0);
        ui.separator();
        let mut add_swap = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Swaps ({})", equipped.swaps.len())).strong());
            if ui.button("＋ Ajouter un swap").clicked() {
                add_swap = true;
            }
        });
        if equipped.swaps.is_empty() {
            ui.weak("Objets alternatifs pour cet emplacement (autre cape, autre potion, arme de rechange…).");
        }
        let count = equipped.swaps.len();
        let mut op = None;
        for (k, swap) in equipped.swaps.iter_mut().enumerate() {
            let conflict = slot == Slot::MainHand && conflicts.contains(&k);
            swap_editor(
                ui,
                catalog,
                icons,
                slot,
                (k, count),
                swap,
                &needle,
                conflict,
                &mut op,
            );
        }
        match op {
            Some(SwapOp::Up(k)) => {
                equipped.move_swap(k, k - 1);
            }
            Some(SwapOp::Down(k)) => {
                equipped.move_swap(k, k + 1);
            }
            Some(SwapOp::Remove(k)) => {
                equipped.remove_swap(k);
            }
            None => {}
        }

        if remove {
            self.build.clear(slot);
        }
        if add_swap {
            self.dialog = Dialog::AddSwap {
                slot,
                filter: Filter::default(),
            };
        }
    }

    pub(super) fn picker(&mut self, ui: &mut egui::Ui) {
        let slot = self.selected;
        if slot == Slot::OffHand && is_two_handed(&self.build, &self.catalog) {
            ui.weak("Main secondaire indisponible : l'arme équipée se tient à deux mains.");
            return;
        }
        let current = self.build.get(slot).map(|e| e.item.as_str());
        let picked = pick_item(
            ui,
            &self.catalog,
            &self.icons,
            slot,
            &mut self.filter,
            current,
        );
        if let Some(item) = picked
            && equip(&mut self.build, item)
        {
            self.status = "Main secondaire retirée : cette arme se tient à deux mains.".into();
        }
    }
}

/// Whether the equipped main-hand weapon takes both hands (and so blocks the off-hand).
pub(super) fn is_two_handed(build: &Build, catalog: &Catalog) -> bool {
    build
        .get(Slot::MainHand)
        .and_then(|e| catalog.item(&e.item))
        .is_some_and(|i| i.two_handed)
}

/// Empties the off-hand if a two-handed weapon is equipped; returns whether something was removed.
pub(super) fn enforce_two_handed(build: &mut Build, catalog: &Catalog) -> bool {
    let blocked = is_two_handed(build, catalog) && build.get(Slot::OffHand).is_some();
    if blocked {
        build.clear(Slot::OffHand);
    }
    blocked
}

/// Puts `item` in its slot with the first spell of each slot picked, keeping the enchantment and
/// whatever else the slot already carried (alternative items). Returns whether a two-handed
/// weapon made it drop the off-hand, so the caller can warn.
pub(super) fn equip(build: &mut Build, item: &Item) -> bool {
    let pick_first = |slot: u8| {
        item.spells
            .iter()
            .find(|s| s.slot == slot)
            .map(|s| s.id.clone())
    };
    let mut equipped = build.get(item.slot).cloned().unwrap_or_default();
    if !item.slot.enchantable() {
        equipped.enchant = 0;
    }
    equipped.item = item.id.clone();
    equipped.spells = (1..=item.active_slots).map(pick_first).collect();
    equipped.passives = (0..item.passive_slots)
        .map(|_| item.passives.first().cloned())
        .collect();
    build.set(item.slot, equipped);

    let drops_off_hand =
        item.slot == Slot::MainHand && item.two_handed && build.get(Slot::OffHand).is_some();
    if drops_off_hand {
        build.clear(Slot::OffHand);
    }
    drops_off_hand
}

/// Indices of the main-hand swaps that take two hands while an off-hand is equipped, i.e. swaps
/// that could not be used together with the off-hand as the build stands.
pub(super) fn two_handed_swap_conflicts(build: &Build, catalog: &Catalog) -> Vec<usize> {
    let Some(main) = build.get(Slot::MainHand) else {
        return Vec::new();
    };
    if build.get(Slot::OffHand).is_none() {
        return Vec::new();
    }
    main.swaps
        .iter()
        .enumerate()
        .filter(|(_, s)| catalog.item(&s.item).is_some_and(|i| i.two_handed))
        .map(|(k, _)| k)
        .collect()
}

/// Adds `item` as a swap of the item already equipped in its slot, starting from the same
/// enchantment and default spell picks. Returns false when the slot is empty.
pub(super) fn add_swap_to(build: &mut Build, item: &Item) -> bool {
    let Some(equipped) = build.get_mut(item.slot) else {
        return false;
    };
    equipped.add_swap(Swap::from_item(item, equipped.enchant));
    true
}

enum SwapOp {
    Up(usize),
    Down(usize),
    Remove(usize),
}

/// Spell and passive icon rows for an item; edits `spells` / `passives` in place.
#[allow(clippy::too_many_arguments)]
pub(super) fn spell_pickers(
    ui: &mut egui::Ui,
    catalog: &Catalog,
    icons: &Path,
    slot: Slot,
    item: &Item,
    spells: &mut [Option<String>],
    passives: &mut [Option<String>],
    needle: &str,
) {
    for (i, chosen) in spells.iter_mut().enumerate() {
        let choices: Vec<&str> = item
            .spells
            .iter()
            .filter(|s| s.slot as usize == i + 1)
            .map(|s| s.id.as_str())
            .collect();
        let label = if slot == Slot::MainHand {
            ["Q", "W", "E"].get(i).copied().unwrap_or("?")
        } else {
            "Sort"
        };
        if let Some(picked) = spell_row(
            ui,
            catalog,
            icons,
            label,
            &choices,
            chosen.as_deref(),
            needle,
        ) {
            *chosen = Some(picked);
        }
    }
    for (i, chosen) in passives.iter_mut().enumerate() {
        // Each passive slot has its own pool; fall back to the whole list for items without slot data.
        let mut choices: Vec<&str> = item
            .passive_choices
            .iter()
            .filter(|s| s.slot as usize == i + 1)
            .map(|s| s.id.as_str())
            .collect();
        if choices.is_empty() {
            choices = item.passives.iter().map(String::as_str).collect();
        }
        if let Some(picked) = spell_row(
            ui,
            catalog,
            icons,
            "Passif",
            &choices,
            chosen.as_deref(),
            needle,
        ) {
            *chosen = Some(picked);
        }
    }
}

/// One swap of the selected slot: item, enchantment, spells, note and ordering buttons.
/// `pos` is (index, total); the requested move/removal lands in `op`.
#[allow(clippy::too_many_arguments)]
fn swap_editor(
    ui: &mut egui::Ui,
    catalog: &Catalog,
    icons: &Path,
    slot: Slot,
    pos: (usize, usize),
    swap: &mut Swap,
    needle: &str,
    two_handed_conflict: bool,
    op: &mut Option<SwapOp>,
) {
    let (k, count) = pos;
    let item = catalog.item(&swap.item);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let icon = ui.add(item_icon(catalog, icons, &swap.item, swap.enchant).fit_to_exact_size(Vec2::splat(40.0)));
            match item {
                Some(item) => icon.on_hover_ui(|ui| item_tooltip(ui, item, swap.enchant)),
                None => icon.on_hover_text(&swap.item),
            };
            ui.vertical(|ui| {
                ui.label(RichText::new(item.map_or(swap.item.as_str(), item_name)).strong());
                enchant_row(ui, slot, &mut swap.enchant);
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("✕").on_hover_text("Supprimer ce swap").clicked() {
                    *op = Some(SwapOp::Remove(k));
                }
                if ui.add_enabled(k + 1 < count, egui::Button::new("▼").small()).clicked() {
                    *op = Some(SwapOp::Down(k));
                }
                if ui.add_enabled(k > 0, egui::Button::new("▲").small()).clicked() {
                    *op = Some(SwapOp::Up(k));
                }
            });
        });
        if two_handed_conflict {
            ui.colored_label(
                HIGHLIGHT,
                "⚠ Ce swap se tient à deux mains : la main secondaire équipée serait inutilisable avec lui.",
            );
        }
        if let Some(item) = item {
            spell_pickers(ui, catalog, icons, slot, item, &mut swap.spells, &mut swap.passives, needle);
        }
        ui.add(
            egui::TextEdit::singleline(&mut swap.note)
                .hint_text("Note du swap (quand l'utiliser)")
                .desired_width(f32::INFINITY),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog::load(Path::new("assets/data")).expect("catalog")
    }

    fn first_item(catalog: &Catalog, slot: Slot, two_handed: bool) -> &Item {
        catalog
            .items
            .iter()
            .find(|i| i.slot == slot && i.two_handed == two_handed)
            .expect("an item of that kind")
    }

    #[test]
    fn an_empty_build_is_not_two_handed() {
        assert!(!is_two_handed(&Build::default(), &catalog()));
    }

    #[test]
    fn equipping_a_two_handed_weapon_drops_the_off_hand() {
        let c = catalog();
        let mut build = Build::default();
        assert!(!equip(&mut build, first_item(&c, Slot::MainHand, false)));
        assert!(!equip(&mut build, first_item(&c, Slot::OffHand, false)));
        assert!(
            build.get(Slot::OffHand).is_some(),
            "a one-handed weapon leaves the off-hand alone"
        );

        assert!(
            equip(&mut build, first_item(&c, Slot::MainHand, true)),
            "reports the dropped off-hand"
        );
        assert!(is_two_handed(&build, &c));
        assert!(build.get(Slot::OffHand).is_none());
    }

    #[test]
    fn a_two_handed_weapon_without_off_hand_reports_nothing() {
        let c = catalog();
        let mut build = Build::default();
        assert!(!equip(&mut build, first_item(&c, Slot::MainHand, true)));
    }

    #[test]
    fn enforce_empties_an_off_hand_found_next_to_a_two_handed_weapon() {
        let c = catalog();
        let mut build = Build::default();
        equip(&mut build, first_item(&c, Slot::MainHand, true));
        equip(&mut build, first_item(&c, Slot::OffHand, false));
        assert!(
            build.get(Slot::OffHand).is_some(),
            "equip itself does not guard the off-hand"
        );

        assert!(enforce_two_handed(&mut build, &c));
        assert!(build.get(Slot::OffHand).is_none());
        assert!(!enforce_two_handed(&mut build, &c), "nothing left to drop");
    }

    #[test]
    fn replacing_an_item_keeps_the_enchant_and_picks_default_spells() {
        let c = catalog();
        let mut build = Build::default();
        let sword = first_item(&c, Slot::MainHand, false);
        equip(&mut build, sword);
        build.get_mut(Slot::MainHand).unwrap().enchant = 3;

        let other = c
            .items
            .iter()
            .find(|i| i.slot == Slot::MainHand && i.id != sword.id && i.active_slots > 0)
            .expect("a second weapon");
        equip(&mut build, other);
        let e = build.get(Slot::MainHand).unwrap();
        assert_eq!((e.item.as_str(), e.enchant), (other.id.as_str(), 3));
        assert_eq!(e.spells.len(), other.active_slots as usize);
    }

    #[test]
    fn a_swap_needs_an_equipped_item_in_the_slot() {
        let c = catalog();
        let mut build = Build::default();
        let cape = first_item(&c, Slot::Cape, false);
        assert!(!add_swap_to(&mut build, cape));
        assert!(
            build.get(Slot::Cape).is_none(),
            "no slot is created for a swap alone"
        );
    }

    #[test]
    fn a_swap_starts_from_the_main_enchant_and_default_picks() {
        let c = catalog();
        let mut build = Build::default();
        let main = first_item(&c, Slot::MainHand, false);
        equip(&mut build, main);
        build.get_mut(Slot::MainHand).unwrap().enchant = 2;

        let other = c
            .items
            .iter()
            .find(|i| i.slot == Slot::MainHand && i.id != main.id && i.active_slots > 0)
            .expect("another weapon");
        assert!(add_swap_to(&mut build, other));
        let e = build.get(Slot::MainHand).unwrap();
        assert_eq!(e.swaps.len(), 1);
        assert_eq!(
            (e.swaps[0].item.as_str(), e.swaps[0].enchant),
            (other.id.as_str(), 2)
        );
        assert_eq!(e.swaps[0].spells.len(), other.active_slots as usize);
        assert_eq!(e.item, main.id, "the main item is untouched");
    }

    #[test]
    fn replacing_the_main_item_keeps_its_swaps() {
        let c = catalog();
        let mut build = Build::default();
        let weapons: Vec<&Item> = c
            .items
            .iter()
            .filter(|i| i.slot == Slot::MainHand)
            .take(3)
            .collect();
        equip(&mut build, weapons[0]);
        add_swap_to(&mut build, weapons[1]);
        equip(&mut build, weapons[2]);
        let e = build.get(Slot::MainHand).unwrap();
        assert_eq!(e.item, weapons[2].id);
        assert_eq!(e.swaps.len(), 1);
        assert_eq!(e.swaps[0].item, weapons[1].id);
    }

    #[test]
    fn a_two_handed_swap_is_flagged_only_while_an_off_hand_is_equipped() {
        let c = catalog();
        let mut build = Build::default();
        equip(&mut build, first_item(&c, Slot::MainHand, false));
        add_swap_to(&mut build, first_item(&c, Slot::MainHand, true));
        add_swap_to(&mut build, first_item(&c, Slot::MainHand, false));
        assert!(
            two_handed_swap_conflicts(&build, &c).is_empty(),
            "no off-hand, nothing to conflict with"
        );

        equip(&mut build, first_item(&c, Slot::OffHand, false));
        assert_eq!(
            two_handed_swap_conflicts(&build, &c),
            vec![0],
            "only the two-handed swap, by index"
        );

        build.clear(Slot::MainHand);
        assert!(two_handed_swap_conflicts(&build, &c).is_empty());
    }

    #[test]
    fn the_mannequin_places_every_slot_exactly_once() {
        let placed: Vec<Slot> = MANNEQUIN.iter().flatten().flatten().copied().collect();
        assert_eq!(placed.len(), Slot::ALL.len());
        for slot in Slot::ALL {
            assert_eq!(placed.iter().filter(|s| **s == slot).count(), 1, "{slot:?}");
        }
    }
}
