//! Item and spell catalog, derived from the ao-bin-dumps files in `assets/data`.
//!
//! The raw dumps are large, so the first run distils them into a compact
//! `catalog.json` that later runs load directly.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Slot {
    MainHand,
    OffHand,
    Head,
    Armor,
    Shoes,
    Cape,
    Bag,
    Mount,
    Food,
    Potion,
}

impl Slot {
    pub const ALL: [Slot; 10] = [
        Slot::MainHand,
        Slot::OffHand,
        Slot::Head,
        Slot::Armor,
        Slot::Shoes,
        Slot::Cape,
        Slot::Bag,
        Slot::Mount,
        Slot::Food,
        Slot::Potion,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Slot::MainHand => "Arme",
            Slot::OffHand => "Main secondaire",
            Slot::Head => "Casque",
            Slot::Armor => "Armure",
            Slot::Shoes => "Bottes",
            Slot::Cape => "Cape",
            Slot::Bag => "Sac",
            Slot::Mount => "Monture",
            Slot::Food => "Nourriture",
            Slot::Potion => "Potion",
        }
    }

    /// Consumables and mounts have no enchantment in the build.
    pub fn enchantable(self) -> bool {
        !matches!(self, Slot::Mount | Slot::Food | Slot::Potion)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpellChoice {
    pub id: String,
    /// 1-based active slot (Q = 1, W = 2, E = 3); 0 for passives.
    pub slot: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    /// Id of the render.albiononline.com icon (`icons/items/<icon>.png`, `<icon>_e<n>.png` when enchanted);
    /// equals `id` today (an item the renderer knows under another id would need an alias in `parse_item`).
    pub icon: String,
    pub name: String,
    /// French in-game name from `item_loc.json`; falls back to `name`. Never contains the tier.
    pub name_fr: String,
    /// Real tier of the item. Always 8 except for food and potions, which are listed at the highest tier
    /// their family exists in (e.g. omelettes T7, healing potions T6); show it next to the name.
    pub tier: u8,
    pub slot: Slot,
    pub category: String,
    pub two_handed: bool,
    pub active_slots: u8,
    pub passive_slots: u8,
    pub spells: Vec<SpellChoice>,
    pub passives: Vec<String>,
    /// Same spells as `passives`, with their 1-based passive slot (plate armour has two, most items one).
    pub passive_choices: Vec<SpellChoice>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Spell {
    pub id: String,
    /// English in-game name.
    pub name: String,
    /// French in-game name (falls back to `name`).
    pub name_fr: String,
    /// Plain-text descriptions, `$...$` stat placeholders resolved and colour tags stripped; empty if the game has none.
    pub description: String,
    pub description_fr: String,
    /// File stem of the icon under `assets/icons/spells`.
    pub icon: String,
    /// Seconds before the spell can be recast (`@recastdelay`).
    pub cooldown: f32,
    /// Energy cost (`@energyusage`).
    pub energy: f32,
    /// Cast range in metres (`@castrange`).
    pub range: f32,
    /// Cast time in seconds (`@castingtime`).
    pub cast_time: f32,
    /// Game category, e.g. `damage`, `crowdcontrol`, `heal`, `buff`, `mobility`.
    pub category: String,
    /// Defined as a `passivespell` in the dumps (equipped in a passive slot rather than Q/W/E).
    pub passive: bool,
}

/// Bumped whenever the cached `catalog.json` layout or filtering changes; stale caches are rebuilt.
const SCHEMA: u32 = 9;

/// The build only deals with this tier; item quality is ignored and enchants live in the build.
pub const TIER: u8 = 8;

/// Consumable families whose best tier is lower (grilled fish, seaweed salad…) are noise in a PvP/PvE builder.
const MIN_CONSUMABLE_TIER: u8 = 4;

#[derive(Default, Serialize, Deserialize)]
pub struct Catalog {
    #[serde(default)]
    schema: u32,
    pub items: Vec<Item>,
    pub spells: HashMap<String, Spell>,
    #[serde(skip)]
    index: HashMap<String, usize>,
}

impl Catalog {
    pub fn load(data_dir: &Path) -> Result<Self> {
        let cache = data_dir.join("catalog.json");
        let cached = std::fs::read(&cache)
            .ok()
            .and_then(|b| serde_json::from_slice::<Catalog>(&b).ok())
            .filter(|c| c.schema == SCHEMA);
        let mut catalog = match cached {
            Some(c) => c,
            None => {
                let built = Self::from_dumps(data_dir)?;
                std::fs::write(&cache, serde_json::to_vec(&built)?)?;
                built
            }
        };
        catalog.reindex();
        Ok(catalog)
    }

    fn reindex(&mut self) {
        self.index = self
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| (item.id.clone(), i))
            .collect();
    }

    pub fn item(&self, id: &str) -> Option<&Item> {
        self.index.get(id).map(|&i| &self.items[i])
    }

    pub fn spell(&self, id: &str) -> Option<&Spell> {
        self.spells.get(id)
    }

    /// Category keys that have items in `slot`, in the game's display order.
    pub fn categories(&self, slot: Slot) -> Vec<&str> {
        let mut keys: Vec<&str> = self
            .items
            .iter()
            .filter(|i| i.slot == slot)
            .map(|i| i.category.as_str())
            .collect();
        keys.sort_by_key(|k| (category_order(slot, k), *k));
        keys.dedup();
        keys
    }

    fn from_dumps(data_dir: &Path) -> Result<Self> {
        let names = read_item_names(&data_dir.join("items.txt"))?;
        let names_fr = read_item_loc(&data_dir.join("item_loc.json"));
        let spells_raw: Value =
            serde_json::from_slice(&std::fs::read(data_dir.join("spells.json"))?)?;
        let items_raw: Value =
            serde_json::from_slice(&std::fs::read(data_dir.join("items.json"))?)?;

        let loc = read_spell_loc(&data_dir.join("spell_loc.json"));
        let mut raw_spells: HashMap<&str, &Value> = HashMap::new();
        let mut spells = HashMap::new();
        for spell_kind in ["activespell", "passivespell", "togglespell"] {
            for s in list(&spells_raw["spells"][spell_kind]) {
                let Some(id) = attr(s, "uniquename") else {
                    continue;
                };
                let icon = attr(s, "uisprite").unwrap_or(id);
                let num = |k: &str| attr(s, k).and_then(|v| v.parse().ok()).unwrap_or(0.0);
                let tag = |explicit: Option<&str>, suffix: &str| {
                    explicit.map_or_else(|| format!("@SPELLS_{id}{suffix}"), str::to_owned)
                };
                let name = loc.get(&tag(attr(s, "namelocatag"), ""));
                raw_spells.insert(id, s);
                spells.insert(
                    id.to_owned(),
                    Spell {
                        id: id.to_owned(),
                        name: name.map_or_else(|| prettify(id), |n| n.en.clone()),
                        name_fr: name
                            .and_then(|n| n.fr.clone())
                            .unwrap_or_else(|| prettify(id)),
                        description: String::new(),
                        description_fr: String::new(),
                        icon: icon.to_owned(),
                        cooldown: num("recastdelay"),
                        energy: num("energyusage"),
                        range: num("castrange"),
                        cast_time: num("castingtime"),
                        category: attr(s, "category").unwrap_or("").to_owned(),
                        passive: spell_kind == "passivespell",
                    },
                );
            }
        }

        const KINDS: [&str; 5] = [
            "weapon",
            "transformationweapon",
            "equipmentitem",
            "mount",
            "consumableitem",
        ];
        // Higher tiers mostly point at a lower tier's spell list, so lookups need every item.
        let by_id: HashMap<&str, &Value> = KINDS
            .iter()
            .flat_map(|k| list(&items_raw["items"][*k]))
            .filter_map(|raw| Some((attr(raw, "uniquename")?, raw)))
            .collect();

        let mut items = Vec::new();
        for kind in KINDS {
            for raw in list(&items_raw["items"][kind]) {
                if let Some(item) = parse_item(kind, raw, &names, &names_fr, &spells, &by_id) {
                    items.push(item);
                }
            }
        }
        // Only spells reachable from a kept item are worth shipping.
        let used: HashSet<&str> = items
            .iter()
            .flat_map(|i| {
                i.spells
                    .iter()
                    .map(|s| s.id.as_str())
                    .chain(i.passives.iter().map(String::as_str))
            })
            .collect();
        spells.retain(|id, _| used.contains(id.as_str()));
        for spell in spells.values_mut() {
            let raw = raw_spells[spell.id.as_str()];
            let tag = attr(raw, "descriptionlocatag")
                .map_or_else(|| format!("@SPELLS_{}_DESC", spell.id), str::to_owned);
            if let Some(d) = loc.get(&tag) {
                spell.description = describe(&d.en, raw, &raw_spells);
                spell.description_fr =
                    d.fr.as_deref()
                        .map(|fr| describe(fr, raw, &raw_spells))
                        .unwrap_or_default();
            }
            if spell.description_fr.is_empty() {
                spell.description_fr = spell.description.clone();
            }
        }
        // Within a family of consumables only the highest tier is worth listing.
        let family = |i: &Item| {
            i.id.split_once('_')
                .map(|(_, f)| f.to_owned())
                .unwrap_or_default()
        };
        let mut best: HashMap<(Slot, String), u8> = HashMap::new();
        for i in items.iter().filter(|i| is_consumable(i.slot)) {
            let top = best.entry((i.slot, family(i))).or_default();
            *top = (*top).max(i.tier);
        }
        items.retain(|i| {
            !is_consumable(i.slot)
                || (i.tier >= MIN_CONSUMABLE_TIER && best[&(i.slot, family(i))] == i.tier)
        });
        items.sort_by(|a, b| {
            (a.slot as u8, &a.category, a.tier, &a.id).cmp(&(
                b.slot as u8,
                &b.category,
                b.tier,
                &b.id,
            ))
        });
        Ok(Catalog {
            schema: SCHEMA,
            items,
            spells,
            index: HashMap::new(),
        })
    }
}

fn parse_item(
    kind: &str,
    raw: &Value,
    names: &HashMap<String, String>,
    names_fr: &HashMap<String, String>,
    spells: &HashMap<String, Spell>,
    by_id: &HashMap<&str, &Value>,
) -> Option<Item> {
    let id = attr(raw, "uniquename")?;
    let slot = match (kind, attr(raw, "slottype")?) {
        (_, "mainhand") => Slot::MainHand,
        (_, "offhand") => Slot::OffHand,
        (_, "head") => Slot::Head,
        (_, "armor") => Slot::Armor,
        (_, "shoes") => Slot::Shoes,
        (_, "cape") => Slot::Cape,
        (_, "bag") => Slot::Bag,
        ("mount", _) => Slot::Mount,
        ("consumableitem", _) => match attr(raw, "shopsubcategory1")? {
            "food" => Slot::Food,
            "potions" => Slot::Potion,
            _ => return None,
        },
        _ => return None,
    };
    // Consumables carry their tier in the id; the dumps omit `@tier` on some of them.
    let tier = attr(raw, "tier")
        .and_then(|t| t.parse().ok())
        .or_else(|| id.strip_prefix('T')?.get(..1)?.parse().ok())
        .unwrap_or(0);
    let consumable = matches!(slot, Slot::Food | Slot::Potion);
    // Consumables rarely exist in T8 (omelettes are T3/5/7, healing T2/4/6): `from_dumps` keeps the best tier of each family.
    if tier != TIER && !(consumable && is_tiered_id(id)) {
        return None;
    }
    if !has_render_icon(id) {
        return None;
    }
    let name = names.get(id)?.clone();
    let name_fr = names_fr.get(id).cloned().unwrap_or_else(|| name.clone());
    let category = attr(raw, "shopsubcategory1").unwrap_or("").to_owned();
    if slot == Slot::MainHand && NOT_WEAPONS.contains(&category.as_str()) {
        return None;
    }

    let mut spell_choices = Vec::new();
    let mut passives = Vec::new();
    let mut passive_choices = Vec::new();
    for s in list(&spell_list(raw, by_id)["craftspell"]) {
        let Some(sid) = attr(s, "uniquename") else {
            continue;
        };
        if !spells.contains_key(sid) {
            continue;
        }
        // `@slots` is the Q/W/E slot of an active spell but the passive slot of a passive one; armour
        // actives carry none and sit in its single active slot.
        let slot = attr(s, "slots")
            .and_then(|n| n.parse::<u8>().ok())
            .unwrap_or(1);
        let choice = SpellChoice {
            id: sid.to_owned(),
            slot,
        };
        if spells[sid].passive {
            passives.push(sid.to_owned());
            passive_choices.push(choice);
        } else {
            spell_choices.push(choice);
        }
    }

    Some(Item {
        id: id.to_owned(),
        icon: id.to_owned(),
        name,
        name_fr,
        tier,
        slot,
        category,
        two_handed: attr(raw, "twohanded") == Some("true") || id.contains("_2H_"),
        active_slots: attr(raw, "activespellslots")
            .and_then(|n| n.parse().ok())
            .unwrap_or(0),
        passive_slots: attr(raw, "passivespellslots")
            .and_then(|n| n.parse().ok())
            .unwrap_or(0),
        spells: spell_choices,
        passives,
        passive_choices,
    })
}

/// Display order of the `shopsubcategory1` keys of a slot, as the game's market lists them.
fn category_keys(slot: Slot) -> &'static [&'static str] {
    match slot {
        Slot::MainHand => &[
            "bow",
            "crossbow",
            "axe",
            "dagger",
            "hammer",
            "knuckles",
            "mace",
            "quarterstaff",
            "spear",
            "sword",
            "arcanestaff",
            "cursestaff",
            "firestaff",
            "froststaff",
            "holystaff",
            "naturestaff",
            "shapeshifterstaff",
        ],
        Slot::OffHand => &["booktype", "torchtype", "shieldtype"],
        Slot::Head => &["cloth_helmet", "leather_helmet", "plate_helmet"],
        Slot::Armor => &["cloth_armor", "leather_armor", "plate_armor"],
        Slot::Shoes => &["cloth_shoes", "leather_shoes", "plate_shoes"],
        Slot::Cape => &[
            "accessoires_capes_capes",
            "accessoires_capes_bridgewatch",
            "accessoires_capes_fortsterling",
            "accessoires_capes_lymhurst",
            "accessoires_capes_martlock",
            "accessoires_capes_thetford",
            "accessoires_capes_caerleon",
            "accessoires_capes_brecilien",
            "accessoires_capes_heretic",
            "accessoires_capes_undead",
            "accessoires_capes_keeper",
            "accessoires_capes_morgana",
            "accessoires_capes_avalon",
            "accessoires_capes_demon",
            "accessoires_capes_smuggler",
        ],
        Slot::Bag => &["bags", "satchels"],
        Slot::Mount => &["basemounts", "raremounts", "battle_mount"],
        Slot::Food => &["food"],
        Slot::Potion => &["potions"],
    }
}

