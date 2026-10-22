# AO Comp Builder

Desktop editor (Rust, egui) for Albion Online builds and compositions. Pick weapon, armour,
mount and consumables, choose the Q/W/E spells and passives, then save builds to "Mes Builds"
or share them as a text string.

Scope: **tier 8 only**. Item quality is ignored; enchantments `.0`–`.4` are part of the build.

## Using a release

Download `AOCompBuilder-<version>-windows.zip` from the Releases page, unzip it anywhere and run
`AOCompBuilder.exe`. The zip carries the icons and data the app needs; builds and compositions are
saved in a `userdata/` folder next to the exe, so keep the exe and its `assets/` folder together.

## Setup from source

Requires Rust (edition 2024) and PowerShell 7 for the scripts. Run everything from the project root.

```powershell
./scripts/fetch_dumps.ps1     # items.json, spells.json, items.txt from ao-bin-dumps
cargo run                     # first run builds assets/data/catalog.json from the dumps
./scripts/fetch_icons.ps1     # item and spell icons into assets/icons (re-runnable)
```

`fetch_icons.ps1` skips icons already on disk, retries, and lists failures in `icons-failed.txt`.
Use `-DryRun` to see what is missing without downloading.

User data lives in `userdata/`.

## Data

| File | Origin |
| --- | --- |
| `assets/data/items.json`, `spells.json`, `items.txt` | [ao-bin-dumps](https://github.com/ao-data/ao-bin-dumps), via `fetch_dumps.ps1` |
| `assets/data/spell_loc.json` | EN/FR spell names and descriptions, distilled from ao-bin-dumps `localization.json` (checked in) |
| `assets/data/item_loc.json` | French item names of the catalog, distilled from the same file (checked in) |
| `assets/data/catalog.json` | Cache built from the above; rebuilt automatically when its schema version changes |

To refresh the localization:
`$env:AO_LOCALIZATION='path\to\localization.json'; cargo test distil_localization -- --ignored`
(and `distil_item_names` for item names)

## Development

```powershell
cargo test
cargo clippy --all-targets
```

See `CLAUDE.md` for the architecture and conventions.

Unofficial fan tool, not affiliated with Sandbox Interactive. Game data comes from ao-bin-dumps and
icons from the public render service at render.albiononline.com.
