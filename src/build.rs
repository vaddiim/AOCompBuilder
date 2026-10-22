//! The build model and its on-disk store ("Mes Builds").

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::catalog::{Catalog, Item, Slot};

/// Highest enchantment level in the game (`.4`).
pub const MAX_ENCHANT: u8 = 4;

/// One equipped item with the spells picked for it.
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Equipped {
    pub item: String,
    pub enchant: u8,
    /// Chosen spell per active slot (index 0 = Q).
    pub spells: Vec<Option<String>>,
    pub passives: Vec<Option<String>>,
    /// Alternative items for the same slot (e.g. another cape or potion for a given situation).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub swaps: Vec<Swap>,
}

/// An alternative item for the slot of the [`Equipped`] that owns it, with its own spells and passives.
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Swap {
    pub item: String,
    pub enchant: u8,
    pub spells: Vec<Option<String>>,
    pub passives: Vec<Option<String>>,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Build {
    pub name: String,
    /// Free-form note about the build (usage, rotation, …).
    #[serde(default)]
    pub note: String,
    pub slots: BTreeMap<u8, Equipped>,
}

/// First available spell per active slot and first passive per passive slot, as the editor picks them.
fn default_picks(item: &Item) -> (Vec<Option<String>>, Vec<Option<String>>) {
    let spells = (1..=item.active_slots)
        .map(|n| {
            item.spells
                .iter()
                .find(|s| s.slot == n)
                .map(|s| s.id.clone())
        })
        .collect();
    let passives = (0..item.passive_slots)
        .map(|_| item.passives.first().cloned())
        .collect();
    (spells, passives)
}

impl Equipped {
    /// `item` with default spell picks and no swaps. Non-enchantable slots force enchant 0.
    #[cfg(test)] // the editor builds its own Equipped; only test fixtures use this
    pub fn from_item(item: &Item, enchant: u8) -> Self {
        let (spells, passives) = default_picks(item);
        let enchant = if item.slot.enchantable() {
            enchant.min(MAX_ENCHANT)
        } else {
            0
        };
        Self {
            item: item.id.clone(),
            enchant,
            spells,
            passives,
            swaps: Vec::new(),
        }
    }

    pub fn add_swap(&mut self, swap: Swap) {
        self.swaps.push(swap);
    }

    /// Removes and returns swap `index`, if it exists.
    pub fn remove_swap(&mut self, index: usize) -> Option<Swap> {
        (index < self.swaps.len()).then(|| self.swaps.remove(index))
    }

    /// Moves swap `from` so it ends up at position `to`; returns false if either index is out of range.
    pub fn move_swap(&mut self, from: usize, to: usize) -> bool {
        if from >= self.swaps.len() || to >= self.swaps.len() {
            return false;
        }
        let swap = self.swaps.remove(from);
        self.swaps.insert(to, swap);
        true
    }
}

impl Swap {
    /// `item` with default spell picks and an empty note. Non-enchantable slots force enchant 0.
    pub fn from_item(item: &Item, enchant: u8) -> Self {
        let (spells, passives) = default_picks(item);
        let enchant = if item.slot.enchantable() {
            enchant.min(MAX_ENCHANT)
        } else {
            0
        };
        Self {
            item: item.id.clone(),
            enchant,
            spells,
            passives,
            note: String::new(),
        }
    }
}

/// A problem found while checking a build against the catalog.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Issue {
    EmptyName,
    /// A slot key that matches no [`Slot`].
    UnknownSlot(u8),
    UnknownItem {
        slot: Slot,
        item: String,
    },
    /// The item exists but belongs to another slot.
    WrongSlot {
        slot: Slot,
        item: String,
    },
    Enchant {
        slot: Slot,
        value: u8,
    },
    /// Off-hand equipped next to a two-handed weapon.
    TwoHandedOffHand,
    /// The spell list does not match the item's active/passive slot count.
    SlotCount {
        slot: Slot,
    },
    /// A spell or passive the item cannot use.
    Spell {
        slot: Slot,
        spell: String,
    },
    /// A problem with swap number `index` (0-based) of some slot; `issue` says which.
    InSwap {
        index: usize,
        issue: Box<Issue>,
    },
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Issue::EmptyName => write!(f, "le build n'a pas de nom"),
            Issue::UnknownSlot(k) => write!(f, "emplacement inconnu ({k})"),
            Issue::UnknownItem { slot, item } => {
                write!(f, "{} : objet inconnu « {item} »", slot.label())
            }
            Issue::WrongSlot { slot, item } => write!(
                f,
                "{} : « {item} » ne va pas dans cet emplacement",
                slot.label()
            ),
            Issue::Enchant { slot, value } => {
                write!(f, "{} : enchantement .{value} invalide", slot.label())
            }
            Issue::TwoHandedOffHand => write!(f, "main secondaire retirée (arme à deux mains)"),
            Issue::SlotCount { slot } => write!(f, "{} : nombre de sorts incorrect", slot.label()),
            Issue::Spell { slot, spell } => write!(
                f,
                "{} : sort « {spell} » indisponible pour cet objet",
                slot.label()
            ),
            Issue::InSwap { index, issue } => write!(f, "{issue} (swap {})", index + 1),
        }
    }
}