/// Gatherer gear is filed under the resource it harvests, whatever the slot.
const GATHERING: [&str; 6] = ["fish", "fiber", "hide", "ore", "rock", "wood"];

/// Main-hand categories that are not combat weapons (harvesting tools, guild siege tools, skins).
const NOT_WEAPONS: [&str; 10] = [
    "fiber", "fish", "hide", "ore", "rock", "wood", "guilds", "vanity", "tracking", "other",
];

/// Position of a category in the game's market order; unknown keys sort last.
pub fn category_order(slot: Slot, key: &str) -> u32 {
    if let Some(i) = category_keys(slot).iter().position(|k| *k == key) {
        return i as u32;
    }
    GATHERING
        .iter()
        .position(|k| *k == key)
        .map_or(1000, |i| 100 + i as u32)
}

/// French name of a category, as the game words it (plural, since it heads a list of items).
pub fn category_label(_slot: Slot, key: &str) -> String {
    let fixed = match key {
        "sword" => "Épées",
        "axe" => "Haches",
        "mace" => "Masses",
        "hammer" => "Marteaux",
        "spear" => "Lances",
        "quarterstaff" => "Longs bâtons",
        "dagger" => "Dagues",
        "bow" => "Arcs",
        "crossbow" => "Arbalètes",
        "firestaff" => "Bâtons de feu",
        "holystaff" => "Bâtons bénis",
        "arcanestaff" => "Bâtons ésotériques",
        "froststaff" => "Bâtons de givre",
        "cursestaff" => "Bâtons damnés",
        "naturestaff" => "Bâtons naturels",
        "shapeshifterstaff" => "Bâtons de Changeforme",
        "knuckles" => "Gants de guerre",
        "shieldtype" => "Boucliers",
        "booktype" => "Livres",
        "torchtype" => "Torches",
        "cloth_armor" => "Robes en tissu",
        "leather_armor" => "Vestes en cuir",
        "plate_armor" => "Armures en plaques",
        "cloth_helmet" => "Capuchons en tissu",
        "leather_helmet" => "Capuches en cuir",
        "plate_helmet" => "Casques en plaques",
        "cloth_shoes" => "Sandales en tissu",
        "leather_shoes" => "Chaussures en cuir",
        "plate_shoes" => "Bottes de plaques",
        "bags" => "Sacs",
        "satchels" => "Sacoches de perspicacité",
        "basemounts" => "Montures de base",
        "raremounts" => "Montures rares",
        "battle_mount" => "Montures de bataille",
        "food" => "Nourriture",
        "potions" => "Potions",
        "accessoires_capes_capes" => "Capes",
        "accessoires_capes_bridgewatch" => "Bridgewatch",
        "accessoires_capes_fortsterling" => "Fort Sterling",
        "accessoires_capes_lymhurst" => "Lymhurst",
        "accessoires_capes_martlock" => "Martlock",
        "accessoires_capes_thetford" => "Thetford",
        "accessoires_capes_caerleon" => "Caerleon",
        "accessoires_capes_brecilien" => "Brécilien",
        "accessoires_capes_heretic" => "Hérétique",
        "accessoires_capes_undead" => "Morts-vivants",
        "accessoires_capes_keeper" => "Gardien",
        "accessoires_capes_morgana" => "Morgana",
        "accessoires_capes_avalon" => "Avalonien",
        "accessoires_capes_demon" => "Démon",
        "accessoires_capes_smuggler" => "Contrebandier",
        _ => "",
    };
    if !fixed.is_empty() {
        return fixed.to_owned();
    }
    let resource = match key {
        "fish" => "poisson",
        "fiber" => "fibre",
        "hide" => "peau",
        "ore" => "minerai",
        "rock" => "pierre",
        "wood" => "bois",
        _ => "",
    };
    if !resource.is_empty() {
        return format!("Récolte : {resource}");
    }
    // Unknown key: make it readable rather than show the raw id.
    let mut label = key.replace('_', " ");
    if let Some(first) = label.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    label
}

