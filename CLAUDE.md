# AO Comp Builder

Rust + egui editor for Albion Online builds. Scope is **tier 8** (`catalog::TIER`); item quality is ignored
and enchants 0–4 live in the build, not the catalog. The one exception is food and potions, many of which
have no T8 version: each family is listed at its highest tier (`Item.tier`, e.g. omelettes T7, healing T6).

## Commands

Run from the project root (the app resolves `assets/` and `userdata/` from the working directory).

- `cargo run` / `cargo test` / `cargo clippy --all-targets`
- When several people or agents work at once, give each its own target dir to avoid lock waits:
  `$env:CARGO_TARGET_DIR='target-<name>'` (all `target*/` dirs are ignored).
- `scripts/fetch_dumps.ps1` downloads the game dumps; `scripts/fetch_icons.ps1 [-DryRun]` downloads
  icons listed by the catalog. Neither is needed for tests except the dumps in `assets/data`.

## Architecture (`src/`)

- `catalog.rs` – read-only game data. `Catalog::load` reads the cache `assets/data/catalog.json`
  and rebuilds it from the dumps (`from_dumps`) when its `schema` differs from `SCHEMA`.
  **Bump `SCHEMA` whenever the layout or filtering of the catalog changes.**
  - `Item`: `name` (EN, from `items.txt`) and `name_fr` (from `item_loc.json`, falls back to `name`; the tier is never
    part of a name, display `tier` separately). T8 weapons, equipment, mounts; food and potions at the best tier of their family (so `tier` is not
    uniform: show it in the UI). Raw fish, fireworks and event consumables are not listed. `spells` are the Q/W/E choices
    (`SpellChoice.slot` 1..=3; armour has a single active slot 1), `passives`/`passive_choices` the passives.
  - `Spell`: EN/FR name and description, cooldown, energy, range, cast time, category, icon stem.
    Only spells reachable from a kept item are stored.
- `build.rs` – `Build`/`Equipped` model, `BuildStore` (`userdata/builds.json`, atomic writes with backup)
  and validation of a build against the catalog.
- `share.rs` – export/import of a build as a base64 string or `.json` file.
- `comp.rs` – compositions: roles pointing at builds by name.
- `ui/` – egui front end (`mod.rs` holds the app state and tabs):
  - `theme.rs` palette, fonts (`assets/fonts/UI-Regular.ttf` if present, else Segoe UI) and egui style; `widgets.rs` icons,
    tooltips, spell rows, the item picker; `editor.rs` the build editor (slots, details, two-handed and swap rules);
    `my_builds.rs` saved builds; `comps.rs` compositions; `dialogs.rs` import/export, rename and confirmation windows.
    A `build_detail` screen is planned.
- `export_image.rs` – PNG export of a build or composition, drawn off-screen (no egui, can run on a worker thread);
  fetches 217 px icons on demand into `userdata/cache/icons_hd`, else uses the 64 px ones. Icons are drawn with their
  frame fitted to the slot square (`icon_box`: the render service leaves a transparent margin). A composition is a table:
  one row per member with the whole equipment on one line, one column per slot (slots nobody fills are left out), each item's
  swaps in small under it, and roles get a colour from their name (`role_color`).
- `main.rs` – startup: loads the catalog, `userdata/builds.json` and `comps.json`, opens the window.

Asset folders: `assets/data` (dumps, `spell_loc.json`, `item_loc.json`, cache), `assets/icons`, `assets/fonts`.
User data lives in `userdata/` (builds, comps, `exports/`, `cache/`).

## Data quirks (from the dumps)

- Most items from T5 up have `craftingspelllist` set to `@reference` another item; `spell_list` follows the chain.
- `@slots` on a weapon spell is its Q/W/E slot; on a passive it is the passive slot (plate armour has two).
  Armour actives have no `@slots`. Active vs passive is decided by the spell's kind in `spells.json`.
- Spell texts come from `spell_loc.json` and item names from `item_loc.json` (both distilled from the 90 MB `localization.json`
  by ignored tests: see `distil_localization` and `distil_item_names` in `catalog.rs`); `$path$` placeholders are resolved against `spells.json`
  (`describe`). Values the game computes from player stats show as `X`.
- Items the render service has no icon for (vanity skins, prototypes, `T8_CAPE_ARENA_BANNER`, Elder's Black Hands) are dropped by
  `has_render_icon`; a test checks every catalog item has its PNG when `assets/icons/items` exists.
- `Item.icon` is the id of the render.albiononline.com icon; it equals `id` today, but always build icon paths from it, never
  from `id`. Items the renderer has no icon for are excluded rather than given someone else's icon (e.g.
  `T8_2H_IRONGAUNTLETS_HELL`, Elder's Black Hands, is not Hellfire Hands = `T8_2H_KNUCKLES_HELL`).
- Categories: `Catalog::categories(slot)`, `category_order` and `category_label` (FR, from the game's own strings)
  mirror the in-game market. Harvesting tools never enter the main-hand slot (`NOT_WEAPONS`); gatherer armour does.
- Food and potion families whose best tier is below T4 (`MIN_CONSUMABLE_TIER`) are dropped as noise.
- `catalog_matches_dumps` audits every T8 equippable of `items.json` against the catalog, so new exclusions must be listed there too.
- Icon files: items `T8_X.png`, enchanted `T8_X_e<n>.png`; spells `<uisprite>.png`, fetched at 64 px (a missing sprite falls back to the spell id).

## Conventions

- Match the surrounding style; comments explain why, not what.
- A change to what `catalog.rs` outputs needs a `SCHEMA` bump and `cargo test` (the test rewrites the cache).
- Do not `git init` or commit unless asked; `assets/icons` and the heavy dumps are ignored by design.