impl Build {
    pub fn get(&self, slot: Slot) -> Option<&Equipped> {
        self.slots.get(&(slot as u8))
    }

    pub fn get_mut(&mut self, slot: Slot) -> Option<&mut Equipped> {
        self.slots.get_mut(&(slot as u8))
    }

    pub fn set(&mut self, slot: Slot, equipped: Equipped) {
        self.slots.insert(slot as u8, equipped);
    }

    pub fn clear(&mut self, slot: Slot) {
        self.slots.remove(&(slot as u8));
    }

    /// Reports every inconsistency with the catalog without changing the build.
    /// The UI repairs on load/import instead, so this is only used by tests.
    #[cfg(test)]
    pub fn validate(&self, catalog: &Catalog) -> Vec<Issue> {
        self.clone().repair(catalog)
    }

    /// Fixes the build in place and returns what had to change: unusable items are dropped,
    /// enchants clamped, spell lists resized and invalid picks replaced by the item's first choice.
    pub fn repair(&mut self, catalog: &Catalog) -> Vec<Issue> {
        let mut issues = Vec::new();
        if self.name.trim().is_empty() {
            issues.push(Issue::EmptyName);
        }

        let keys: Vec<u8> = self.slots.keys().copied().collect();
        for key in keys {
            let Some(slot) = Slot::ALL.into_iter().find(|s| *s as u8 == key) else {
                self.slots.remove(&key);
                issues.push(Issue::UnknownSlot(key));
                continue;
            };
            let equipped = self.slots.get_mut(&key).expect("key was just listed");
            if !repair_loadout(
                catalog,
                slot,
                &equipped.item,
                &mut equipped.enchant,
                &mut equipped.spells,
                &mut equipped.passives,
                &mut issues,
            ) {
                self.slots.remove(&key);
                continue;
            }

            let mut index = 0;
            equipped.swaps.retain_mut(|swap| {
                let mut inner = Vec::new();
                let keep = repair_loadout(
                    catalog,
                    slot,
                    &swap.item,
                    &mut swap.enchant,
                    &mut swap.spells,
                    &mut swap.passives,
                    &mut inner,
                );
                issues.extend(inner.into_iter().map(|issue| Issue::InSwap {
                    index,
                    issue: Box::new(issue),
                }));
                index += 1;
                keep
            });
        }

        let two_handed = self
            .get(Slot::MainHand)
            .and_then(|e| catalog.item(&e.item))
            .is_some_and(|i| i.two_handed);
        if two_handed && self.get(Slot::OffHand).is_some() {
            self.clear(Slot::OffHand);
            issues.push(Issue::TwoHandedOffHand);
        }
        issues
    }
}