fn is_consumable(slot: Slot) -> bool {
    matches!(slot, Slot::Food | Slot::Potion)
}

/// `T5_MEAL_SOUP` style ids; event consumables (`UNIQUE_CONSUMABLE_…`) are not part of any tier family.
fn is_tiered_id(id: &str) -> bool {
    let mut c = id.chars();
    c.next() == Some('T') && c.next().is_some_and(|d| d.is_ascii_digit()) && c.next() == Some('_')
}

/// The render service has no icon for vanity skins, prototypes and a couple of odd items, so the UI could
/// only show them blank.
fn has_render_icon(id: &str) -> bool {
    const MISSING: [&str; 2] = ["T8_CAPE_ARENA_BANNER", "T8_2H_IRONGAUNTLETS_HELL"];
    !(id.contains("VANITY") || id.contains("PROTOTYPE") || MISSING.contains(&id))
}

/// A `craftingspelllist` is either inline or `@reference` to another item's list.
fn spell_list<'a>(raw: &'a Value, by_id: &HashMap<&str, &'a Value>) -> &'a Value {
    let mut cur = raw;
    for _ in 0..16 {
        let Some(target) = attr(&cur["craftingspelllist"], "reference") else {
            break;
        };
        let Some(next) = by_id.get(target) else { break };
        cur = next;
    }
    &cur["craftingspelllist"]
}

/// One localized string pair from `spell_loc.json` (`tag -> {en, fr}`).
#[derive(Deserialize)]
struct LocEntry {
    en: String,
    fr: Option<String>,
}

/// `spell_loc.json` is distilled from the 90 MB `localization.json` of ao-bin-dumps; it is optional,
/// without it spells fall back to names derived from their ids and have no description.
fn read_spell_loc(path: &Path) -> HashMap<String, LocEntry> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Turns a localized description into plain text: stat placeholders are looked up in the spell
/// dumps and colour tags such as `[dmg]...[/dmg]` are dropped.
fn describe(text: &str, spell: &Value, spells: &HashMap<&str, &Value>) -> String {
    // `{0}` marks a value the game computes from the player's stats, which the dumps cannot give.
    let text = (0..4).fold(text.replace("\\n", "\n"), |t, i| {
        t.replace(&format!("{{{i}}}"), "X")
    });
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        // `$$SPELL.path$` and `$path$` both occur.
        let body = rest[start..].trim_start_matches('$');
        match body.find('$') {
            Some(end) => {
                out.push_str(
                    &resolve(&body[..end], spell, spells).unwrap_or_else(|| "?".to_owned()),
                );
                rest = &body[end + 1..];
            }
            None => {
                rest = body;
                break;
            }
        }
    }
    out.push_str(rest);
    strip_tags(&out)
}