/// Checks one item choice (main item or swap) against the catalog and fixes its enchant and picks.
/// Returns false, after reporting why, when the item itself cannot be used in `slot`.
fn repair_loadout(
    catalog: &Catalog,
    slot: Slot,
    item_id: &str,
    enchant: &mut u8,
    spells: &mut Vec<Option<String>>,
    passives: &mut Vec<Option<String>>,
    issues: &mut Vec<Issue>,
) -> bool {
    let Some(item) = catalog.item(item_id) else {
        issues.push(Issue::UnknownItem {
            slot,
            item: item_id.to_owned(),
        });
        return false;
    };
    if item.slot != slot {
        issues.push(Issue::WrongSlot {
            slot,
            item: item_id.to_owned(),
        });
        return false;
    }

    let max = if slot.enchantable() { MAX_ENCHANT } else { 0 };
    if *enchant > max {
        issues.push(Issue::Enchant {
            slot,
            value: *enchant,
        });
        *enchant = max;
    }

    let mut resized = false;
    let mut fit = |picks: &mut Vec<Option<String>>,
                   len: u8,
                   valid: &dyn Fn(usize, &str) -> bool,
                   first: &dyn Fn(usize) -> Option<String>| {
        if picks.len() != len as usize {
            picks.resize(len as usize, None);
            resized = true;
        }
        for (i, pick) in picks.iter_mut().enumerate() {
            match pick {
                Some(id) if valid(i, id) => {}
                Some(id) => {
                    issues.push(Issue::Spell {
                        slot,
                        spell: std::mem::take(id),
                    });
                    *pick = first(i);
                }
                None => *pick = first(i),
            }
        }
    };
    fit(
        spells,
        item.active_slots,
        &|i, id| {
            item.spells
                .iter()
                .any(|s| s.slot as usize == i + 1 && s.id == id)
        },
        &|i| {
            item.spells
                .iter()
                .find(|s| s.slot as usize == i + 1)
                .map(|s| s.id.clone())
        },
    );
    fit(
        passives,
        item.passive_slots,
        &|_, id| item.passives.iter().any(|p| p == id),
        &|_| item.passives.first().cloned(),
    );
    if resized {
        issues.push(Issue::SlotCount { slot });
    }
    true
}

pub struct BuildStore {
    path: PathBuf,
    pub builds: Vec<Build>,
}

impl BuildStore {
    /// Loads the store, falling back to the `.bak` copy if the main file is missing or unreadable.
    pub fn load(path: PathBuf) -> Self {
        let builds = load_list(&path);
        Self { path, builds }
    }

    /// Writes to a temporary file then renames it over `builds.json`, keeping the previous
    /// version as `builds.json.bak`, so a crash mid-save never leaves a truncated store.
    pub fn save(&self) -> Result<()> {
        write_atomic(&self.path, &serde_json::to_vec_pretty(&self.builds)?, true)
    }

    /// Repairs every stored build against the catalog; returns `(build name, issues)` for those
    /// that needed changes. Call `save` afterwards to persist the repairs.
    pub fn validate_all(&mut self, catalog: &Catalog) -> Vec<(String, Vec<Issue>)> {
        self.builds
            .iter_mut()
            .filter_map(|b| {
                let issues = b.repair(catalog);
                (!issues.is_empty()).then(|| (b.name.clone(), issues))
            })
            .collect()
    }

    /// Insert or overwrite by name; returns whether an existing build was replaced.
    pub fn upsert(&mut self, build: Build) -> bool {
        match self.builds.iter_mut().find(|b| b.name == build.name) {
            Some(existing) => {
                *existing = build;
                true
            }
            None => {
                self.builds.push(build);
                false
            }
        }
    }
}