/// Drops `[word]` / `[/word]` markup but keeps other square brackets.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let tail = &rest[open + 1..];
        match tail.find(']') {
            Some(close)
                if tail[..close]
                    .trim_start_matches('/')
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && close > 0 =>
            {
                rest = &tail[close + 1..];
            }
            _ => {
                out.push('[');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Resolves `SPELL.node[0].attr` (or a path relative to `spell`) to its printed value.
fn resolve(path: &str, spell: &Value, spells: &HashMap<&str, &Value>) -> Option<String> {
    let mut segs = path.trim().split('.').peekable();
    let mut cur = spell;
    if let Some(other) = segs.peek().and_then(|first| spells.get(first)) {
        cur = other;
        segs.next();
    }
    let segs: Vec<&str> = segs.collect();
    let (last, steps) = segs.split_last()?;
    for seg in steps {
        cur = step(cur, seg)?;
    }
    let leaf = if last.contains('[') {
        step(cur, last)?
    } else {
        cur.get(format!("@{last}")).or_else(|| cur.get(*last))?
    };
    let raw = match leaf {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let Ok(n) = raw.parse::<f64>() else {
        return Some(raw);
    };
    // The game formats by attribute kind; mirror the common ones. Damage and debuff values are
    // stored negative, and fractional buff values are ratios shown as percentages.
    let fmt = |n: f64| format!("{}", (n * 100.0).round() / 100.0);
    let last = last.split('[').next().unwrap_or(last);
    Some(
        if last.ends_with("time")
            || last.ends_with("duration")
            || last.ends_with("interval")
            || last.ends_with("delay")
        {
            format!("{}s", fmt(n))
        } else if last == "value" && n.abs() <= 1.0 {
            format!("{}%", fmt(n.abs() * 100.0))
        } else if last.contains("radius") || last.contains("distance") || last.contains("range") {
            format!("{}m", fmt(n))
        } else {
            fmt(n.abs())
        },
    )
}

/// `name[2]` -> third element of `cur[name]`; a lone element counts as index 0.
fn step<'a>(cur: &'a Value, seg: &str) -> Option<&'a Value> {
    let (name, idx) = match seg.split_once('[') {
        Some((n, i)) => (n, i.trim_end_matches(']').parse::<usize>().ok()?),
        None => (seg, 0),
    };
    match cur.get(name)? {
        Value::Array(a) => a.get(idx),
        v if idx == 0 => Some(v),
        _ => None,
    }
}

/// The dumps convert XML to JSON, so a repeated element is an array but a lone one is an object.
fn list(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![v],
        _ => Vec::new(),
    }
}