/// Reads a JSON list from `path`, else from its `.bak` copy, else returns an empty list. A main file
/// that exists but cannot be parsed is first copied to `.corrupt`: the next save would otherwise
/// replace the `.bak` with it and the user's data would be lost for good.
pub(crate) fn load_list<T: DeserializeOwned>(path: &Path) -> Vec<T> {
    let read = |p: &Path| {
        std::fs::read(p)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
    };
    if let Some(list) = read(path) {
        return list;
    }
    if path.exists() {
        let _ = std::fs::copy(path, sibling(path, "corrupt"));
    }
    read(&sibling(path, "bak")).unwrap_or_default()
}

/// `builds.json` -> `builds.json.bak` (appends rather than replaces the extension).
pub(crate) fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(suffix);
    path.with_file_name(name)
}

/// Writes `bytes` to `path` through a temporary sibling file; optionally keeps the old file as `.bak`.
pub fn write_atomic(path: &Path, bytes: &[u8], keep_backup: bool) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = sibling(path, "tmp");
    std::fs::write(&tmp, bytes)?;
    if keep_backup && path.exists() {
        std::fs::copy(path, sibling(path, "bak"))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Item;

    fn catalog() -> Catalog {
        Catalog::load(Path::new("assets/data")).expect("catalog")
    }

    /// A build with the first item of `slot` equipped, picks filled in like the editor does.
    fn equip(_catalog: &Catalog, build: &mut Build, item: &Item) {
        build.set(item.slot, Equipped::from_item(item, 0));
    }

    fn sword(catalog: &Catalog) -> &Item {
        catalog.item("T8_MAIN_SWORD").expect("T8_MAIN_SWORD")
    }

    fn valid_build(catalog: &Catalog) -> Build {
        let mut b = Build {
            name: "Test".into(),
            ..Default::default()
        };
        equip(catalog, &mut b, sword(catalog));
        b
    }

    #[test]
    fn clean_build_has_no_issues() {
        let c = catalog();
        assert_eq!(valid_build(&c).validate(&c), vec![]);
    }

    #[test]
    fn unknown_item_is_dropped() {
        let c = catalog();
        let mut b = valid_build(&c);
        b.set(
            Slot::Head,
            Equipped {
                item: "NOPE".into(),
                ..Default::default()
            },
        );
        let issues = b.repair(&c);
        assert_eq!(
            issues,
            vec![Issue::UnknownItem {
                slot: Slot::Head,
                item: "NOPE".into()
            }]
        );
        assert!(b.get(Slot::Head).is_none());
    }

    #[test]
    fn item_in_wrong_slot_is_dropped() {
        let c = catalog();
        let mut b = valid_build(&c);
        let e = b.get(Slot::MainHand).unwrap().clone();
        b.set(Slot::Head, e);
        assert!(matches!(
            b.repair(&c)[..],
            [Issue::WrongSlot {
                slot: Slot::Head,
                ..
            }]
        ));
        assert!(b.get(Slot::Head).is_none());
    }

    #[test]
    fn enchant_is_clamped() {
        let c = catalog();
        let mut b = valid_build(&c);
        b.get_mut(Slot::MainHand).unwrap().enchant = 9;
        assert_eq!(
            b.repair(&c),
            vec![Issue::Enchant {
                slot: Slot::MainHand,
                value: 9
            }]
        );
        assert_eq!(b.get(Slot::MainHand).unwrap().enchant, MAX_ENCHANT);
    }

    #[test]
    fn foreign_spell_is_replaced() {
        let c = catalog();
        let item = c
            .items
            .iter()
            .find(|i| i.slot == Slot::MainHand && i.spells.iter().any(|s| s.slot == 1))
            .expect("a weapon with a Q spell");
        let mut b = Build {
            name: "Test".into(),
            ..Default::default()
        };
        equip(&c, &mut b, item);
        b.get_mut(Slot::MainHand).unwrap().spells[0] = Some("NOT_A_SPELL".into());
        assert_eq!(
            b.repair(&c),
            vec![Issue::Spell {
                slot: Slot::MainHand,
                spell: "NOT_A_SPELL".into()
            }]
        );
        assert_eq!(b.validate(&c), vec![]);
        assert!(b.get(Slot::MainHand).unwrap().spells[0].is_some());
    }

    #[test]
    fn wrong_spell_count_is_resized() {
        let c = catalog();
        let mut b = valid_build(&c);
        b.get_mut(Slot::MainHand).unwrap().spells.clear();
        assert_eq!(
            b.repair(&c),
            vec![Issue::SlotCount {
                slot: Slot::MainHand
            }]
        );
        assert_eq!(
            b.get(Slot::MainHand).unwrap().spells.len(),
            sword(&c).active_slots as usize
        );
    }

    #[test]
    fn two_handed_weapon_removes_offhand() {
        let c = catalog();
        let two = c
            .items
            .iter()
            .find(|i| i.slot == Slot::MainHand && i.two_handed)
            .expect("2h weapon");
        let off = c
            .items
            .iter()
            .find(|i| i.slot == Slot::OffHand)
            .expect("offhand");
        let mut b = Build {
            name: "2h".into(),
            ..Default::default()
        };
        equip(&c, &mut b, two);
        equip(&c, &mut b, off);
        assert_eq!(b.repair(&c), vec![Issue::TwoHandedOffHand]);
        assert!(b.get(Slot::OffHand).is_none());
    }

    #[test]
    fn unknown_slot_key_and_empty_name_reported() {
        let c = catalog();
        let mut b = valid_build(&c);
        b.name = "  ".into();
        b.slots.insert(200, Equipped::default());
        let issues = b.repair(&c);
        assert!(issues.contains(&Issue::EmptyName) && issues.contains(&Issue::UnknownSlot(200)));
    }

    #[test]
    fn store_saves_atomically_with_backup_and_recovers() {
        let c = catalog();
        let dir = std::env::temp_dir().join(format!("aocb-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("builds.json");

        let mut store = BuildStore::load(path.clone());
        store.upsert(valid_build(&c));
        store.save().unwrap();
        store.upsert(Build {
            name: "Autre".into(),
            ..Default::default()
        });
        store.save().unwrap();
        assert!(!sibling(&path, "tmp").exists());
        assert!(sibling(&path, "bak").exists());
        assert_eq!(BuildStore::load(path.clone()).builds.len(), 2);

        std::fs::write(&path, b"{ truncated").unwrap();
        assert_eq!(
            BuildStore::load(path.clone()).builds.len(),
            1,
            "falls back to the .bak copy"
        );
        assert_eq!(
            std::fs::read(sibling(&path, "corrupt")).unwrap(),
            b"{ truncated",
            "the unreadable file is kept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn other_mainhand(catalog: &Catalog) -> &Item {
        catalog
            .items
            .iter()
            .find(|i| i.slot == Slot::MainHand && i.id != "T8_MAIN_SWORD")
            .expect("another weapon")
    }

    #[test]
    fn swap_helpers_add_remove_move() {
        let c = catalog();
        let item = other_mainhand(&c);
        let mut e = Equipped::from_item(sword(&c), 9);
        assert_eq!(e.enchant, MAX_ENCHANT);
        assert!(e.swaps.is_empty());
        for n in 0..3 {
            let mut s = Swap::from_item(item, 0);
            s.note = n.to_string();
            e.add_swap(s);
        }
        assert!(e.move_swap(0, 2));
        assert_eq!(
            e.swaps.iter().map(|s| s.note.as_str()).collect::<Vec<_>>(),
            ["1", "2", "0"]
        );
        assert!(!e.move_swap(0, 3) && !e.move_swap(3, 0));
        assert_eq!(e.remove_swap(1).map(|s| s.note), Some("2".into()));
        assert!(e.remove_swap(5).is_none());
        assert_eq!(e.swaps.len(), 2);
    }

    #[test]
    fn from_item_zeroes_enchant_on_consumables() {
        let c = catalog();
        let potion = c
            .items
            .iter()
            .find(|i| i.slot == Slot::Potion)
            .expect("a potion");
        assert_eq!(Equipped::from_item(potion, 3).enchant, 0);
        assert_eq!(Swap::from_item(potion, 3).enchant, 0);
    }

    #[test]
    fn swaps_are_repaired() {
        let c = catalog();
        let item = other_mainhand(&c);
        let head = c
            .items
            .iter()
            .find(|i| i.slot == Slot::Head)
            .expect("a helmet");
        let mut b = valid_build(&c);
        let e = b.get_mut(Slot::MainHand).unwrap();
        e.add_swap(Swap::from_item(item, 0));
        e.add_swap(Swap {
            item: "NOPE".into(),
            ..Default::default()
        });
        e.add_swap(Swap::from_item(head, 0));
        e.add_swap(Swap::from_item(sword(&c), 0)); // duplicate of the main item is fine
        let mut bad = Swap::from_item(item, 0);
        bad.enchant = 8;
        bad.spells[0] = Some("NOT_A_SPELL".into());
        e.add_swap(bad);

        let issues = b.repair(&c);
        assert_eq!(
            issues,
            vec![
                Issue::InSwap {
                    index: 1,
                    issue: Box::new(Issue::UnknownItem {
                        slot: Slot::MainHand,
                        item: "NOPE".into()
                    })
                },
                Issue::InSwap {
                    index: 2,
                    issue: Box::new(Issue::WrongSlot {
                        slot: Slot::MainHand,
                        item: head.id.clone()
                    })
                },
                Issue::InSwap {
                    index: 4,
                    issue: Box::new(Issue::Enchant {
                        slot: Slot::MainHand,
                        value: 8
                    })
                },
                Issue::InSwap {
                    index: 4,
                    issue: Box::new(Issue::Spell {
                        slot: Slot::MainHand,
                        spell: "NOT_A_SPELL".into()
                    })
                },
            ]
        );
        let swaps = &b.get(Slot::MainHand).unwrap().swaps;
        assert_eq!(swaps.len(), 3);
        assert_eq!(swaps[2].enchant, MAX_ENCHANT);
        assert_eq!(b.validate(&c), vec![]);
        assert!(issues[0].to_string().ends_with("(swap 2)"));
    }

    #[test]
    fn removing_the_main_item_drops_its_swaps() {
        let c = catalog();
        let mut b = valid_build(&c);
        b.get_mut(Slot::MainHand).unwrap().item = "NOPE".into();
        b.get_mut(Slot::MainHand)
            .unwrap()
            .add_swap(Swap::from_item(other_mainhand(&c), 0));
        assert_eq!(b.repair(&c).len(), 1);
        assert!(b.get(Slot::MainHand).is_none());
    }

    #[test]
    fn old_builds_without_note_or_swaps_still_load() {
        let old = r#"[{"name":"Vieux","slots":{"0":{"item":"T8_MAIN_SWORD","enchant":1,"spells":[null],"passives":[]}}}]"#;
        let builds: Vec<Build> = serde_json::from_str(old).unwrap();
        assert_eq!(builds[0].note, "");
        assert!(builds[0].get(Slot::MainHand).unwrap().swaps.is_empty());
        // New fields round-trip, and empty swaps stay out of the file.
        let mut b = builds[0].clone();
        b.note = "Rotation: Q puis E".into();
        assert!(!serde_json::to_string(&b).unwrap().contains("swaps"));
        b.get_mut(Slot::MainHand).unwrap().add_swap(Swap {
            item: "X".into(),
            note: "pvp".into(),
            ..Default::default()
        });
        let back: Build = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(back.note, b.note);
        assert_eq!(back.get(Slot::MainHand).unwrap().swaps[0].note, "pvp");
    }
}