fn attr<'a>(v: &'a Value, name: &str) -> Option<&'a str> {
    v.get(format!("@{name}"))?.as_str()
}

/// `item_loc.json` maps an item id to its French name (distilled from the game's localization); optional.
fn read_item_loc(path: &Path) -> HashMap<String, String> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// `items.txt` lines look like `  12: T4_BAG   : Adept's Bag`; unnamed entries are skipped.
fn read_item_names(path: &Path) -> Result<HashMap<String, String>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut names = HashMap::new();
    for line in text.lines() {
        let mut parts = line.splitn(3, ':');
        let (_, Some(id), Some(name)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let (id, name) = (id.trim(), name.trim());
        if !id.is_empty() && !name.is_empty() {
            names.insert(id.to_owned(), name.to_owned());
        }
    }
    Ok(names)
}

/// `PASSIVE_REDUCE_DMG_SWORD` -> `Reduce Dmg Sword`; the dumps ship no spell localization.
fn prettify(id: &str) -> String {
    let id = id.strip_prefix("PASSIVE_").unwrap_or(id);
    id.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| {
                    f.to_uppercase()
                        .chain(c.flat_map(char::to_lowercase))
                        .collect::<String>()
                })
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dumps_yield_weapons_with_spells() {
        // Going through `load` also rewrites a stale `catalog.json` cache.
        let catalog = Catalog::load(Path::new("assets/data")).unwrap();
        assert_eq!(catalog.schema, SCHEMA);
        let sword = catalog
            .items
            .iter()
            .find(|i| i.id == "T8_MAIN_SWORD")
            .expect("T8_MAIN_SWORD");
        assert_eq!(sword.slot, Slot::MainHand);
        assert_eq!(sword.active_slots, 3);
        assert!((1..=3).all(|s| sword.spells.iter().any(|c| c.slot == s)));
        assert!(!sword.passives.is_empty());
        assert!(
            catalog
                .items
                .iter()
                .all(|i| i.tier == TIER || is_consumable(i.slot)),
            "non-T8 gear kept"
        );
        assert!(
            catalog
                .items
                .iter()
                .flat_map(|i| i.spells.iter().map(|s| &s.id).chain(&i.passives))
                .all(|id| catalog.spells.contains_key(id))
        );
        for slot in Slot::ALL {
            assert!(
                catalog.items.iter().any(|i| i.slot == slot),
                "no items for {slot:?}"
            );
        }
    }

    #[test]
    fn spells_have_real_names_and_stats() {
        let catalog = Catalog::from_dumps(Path::new("assets/data")).unwrap();
        let name = |id: &str| {
            catalog
                .spell(id)
                .unwrap_or_else(|| panic!("{id}"))
                .name
                .as_str()
        };
        assert_eq!(name("MIGHTYBLOW"), "Mighty Blow");
        assert_eq!(name("PASSIVE_REDUCE_DMG_SWORD"), "Weakening");
        let blow = catalog.spell("MIGHTYBLOW").unwrap();
        assert!(!blow.description.is_empty() && !blow.description_fr.is_empty());
        assert!(blow.cooldown > 0.0 && blow.energy >= 0.0 && !blow.category.is_empty());
        for s in catalog.spells.values() {
            for text in [&s.description, &s.description_fr] {
                assert!(
                    !text.contains('$') && !text.contains("[/"),
                    "{}: {text}",
                    s.id
                );
            }
        }
        // Names equal to their prettified id are usually genuine ("Block"), so this only catches a missing loc file.
        let guessed = catalog
            .spells
            .values()
            .filter(|s| s.name == prettify(&s.id))
            .count();
        assert!(
            guessed * 5 < catalog.spells.len(),
            "{guessed}/{} names guessed",
            catalog.spells.len()
        );
    }

    /// Ids the render service has no icon for (and no alias): kept out of the catalog.
    const NO_ICON: [&str; 2] = ["T8_CAPE_ARENA_BANNER", "T8_2H_IRONGAUNTLETS_HELL"];

    /// Every T8 equippable in the dumps is in the catalog, except the documented exclusions.
    #[test]
    fn catalog_matches_dumps() {
        let dir = Path::new("assets/data");
        let mut catalog = Catalog::from_dumps(dir).unwrap();
        catalog.reindex();
        let names = read_item_names(&dir.join("items.txt")).unwrap();
        let raw: Value =
            serde_json::from_slice(&std::fs::read(dir.join("items.json")).unwrap()).unwrap();
        // Highest tier of each food/potion family, computed straight from the dumps.
        let tier_of = |item: &Value, id: &str| -> u8 {
            attr(item, "tier")
                .and_then(|t| t.parse().ok())
                .unwrap_or_else(|| id[1..2].parse().unwrap())
        };
        let is_meal = |item: &Value, id: &str| {
            matches!(attr(item, "slottype"), Some("food" | "potion"))
                && matches!(attr(item, "shopsubcategory1"), Some("food" | "potions"))
                && id.len() > 3
                && id.starts_with('T')
                && id.as_bytes()[1].is_ascii_digit()
                && id.as_bytes()[2] == b'_'
                && !id.contains("PROTOTYPE")
                && names.contains_key(id)
        };
        let mut best: HashMap<&str, u8> = HashMap::new();
        for item in list(&raw["items"]["consumableitem"]) {
            let id = attr(item, "uniquename").unwrap();
            if is_meal(item, id) {
                let top = best.entry(&id[3..]).or_default();
                *top = (*top).max(tier_of(item, id));
            }
        }
        assert!(best.len() > 30, "only {} consumable families", best.len());

        let mut seen = 0;
        for kind in [
            "weapon",
            "transformationweapon",
            "equipmentitem",
            "mount",
            "consumableitem",
        ] {
            for item in list(&raw["items"][kind]) {
                let id = attr(item, "uniquename").unwrap();
                let slottype = attr(item, "slottype").unwrap_or("");
                let category = attr(item, "shopsubcategory1").unwrap_or("");
                if is_meal(item, id) {
                    seen += 1;
                    let wanted = tier_of(item, id) == best[&id[3..]]
                        && best[&id[3..]] >= MIN_CONSUMABLE_TIER;
                    assert_eq!(
                        catalog.item(id).is_some(),
                        wanted,
                        "{id} (best tier of its family: {})",
                        best[&id[3..]]
                    );
                    continue;
                }
                let equippable = [
                    "mainhand", "offhand", "head", "armor", "shoes", "cape", "bag", "mount",
                ];
                if !id.starts_with("T8_")
                    || !equippable.contains(&slottype)
                    || !names.contains_key(id)
                {
                    continue;
                }
                seen += 1;
                let excluded = id.contains("VANITY")
                    || id.contains("PROTOTYPE")
                    || NO_ICON.contains(&id)
                    || (slottype == "mainhand" && NOT_WEAPONS.contains(&category));
                assert_eq!(
                    catalog.item(id).is_some(),
                    !excluded,
                    "{id} ({slottype}/{category})"
                );
            }
        }
        assert!(seen > 300, "only {seen} items audited");
        // Raw fish, fireworks and event consumables are not meals.
        assert!(catalog.items.iter().all(|i| !i.id.contains("FISH_")
            && !i.id.contains("FIREWORKS")
            && !i.id.starts_with("UNIQUE_CONSUMABLE")));
        assert!(
            catalog.item("T7_MEAL_OMELETTE").is_some() && catalog.item("T6_POTION_HEAL").is_some()
        );
        assert!(
            catalog.item("T8_MEAL_OMELETTE").is_none()
                && catalog.item("T5_MEAL_OMELETTE").is_none()
        );
        assert!(catalog.item("T8_2H_SHAPESHIFTER_HELL").is_some());
    }

    #[test]
    fn icons_and_two_handed_flags() {
        let mut catalog = Catalog::from_dumps(Path::new("assets/data")).unwrap();
        catalog.reindex();
        // Elder's Black Hands has no icon of its own; showing Hellfire Hands' would be wrong.
        assert!(catalog.item("T8_2H_IRONGAUNTLETS_HELL").is_none());
        assert!(catalog.item("T8_2H_KNUCKLES_HELL").is_some());
        assert!(catalog.items.iter().all(|i| i.icon == i.id));
        for item in catalog.items.iter().filter(|i| i.slot == Slot::MainHand) {
            assert_eq!(item.two_handed, item.id.contains("_2H_"), "{}", item.id);
        }
    }

    #[test]
    fn categories_follow_the_game_order() {
        let catalog = Catalog::from_dumps(Path::new("assets/data")).unwrap();
        let weapons = catalog.categories(Slot::MainHand);
        assert_eq!(
            (weapons.first(), weapons.last()),
            (Some(&"bow"), Some(&"shapeshifterstaff"))
        );
        assert_eq!(weapons.len(), 17, "{weapons:?}");
        assert_eq!(category_label(Slot::MainHand, "sword"), "Épées");
        assert_eq!(
            category_label(Slot::MainHand, "knuckles"),
            "Gants de guerre"
        );
        assert_eq!(category_label(Slot::Armor, "mystery_kind"), "Mystery kind");
        for slot in Slot::ALL {
            let keys = catalog.categories(slot);
            assert!(!keys.is_empty(), "{slot:?}");
            assert!(
                keys.windows(2)
                    .all(|w| category_order(slot, w[0]) <= category_order(slot, w[1]))
            );
            for key in keys {
                assert!(!category_label(slot, key).contains('_'), "{slot:?} {key}");
            }
        }
    }

    #[test]
    fn items_have_icons_when_downloaded() {
        let dir = Path::new("assets/icons/items");
        if !dir.exists() {
            return;
        }
        let catalog = Catalog::from_dumps(Path::new("assets/data")).unwrap();
        let missing: Vec<&str> = catalog
            .items
            .iter()
            .map(|i| i.icon.as_str())
            .filter(|icon| !dir.join(format!("{icon}.png")).exists())
            .collect();
        assert!(missing.is_empty(), "items without an icon: {missing:?}");
    }

    /// `@slots` 1..=3 are Q/W/E: every weapon-like item offers at least one spell per active slot,
    /// never one beyond its slot count, and passive pickers are never empty.
    #[test]
    fn spell_slots_are_consistent() {
        let catalog = Catalog::from_dumps(Path::new("assets/data")).unwrap();
        for item in &catalog.items {
            assert!(
                item.spells
                    .iter()
                    .all(|c| (1..=item.active_slots).contains(&c.slot)),
                "{}",
                item.id
            );
            for slot in 1..=item.active_slots {
                assert!(
                    item.spells.iter().any(|c| c.slot == slot),
                    "{} has no spell for slot {slot}",
                    item.id
                );
            }
            assert!(
                item.passive_slots == 0 || !item.passives.is_empty(),
                "{} has no passives",
                item.id
            );
        }
    }

    #[test]
    fn items_have_french_names() {
        let mut catalog = Catalog::from_dumps(Path::new("assets/data")).unwrap();
        catalog.reindex();
        assert_eq!(
            catalog.item("T8_MAIN_SWORD").unwrap().name_fr,
            "Épée large du sage"
        );
        assert_eq!(
            catalog.item("T7_MEAL_OMELETTE").unwrap().name_fr,
            "Omelette au porc"
        );
        // Items missing from item_loc.json fall back to English; that should stay rare.
        let untranslated = catalog.items.iter().filter(|i| i.name_fr == i.name).count();
        assert!(
            untranslated * 20 < catalog.items.len(),
            "{untranslated}/{} items untranslated",
            catalog.items.len()
        );
    }

    /// Regenerates `assets/data/item_loc.json` (French item names of the catalog):
    /// `AO_LOCALIZATION=path/to/localization.json cargo test distil_item_names -- --ignored`
    #[test]
    #[ignore]
    fn distil_item_names() {
        let src = std::env::var("AO_LOCALIZATION").expect("AO_LOCALIZATION");
        let catalog = Catalog::from_dumps(Path::new("assets/data")).unwrap();
        let doc: Value = serde_json::from_slice(&std::fs::read(src).unwrap()).unwrap();
        let mut out = std::collections::BTreeMap::new();
        for tu in list(&doc["tmx"]["body"]["tu"]) {
            let Some(id) = attr(tu, "tuid").and_then(|t| t.strip_prefix("@ITEMS_")) else {
                continue;
            };
            if !catalog.items.iter().any(|i| i.id == id) {
                continue;
            }
            let fr = list(&tu["tuv"])
                .into_iter()
                .find(|v| attr(v, "xml:lang") == Some("FR-FR"))
                .and_then(|v| v["seg"].as_str());
            if let Some(fr) = fr.filter(|s| !s.is_empty()) {
                out.insert(id.to_owned(), fr.to_owned());
            }
        }
        std::fs::write(
            "assets/data/item_loc.json",
            serde_json::to_vec(&out).unwrap(),
        )
        .unwrap();
    }

    /// Regenerates `assets/data/spell_loc.json` from the full `localization.json` of ao-bin-dumps:
    /// `AO_LOCALIZATION=path/to/localization.json cargo test distil_localization -- --ignored`
    #[test]
    #[ignore]
    fn distil_localization() {
        let src = std::env::var("AO_LOCALIZATION").expect("AO_LOCALIZATION");
        let doc: Value = serde_json::from_slice(&std::fs::read(src).unwrap()).unwrap();
        let mut out = std::collections::BTreeMap::new();
        for tu in list(&doc["tmx"]["body"]["tu"]) {
            let Some(tag) = attr(tu, "tuid").filter(|t| t.starts_with("@SPELLS_")) else {
                continue;
            };
            let seg = |lang: &str| {
                list(&tu["tuv"])
                    .into_iter()
                    .find(|v| attr(v, "xml:lang") == Some(lang))
                    .and_then(|v| v["seg"].as_str())
            };
            let Some(en) = seg("EN-US") else { continue };
            out.insert(
                tag.to_owned(),
                serde_json::json!({ "en": en, "fr": seg("FR-FR") }),
            );
        }
        std::fs::write(
            "assets/data/spell_loc.json",
            serde_json::to_vec(&out).unwrap(),
        )
        .unwrap();
    }
}
