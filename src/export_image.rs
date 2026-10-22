//! PNG export of a build or a composition, drawn off-screen into memory (never a screen capture).
//!
//! Equipment only: no spells or passives. Item icons come from render.albiononline.com at 217 px,
//! downloaded on demand into `userdata/cache/icons_hd`; offline, the 64 px icons of `assets/icons`
//! are used instead. Nothing here depends on egui, so it can run on a worker thread.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use anyhow::{Context, Result, bail};
use image::{Rgba, RgbaImage, imageops};

use crate::build::{Build, Equipped, write_atomic};
use crate::catalog::{Catalog, Slot};
use crate::comp::Composition;

/// Where the export reads and writes. Build it once from the app's root directory.
#[derive(Clone, Debug)]
pub struct Paths {
    /// `assets/icons`; the 64 px icons live in its `items` folder.
    pub icons: PathBuf,
    /// `userdata/cache/icons_hd`.
    pub cache: PathBuf,
    /// `userdata/exports`, where [`export_build`] and [`export_comp`] write.
    pub exports: PathBuf,
}

impl Paths {
    #[cfg(test)] // the app derives these from its own folders (see App::export_paths)
    pub fn new(root: &Path) -> Self {
        Self {
            icons: root.join("assets").join("icons"),
            cache: root.join("userdata").join("cache").join("icons_hd"),
            exports: root.join("userdata").join("exports"),
        }
    }
}

/// What the export is doing, for a progress indicator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Progress {
    /// `done` of `total` item icons fetched (from cache, network or the 64 px fallback).
    Icons {
        done: usize,
        total: usize,
    },
    Rendering,
    Saving,
}

/// An item icon: the renderer's id (`Item.icon`) and the enchant level (0–4).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct IconKey {
    pub icon: String,
    pub enchant: u8,
}

impl IconKey {
    /// File stem used both in the HD cache and in `assets/icons/items`: `T8_X` / `T8_X_e2`.
    fn stem(&self) -> String {
        if self.enchant > 0 {
            format!("{}_e{}", self.icon, self.enchant)
        } else {
            self.icon.clone()
        }
    }

    fn url(&self) -> String {
        let name = if self.enchant > 0 {
            format!("{}@{}", self.icon, self.enchant)
        } else {
            self.icon.clone()
        };
        format!("https://render.albiononline.com/v1/item/{name}.png?size=217")
    }
}

pub type Icons = HashMap<IconKey, RgbaImage>;

/// What the renderer needs to know about an item: copied out of the catalog so an export can run
/// on a worker thread without borrowing it.
#[derive(Clone, Debug)]
struct ItemInfo {
    icon: String,
    name: String,
    two_handed: bool,
}

/// The catalog entries used by some builds, keyed by item id.
#[derive(Clone, Default, Debug)]
pub struct Items(HashMap<String, ItemInfo>);

impl Items {
    /// Looks up the main items and swaps of `builds`; unknown ids are simply absent.
    pub fn for_builds<'a>(builds: impl IntoIterator<Item = &'a Build>, catalog: &Catalog) -> Self {
        let mut map = HashMap::new();
        for e in builds.into_iter().flat_map(|b| b.slots.values()) {
            for id in std::iter::once(&e.item).chain(e.swaps.iter().map(|s| &s.item)) {
                if let Some(item) = catalog.item(id) {
                    let name = if item.name_fr.is_empty() {
                        &item.name
                    } else {
                        &item.name_fr
                    };
                    map.entry(id.clone()).or_insert_with(|| ItemInfo {
                        icon: item.icon.clone(),
                        name: name.clone(),
                        two_handed: item.two_handed,
                    });
                }
            }
        }
        Self(map)
    }

    fn get(&self, id: &str) -> Option<&ItemInfo> {
        self.0.get(id)
    }
}

enum Subject {
    Build(Build),
    Comp(Composition, Vec<Build>),
}

/// Everything an export needs, owned: build it on the UI thread, then [`Job::run`] it anywhere.
pub struct Job {
    subject: Subject,
    items: Items,
    paths: Paths,
}

impl Job {
    pub fn build(build: &Build, catalog: &Catalog, paths: &Paths) -> Self {
        Self {
            items: Items::for_builds([build], catalog),
            subject: Subject::Build(build.clone()),
            paths: paths.clone(),
        }
    }

    /// `builds` is "Mes Builds"; only the ones the composition references are kept.
    pub fn comp(comp: &Composition, builds: &[Build], catalog: &Catalog, paths: &Paths) -> Self {
        let used: Vec<Build> = (0..comp.members.len())
            .filter_map(|i| comp.resolve(i, builds))
            .cloned()
            .collect();
        Self {
            items: Items::for_builds(&used, catalog),
            subject: Subject::Comp(comp.clone(), used),
            paths: paths.clone(),
        }
    }

    /// Name of the build or composition being exported.
    pub fn title(&self) -> &str {
        match &self.subject {
            Subject::Build(b) => &b.name,
            Subject::Comp(c, _) => &c.name,
        }
    }

    /// Where this export goes by default: `<exports>/<name>.png`.
    pub fn default_dest(&self) -> PathBuf {
        let (name, fallback) = match &self.subject {
            Subject::Build(b) => (&b.name, "build"),
            Subject::Comp(c, _) => (&c.name, "composition"),
        };
        self.paths
            .exports
            .join(format!("{}.png", file_stem(name, fallback)))
    }

    /// Fetches the icons, draws the image and writes it to `dest`.
    pub fn run(self, dest: &Path, progress: &mut dyn FnMut(Progress)) -> Result<()> {
        let keys = match &self.subject {
            Subject::Build(b) => build_keys(b, &self.items),
            Subject::Comp(_, builds) => builds
                .iter()
                .flat_map(|b| build_keys(b, &self.items))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        };
        let icons = load_icons(&keys, &self.paths, progress);
        progress(Progress::Rendering);
        let image = match &self.subject {
            Subject::Build(b) => render_build(b, &self.items, &icons)?,
            Subject::Comp(c, builds) => render_comp(c, builds, &self.items, &icons)?,
        };
        save(&image, dest, progress)
    }
}

fn save(image: &RgbaImage, dest: &Path, progress: &mut dyn FnMut(Progress)) -> Result<()> {
    progress(Progress::Saving);
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .context("encodage PNG")?;
    write_atomic(dest, &png, false).with_context(|| format!("écriture de {}", dest.display()))
}

/// A file name that is valid on Windows; falls back to `fallback` when nothing usable is left.
fn file_stem(name: &str, fallback: &str) -> String {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let cleaned: String = name
        .chars()
        .map(|c| {
            if r#"<>:"/\|?*"#.contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .take(100)
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.').trim_end();
    if cleaned.is_empty() {
        fallback.to_owned()
    } else if RESERVED.contains(&cleaned.to_ascii_uppercase().as_str()) {
        format!("_{cleaned}")
    } else {
        cleaned.to_owned()
    }
}

// ---- icons ---------------------------------------------------------------------------------

/// Icons needed by a build: its items and their swaps.
pub fn build_keys(build: &Build, items: &Items) -> Vec<IconKey> {
    let mut keys = BTreeSet::new();
    for equipped in build.slots.values() {
        let picks = std::iter::once((&equipped.item, equipped.enchant))
            .chain(equipped.swaps.iter().map(|s| (&s.item, s.enchant)));
        for (id, enchant) in picks {
            if let Some(item) = items.get(id) {
                keys.insert(IconKey {
                    icon: item.icon.clone(),
                    enchant,
                });
            }
        }
    }
    keys.into_iter().collect()
}

enum Fetch {
    /// The service has no such icon (or answered with an error status).
    Missing,
    /// No answer at all: stop trying for the rest of this export.
    Offline,
}

/// Cached HD icon, else a download (cached for next time), else the 64 px icon, else nothing.
fn load_icons(keys: &[IconKey], paths: &Paths, progress: &mut dyn FnMut(Progress)) -> Icons {
    load_icons_with(keys, paths, progress, true)
}

/// Total time spent downloading icons in one export (each request also has its own 12 s timeout).
const DOWNLOAD_BUDGET: Duration = Duration::from_secs(60);

/// `allow_network = false` serves only the cache and the 64 px icons (used by tests).
fn load_icons_with(
    keys: &[IconKey],
    paths: &Paths,
    progress: &mut dyn FnMut(Progress),
    allow_network: bool,
) -> Icons {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(12)))
        .build()
        .into();
    let started = Instant::now();
    let mut online = allow_network;
    let mut icons = Icons::new();
    for (done, key) in keys.iter().enumerate() {
        progress(Progress::Icons {
            done,
            total: keys.len(),
        });
        let cached = paths.cache.join(format!("{}.png", key.stem()));
        let mut icon = open_png(&cached);
        // A slow server must not hold the export for minutes: past the budget, use what is on disk.
        if icon.is_none() && online && started.elapsed() < DOWNLOAD_BUDGET {
            match download(&agent, key, &cached) {
                Ok(img) => icon = Some(img),
                Err(Fetch::Missing) => {}
                Err(Fetch::Offline) => online = false,
            }
        }
        let icon = icon.or_else(|| {
            open_png(
                &paths
                    .icons
                    .join("items")
                    .join(format!("{}.png", key.stem())),
            )
        });
        if let Some(icon) = icon {
            icons.insert(key.clone(), icon);
        }
    }
    progress(Progress::Icons {
        done: keys.len(),
        total: keys.len(),
    });
    icons
}

fn open_png(path: &Path) -> Option<RgbaImage> {
    image::open(path).ok().map(|i| i.to_rgba8())
}

fn download(agent: &ureq::Agent, key: &IconKey, cache_path: &Path) -> Result<RgbaImage, Fetch> {
    let mut response = agent.get(key.url()).call().map_err(|e| match e {
        ureq::Error::StatusCode(_) => Fetch::Missing,
        _ => Fetch::Offline,
    })?;
    let bytes = response
        .body_mut()
        .read_to_vec()
        .map_err(|_| Fetch::Offline)?;
    let img = image::load_from_memory(&bytes)
        .map_err(|_| Fetch::Missing)?
        .to_rgba8();
    // A failed cache write only costs a re-download next time.
    let _ = write_atomic(cache_path, &bytes, false);
    Ok(img)
}

// ---- drawing primitives -----------------------------------------------------------------------

const FONT: &[u8] = include_bytes!("../assets/fonts/Ubuntu-Light.ttf");

const BG_TOP: [u8; 3] = [27, 30, 39];
const BG_BOTTOM: [u8; 3] = [14, 15, 20];
const CARD: [u8; 3] = [38, 42, 54];
const CARD_BORDER: [u8; 3] = [64, 69, 86];
const TEXT: [u8; 3] = [236, 236, 240];
const DIM: [u8; 3] = [140, 146, 162];
const GOLD: [u8; 3] = [240, 180, 60];

fn enchant_color(enchant: u8) -> [u8; 3] {
    match enchant {
        1 => [88, 200, 120],
        2 => [80, 150, 230],
        3 => [170, 100, 230],
        _ => GOLD,
    }
}

fn font() -> FontRef<'static> {
    FontRef::try_from_slice(FONT).expect("embedded font is valid")
}

fn background(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |_, y| {
        let t = y as f32 / h.max(1) as f32;
        let c = |i: usize| (BG_TOP[i] as f32 + (BG_BOTTOM[i] as f32 - BG_TOP[i] as f32) * t) as u8;
        Rgba([c(0), c(1), c(2), 255])
    })
}

fn blend(img: &mut RgbaImage, x: i64, y: i64, color: [u8; 3], alpha: f32) {
    if x < 0 || y < 0 || x >= img.width() as i64 || y >= img.height() as i64 || alpha <= 0.0 {
        return;
    }
    let a = alpha.min(1.0);
    let px = img.get_pixel_mut(x as u32, y as u32);
    for (channel, &src) in px.0.iter_mut().zip(color.iter()) {
        *channel = (src as f32 * a + *channel as f32 * (1.0 - a)).round() as u8;
    }
    px.0[3] = 255;
}

/// Filled rectangle with anti-aliased rounded corners.
fn round_rect(img: &mut RgbaImage, x: i64, y: i64, w: i64, h: i64, r: i64, color: [u8; 3]) {
    let r = r.min(w / 2).min(h / 2).max(0);
    for py in y..y + h {
        for px in x..x + w {
            let dx = (x + r - 1 - px).max(px - (x + w - r)).max(0) as f32;
            let dy = (y + r - 1 - py).max(py - (y + h - r)).max(0) as f32;
            let coverage = if dx > 0.0 && dy > 0.0 {
                (r as f32 - ((dx - 0.5).powi(2) + (dy - 0.5).powi(2)).sqrt() + 0.5).clamp(0.0, 1.0)
            } else {
                1.0
            };
            blend(img, px, py, color, coverage);
        }
    }
}

/// Card: a border-colored round rect with a slightly smaller fill on top.
fn card(img: &mut RgbaImage, x: i64, y: i64, w: i64, h: i64, fill: [u8; 3]) {
    round_rect(img, x, y, w, h, 16, CARD_BORDER);
    round_rect(img, x + 2, y + 2, w - 4, h - 4, 14, fill);
}

fn text_width(font: &FontRef, px: f32, text: &str) -> f32 {
    let scaled = font.as_scaled(PxScale::from(px));
    let mut width = 0.0;
    let mut prev = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(p) = prev {
            width += scaled.kern(p, id);
        }
        width += scaled.h_advance(id);
        prev = Some(id);
    }
    width
}

/// Draws `text` with its top-left at (x, y); `bold` thickens it by drawing it twice.
#[allow(clippy::too_many_arguments)] // canvas, font, size, position, colour and style are all independent
fn draw_text(
    img: &mut RgbaImage,
    font: &FontRef,
    px: f32,
    x: f32,
    y: f32,
    color: [u8; 3],
    text: &str,
    bold: bool,
) {
    let scale = PxScale::from(px);
    let scaled = font.as_scaled(scale);
    let baseline = y + scaled.ascent();
    for dx in if bold { vec![0.0, 0.8] } else { vec![0.0] } {
        let mut cursor = x + dx;
        let mut prev = None;
        for c in text.chars() {
            let id = scaled.glyph_id(c);
            if let Some(p) = prev {
                cursor += scaled.kern(p, id);
            }
            let glyph = id.with_scale_and_position(scale, ab_glyph::point(cursor, baseline));
            cursor += scaled.h_advance(id);
            prev = Some(id);
            if let Some(outlined) = font.outline_glyph(glyph) {
                let b = outlined.px_bounds();
                outlined.draw(|gx, gy, cov| {
                    blend(
                        img,
                        b.min.x as i64 + gx as i64,
                        b.min.y as i64 + gy as i64,
                        color,
                        cov,
                    )
                });
            }
        }
    }
}

#[allow(clippy::too_many_arguments)] // see draw_text
fn draw_centered(
    img: &mut RgbaImage,
    font: &FontRef,
    px: f32,
    cx: f32,
    y: f32,
    color: [u8; 3],
    text: &str,
    bold: bool,
) {
    let w = text_width(font, px, text);
    draw_text(img, font, px, cx - w / 2.0, y, color, text, bold);
}

/// Cuts `text` to fit `max_w`, ending with an ellipsis.
fn ellipsize(font: &FontRef, px: f32, text: &str, max_w: f32) -> String {
    if text_width(font, px, text) <= max_w {
        return text.to_owned();
    }
    let mut out: String = text.to_owned();
    while !out.is_empty() && text_width(font, px, &format!("{out}…")) > max_w {
        out.pop();
    }
    format!("{}…", out.trim_end())
}

/// Word-wraps `text` into at most `max_lines` lines of `max_w` pixels; the last line is ellipsized.
fn wrap(font: &FontRef, px: f32, text: &str, max_w: f32, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if text_width(font, px, &candidate) <= max_w || line.is_empty() {
                line = candidate;
            } else {
                lines.push(std::mem::take(&mut line));
                line = word.to_owned();
            }
        }
        lines.push(line);
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        let last = lines.pop().unwrap_or_default();
        lines.push(ellipsize(font, px, &format!("{last} …"), max_w));
    } else if let Some(last) = lines.last_mut() {
        *last = ellipsize(font, px, last, max_w);
    }
    lines.retain(|l| !l.is_empty());
    lines
}

/// Draws `icon` scaled to a `size` square at (x, y), or a placeholder when the icon is missing.
fn draw_icon(
    img: &mut RgbaImage,
    font: &FontRef,
    icon: Option<&RgbaImage>,
    x: i64,
    y: i64,
    size: u32,
) {
    match icon {
        Some(icon) => {
            let scaled;
            let icon = if icon.dimensions() == (size, size) {
                icon
            } else {
                scaled = imageops::resize(icon, size, size, imageops::FilterType::Lanczos3);
                &scaled
            };
            imageops::overlay(img, icon, x, y);
        }
        None => {
            round_rect(img, x, y, size as i64, size as i64, 10, [52, 56, 70]);
            draw_centered(
                img,
                font,
                size as f32 * 0.5,
                x as f32 + size as f32 / 2.0,
                y as f32 + size as f32 * 0.22,
                DIM,
                "?",
                false,
            );
        }
    }
}

/// `.3` pill at the bottom-right corner of an icon; nothing for enchant 0.
fn draw_badge(img: &mut RgbaImage, font: &FontRef, enchant: u8, right: i64, bottom: i64, px: f32) {
    if enchant == 0 {
        return;
    }
    let label = format!(".{enchant}");
    let w = (text_width(font, px, &label) + px * 0.7).round() as i64;
    let h = (px * 1.3).round() as i64;
    round_rect(img, right - w, bottom - h, w, h, h / 2, [18, 19, 26]);
    round_rect(
        img,
        right - w + 2,
        bottom - h + 2,
        w - 4,
        h - 4,
        h / 2 - 2,
        enchant_color(enchant),
    );
    draw_text(
        img,
        font,
        px,
        (right - w) as f32 + px * 0.35,
        (bottom - h) as f32 + px * 0.15,
        [18, 19, 26],
        &label,
        true,
    );
}

fn icon_for<'a>(
    icons: &'a Icons,
    items: &Items,
    id: &str,
    enchant: u8,
) -> (Option<&'a RgbaImage>, String) {
    match items.get(id) {
        Some(item) => (
            icons.get(&IconKey {
                icon: item.icon.clone(),
                enchant,
            }),
            item.name.clone(),
        ),
        None => (None, id.to_owned()),
    }
}

// ---- build image ------------------------------------------------------------------------------

/// Largest composition image drawn (about 240 MB in memory).
const MAX_PIXELS: i64 = 60_000_000;

const MARGIN: i64 = 60;
const GAP: i64 = 30;

/// Renders the build as one large image (2400 px wide) from already-loaded icons.
pub fn render_build(build: &Build, items: &Items, icons: &Icons) -> Result<RgbaImage> {
    const W: i64 = 2400;
    const COLS: i64 = 5;
    const ICON: i64 = 217;
    const SWAP: i64 = 72;
    const SWAP_GAP: i64 = 8;
    const PAD: i64 = 16;
    let font = font();

    let card_w = (W - 2 * MARGIN - (COLS - 1) * GAP) / COLS;
    let per_row = (card_w - 2 * PAD + SWAP_GAP) / (SWAP + SWAP_GAP);
    let max_swaps = build
        .slots
        .values()
        .map(|e| e.swaps.len() as i64)
        .max()
        .unwrap_or(0);
    let swap_rows = ((max_swaps + per_row - 1) / per_row).min(2);
    let swap_area = if swap_rows > 0 {
        12 + swap_rows * SWAP + (swap_rows - 1) * SWAP_GAP
    } else {
        0
    };
    let card_h = PAD + 36 + 8 + ICON + 10 + 72 + swap_area + PAD;

    let title = if build.name.trim().is_empty() {
        "Build sans nom"
    } else {
        build.name.trim()
    };
    let note_lines = wrap(&font, 32.0, &build.note, (W - 2 * MARGIN) as f32, 3);
    let header_h = MARGIN
        + 100
        + if note_lines.is_empty() {
            0
        } else {
            note_lines.len() as i64 * 42 + 12
        }
        + 30;
    let footer_h = 56;
    let height = header_h + 2 * card_h + GAP + footer_h;

    let mut img = background(W as u32, height as u32);
    draw_text(
        &mut img,
        &font,
        84.0,
        MARGIN as f32,
        MARGIN as f32 - 10.0,
        TEXT,
        &ellipsize(&font, 84.0, title, (W - 2 * MARGIN) as f32),
        true,
    );
    round_rect(&mut img, MARGIN, MARGIN + 92, 220, 5, 2, GOLD);
    for (i, line) in note_lines.iter().enumerate() {
        draw_text(
            &mut img,
            &font,
            32.0,
            MARGIN as f32,
            (MARGIN + 112) as f32 + i as f32 * 42.0,
            DIM,
            line,
            false,
        );
    }

    let two_handed = build
        .get(Slot::MainHand)
        .and_then(|e| items.get(&e.item))
        .is_some_and(|i| i.two_handed);

    for (n, slot) in Slot::ALL.into_iter().enumerate() {
        let (col, row) = (n as i64 % COLS, n as i64 / COLS);
        let x = MARGIN + col * (card_w + GAP);
        let y = header_h + row * (card_h + GAP);
        let equipped = build.get(slot);
        card(
            &mut img,
            x,
            y,
            card_w,
            card_h,
            if equipped.is_some() {
                CARD
            } else {
                [30, 33, 42]
            },
        );
        draw_text(
            &mut img,
            &font,
            28.0,
            (x + PAD) as f32,
            (y + PAD) as f32,
            GOLD,
            slot.label(),
            true,
        );

        let icon_x = x + (card_w - ICON) / 2;
        let icon_y = y + PAD + 36 + 8;
        let Some(e) = equipped else {
            let note = if slot == Slot::OffHand && two_handed {
                "Arme à deux mains"
            } else {
                "—"
            };
            draw_centered(
                &mut img,
                &font,
                30.0,
                (x + card_w / 2) as f32,
                (icon_y + ICON / 2 - 18) as f32,
                DIM,
                note,
                false,
            );
            continue;
        };

        let (icon, name) = icon_for(icons, items, &e.item, e.enchant);
        draw_icon(&mut img, &font, icon, icon_x, icon_y, ICON as u32);
        draw_badge(
            &mut img,
            &font,
            e.enchant,
            icon_x + ICON - 4,
            icon_y + ICON - 4,
            34.0,
        );

        let name_y = icon_y + ICON + 10;
        for (i, line) in wrap(&font, 30.0, &name, (card_w - 2 * PAD) as f32, 2)
            .iter()
            .enumerate()
        {
            draw_centered(
                &mut img,
                &font,
                30.0,
                (x + card_w / 2) as f32,
                (name_y + i as i64 * 36) as f32,
                TEXT,
                line,
                false,
            );
        }
        draw_swaps(
            &mut img,
            &font,
            items,
            icons,
            e,
            x + PAD,
            name_y + 72 + 12,
            (per_row * swap_rows) as usize,
            per_row,
            SWAP,
        );
    }

    let footer = "AO Comp Builder";
    draw_text(
        &mut img,
        &font,
        24.0,
        (W - MARGIN) as f32 - text_width(&font, 24.0, footer),
        (height - footer_h + 14) as f32,
        DIM,
        footer,
        false,
    );
    Ok(img)
}

/// Small swap icons in rows of `per_row`; when they do not all fit the last cell reads "+N".
#[allow(clippy::too_many_arguments)]
fn draw_swaps(
    img: &mut RgbaImage,
    font: &FontRef,
    items: &Items,
    icons: &Icons,
    equipped: &Equipped,
    x: i64,
    y: i64,
    capacity: usize,
    per_row: i64,
    size: i64,
) {
    let total = equipped.swaps.len();
    if total == 0 || capacity == 0 {
        return;
    }
    let shown = if total > capacity {
        capacity - 1
    } else {
        total
    };
    for (i, swap) in equipped.swaps.iter().take(shown).enumerate() {
        let (cx, cy) = (
            x + (i as i64 % per_row) * (size + 8),
            y + (i as i64 / per_row) * (size + 8),
        );
        let (icon, _) = icon_for(icons, items, &swap.item, swap.enchant);
        round_rect(img, cx, cy, size, size, 10, [28, 31, 40]);
        draw_icon(img, font, icon, cx, cy, size as u32);
        draw_badge(img, font, swap.enchant, cx + size, cy + size, 20.0);
    }
    if total > shown {
        let i = shown as i64;
        let (cx, cy) = (
            x + (i % per_row) * (size + 8),
            y + (i / per_row) * (size + 8),
        );
        round_rect(img, cx, cy, size, size, 10, [28, 31, 40]);
        draw_centered(
            img,
            font,
            30.0,
            (cx + size / 2) as f32,
            (cy + size / 2 - 18) as f32,
            DIM,
            &format!("+{}", total - shown),
            true,
        );
    }
}

// ---- composition image ---------------------------------------------------------------------------

/// Renders a composition: one column per member (role, player, build name, equipment), wrapped
/// into rows of at most six columns.
pub fn render_comp(
    comp: &Composition,
    builds: &[Build],
    items: &Items,
    icons: &Icons,
) -> Result<RgbaImage> {
    const COL_W: i64 = 330;
    const MAX_COLS: i64 = 6;
    const TILE: i64 = 120;
    const TILE_GAP: i64 = 12;
    const PAD: i64 = 16;
    let font = font();

    let members = comp.members.len() as i64;
    let cols = members.clamp(1, MAX_COLS);
    let rows = ((members + MAX_COLS - 1) / MAX_COLS).max(1);
    let width = (2 * MARGIN + cols * COL_W + (cols - 1) * GAP).max(1400);

    let title = if comp.name.trim().is_empty() {
        "Composition sans nom"
    } else {
        comp.name.trim()
    };
    let note_lines = wrap(&font, 30.0, &comp.notes, (width - 2 * MARGIN) as f32, 4);
    let header_h = MARGIN
        + 100
        + if note_lines.is_empty() {
            0
        } else {
            note_lines.len() as i64 * 40 + 12
        }
        + 30;
    let member_notes = comp.members.iter().any(|m| !m.note.trim().is_empty());
    let col_head = PAD + 44 + 36 + 32 + if member_notes { 60 } else { 0 } + 12;
    let col_h = col_head + 5 * (TILE + TILE_GAP) + PAD - TILE_GAP;
    let footer_h = 56;
    let height = header_h + rows * col_h + (rows - 1) * GAP + footer_h;
    // Memory is width x height x 4 bytes: refuse absurd compositions (imported strings can hold thousands of members).
    if width * height > MAX_PIXELS {
        bail!(
            "composition trop grande pour une image ({members} membres) : retire des membres ou exporte-la en deux fois"
        );
    }

    let mut img = background(width as u32, height as u32);
    draw_text(
        &mut img,
        &font,
        84.0,
        MARGIN as f32,
        MARGIN as f32 - 10.0,
        TEXT,
        &ellipsize(&font, 84.0, title, (width - 2 * MARGIN) as f32),
        true,
    );
    round_rect(&mut img, MARGIN, MARGIN + 92, 220, 5, 2, GOLD);
    for (i, line) in note_lines.iter().enumerate() {
        draw_text(
            &mut img,
            &font,
            30.0,
            MARGIN as f32,
            (MARGIN + 112) as f32 + i as f32 * 40.0,
            DIM,
            line,
            false,
        );
    }
    if comp.members.is_empty() {
        draw_text(
            &mut img,
            &font,
            36.0,
            MARGIN as f32,
            header_h as f32,
            DIM,
            "Aucun membre",
            false,
        );
    }

    for (n, member) in comp.members.iter().enumerate() {
        let (col, row) = (n as i64 % MAX_COLS, n as i64 / MAX_COLS);
        let x = MARGIN + col * (COL_W + GAP);
        let y = header_h + row * (col_h + GAP);
        let inner = (COL_W - 2 * PAD) as f32;
        let cx = (x + COL_W / 2) as f32;
        let build = comp.resolve(n, builds);
        card(&mut img, x, y, COL_W, col_h, CARD);

        let role = if member.role.trim().is_empty() {
            "Rôle"
        } else {
            member.role.trim()
        };
        draw_centered(
            &mut img,
            &font,
            40.0,
            cx,
            (y + PAD) as f32,
            GOLD,
            &ellipsize(&font, 40.0, role, inner),
            true,
        );
        let player = if member.player.trim().is_empty() {
            "—"
        } else {
            member.player.trim()
        };
        draw_centered(
            &mut img,
            &font,
            30.0,
            cx,
            (y + PAD + 44) as f32,
            TEXT,
            &ellipsize(&font, 30.0, player, inner),
            false,
        );
        let (build_label, build_color) = match (&member.build, build) {
            (_, Some(b)) => (b.name.clone(), DIM),
            (Some(name), None) => (format!("{name} (introuvable)"), [210, 110, 110]),
            (None, None) => ("Aucun build".to_owned(), DIM),
        };
        draw_centered(
            &mut img,
            &font,
            26.0,
            cx,
            (y + PAD + 80) as f32,
            build_color,
            &ellipsize(&font, 26.0, &build_label, inner),
            false,
        );
        for (i, line) in wrap(&font, 24.0, &member.note, inner, 2).iter().enumerate() {
            draw_centered(
                &mut img,
                &font,
                24.0,
                cx,
                (y + PAD + 114 + i as i64 * 28) as f32,
                DIM,
                line,
                false,
            );
        }

        let grid_x = x + (COL_W - (2 * TILE + TILE_GAP)) / 2;
        for (i, slot) in Slot::ALL.into_iter().enumerate() {
            let tx = grid_x + (i as i64 % 2) * (TILE + TILE_GAP);
            let ty = y + col_head + (i as i64 / 2) * (TILE + TILE_GAP);
            match build.and_then(|b| b.get(slot)) {
                Some(e) => {
                    let (icon, _) = icon_for(icons, items, &e.item, e.enchant);
                    draw_icon(&mut img, &font, icon, tx, ty, TILE as u32);
                    draw_badge(
                        &mut img,
                        &font,
                        e.enchant,
                        tx + TILE - 2,
                        ty + TILE - 2,
                        24.0,
                    );
                }
                None => round_rect(&mut img, tx, ty, TILE, TILE, 12, [30, 33, 42]),
            }
        }
    }

    let footer = "AO Comp Builder";
    draw_text(
        &mut img,
        &font,
        24.0,
        (width - MARGIN) as f32 - text_width(&font, 24.0, footer),
        (height - footer_h + 14) as f32,
        DIM,
        footer,
        false,
    );
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::Swap;
    use crate::comp::Member;

    fn catalog() -> Catalog {
        Catalog::load(Path::new("assets/data")).expect("catalog")
    }

    /// Solid-color stand-ins for every icon a build needs, so no file or network is involved.
    fn its(build: &Build, catalog: &Catalog) -> Items {
        Items::for_builds([build], catalog)
    }

    fn fake_icons(build: &Build, catalog: &Catalog) -> Icons {
        build_keys(build, &its(build, catalog))
            .into_iter()
            .map(|k| {
                let shade = (k.icon.len() * 13 % 150) as u8 + 60;
                (
                    k,
                    RgbaImage::from_pixel(217, 217, Rgba([shade, 120, 200, 255])),
                )
            })
            .collect()
    }

    fn first_item(catalog: &Catalog, slot: Slot) -> &crate::catalog::Item {
        catalog
            .items
            .iter()
            .find(|i| i.slot == slot && !i.two_handed)
            .expect("an item")
    }

    fn sample(catalog: &Catalog, swaps: usize) -> Build {
        let mut b = Build {
            name: "Frappeur de test".into(),
            note: "Ouvrir avec Q puis E, garder la mobilité pour la fin du combat.".into(),
            ..Default::default()
        };
        for slot in Slot::ALL {
            let mut e = Equipped::from_item(first_item(catalog, slot), 2);
            if slot == Slot::Cape {
                for _ in 0..swaps {
                    e.add_swap(Swap::from_item(first_item(catalog, slot), 1));
                }
            }
            b.set(slot, e);
        }
        b
    }

    fn is_drawn(img: &RgbaImage) -> bool {
        let first = *img.get_pixel(img.width() / 2, 2);
        img.pixels().step_by(997).any(|p| p.0[..3] != first.0[..3])
    }

    #[test]
    fn build_image_has_expected_size_and_content() {
        let c = catalog();
        let b = sample(&c, 0);
        let img = render_build(&b, &its(&b, &c), &fake_icons(&b, &c)).unwrap();
        assert_eq!(img.width(), 2400);
        assert!((1000..2000).contains(&img.height()), "{}", img.height());
        assert!(is_drawn(&img));
        // The first card holds the main-hand icon: its center must not be card or background colored.
        let px = img.get_pixel(60 + 432 / 2, 60 + 100 + 42 + 30 + 16 + 36 + 8 + 100);
        assert!(px.0[2] > 150, "icon not drawn: {px:?}");
    }

    #[test]
    fn swaps_make_the_image_taller_and_overflow_is_capped() {
        let c = catalog();
        let plain = sample(&c, 0);
        let few = sample(&c, 3);
        let many = sample(&c, 40);
        let h = |b: &Build| {
            render_build(b, &its(b, &c), &fake_icons(b, &c))
                .unwrap()
                .height()
        };
        assert!(h(&few) > h(&plain));
        assert!(h(&many) > h(&few));
        assert_eq!(h(&many), h(&sample(&c, 11)), "at most two swap rows");
    }

    #[test]
    fn missing_icons_and_unknown_items_still_render() {
        let c = catalog();
        let mut b = sample(&c, 1);
        b.set(
            Slot::Head,
            Equipped {
                item: "NOPE".into(),
                ..Default::default()
            },
        );
        let img = render_build(&b, &its(&b, &c), &Icons::new()).unwrap();
        assert_eq!(img.width(), 2400);
        assert!(is_drawn(&img));
    }

    #[test]
    fn comp_image_scales_with_member_count() {
        let c = catalog();
        let tank = sample(&c, 0);
        let member = |role: &str, build: Option<&str>| Member {
            role: role.into(),
            build: build.map(Into::into),
            player: "Joueur".into(),
            note: String::new(),
        };
        let comp = |n: usize| Composition {
            name: "ZvZ".into(),
            notes: "Rendez-vous 20h".into(),
            members: (0..n)
                .map(|i| member(&format!("Rôle {i}"), Some(&tank.name)))
                .collect(),
        };
        let builds = [tank.clone()];
        let icons = fake_icons(&tank, &c);
        let items = Items::for_builds(&builds, &c);
        let render = |n| render_comp(&comp(n), &builds, &items, &icons).unwrap();
        let (one, six, seven) = (render(1), render(6), render(7));
        assert_eq!(one.width(), 1400);
        assert_eq!(six.width(), 2 * 60 + 6 * 330 + 5 * 30);
        assert_eq!(seven.width(), six.width());
        assert!(seven.height() > six.height(), "7th member starts a new row");
        assert!(is_drawn(&six));

        let empty = render_comp(&comp(0), &builds, &items, &icons).unwrap();
        assert!(is_drawn(&empty));
        // A member pointing at a deleted build renders without panicking.
        let broken = Composition {
            members: vec![member("Heal", Some("Perdu")), member("Scout", None)],
            ..comp(0)
        };
        assert!(is_drawn(
            &render_comp(&broken, &builds, &items, &icons).unwrap()
        ));
    }

    #[test]
    fn text_helpers_fit_their_width() {
        let f = font();
        let wrapped = wrap(
            &f,
            30.0,
            "un texte assez long pour passer sur plusieurs lignes dans une petite largeur",
            200.0,
            2,
        );
        assert_eq!(wrapped.len(), 2);
        assert!(
            wrapped.iter().all(|l| text_width(&f, 30.0, l) <= 200.0),
            "{wrapped:?}"
        );
        assert!(wrapped[1].ends_with('…'));
        assert!(ellipsize(&f, 30.0, "Épée à deux mains de l'ancien", 150.0).ends_with('…'));
        assert_eq!(ellipsize(&f, 30.0, "court", 150.0), "court");
        assert!(wrap(&f, 30.0, "", 200.0, 3).is_empty());
    }

    #[test]
    fn file_stem_is_windows_safe() {
        assert_eq!(file_stem("Tank: A/B?", "x"), "Tank_ A_B_");
        assert_eq!(file_stem("  ... ", "build"), "build");
    }

    #[test]
    fn file_stem_avoids_reserved_names_and_long_names() {
        assert_eq!(file_stem("con", "x"), "_con");
        assert_eq!(file_stem("Lpt1", "x"), "_Lpt1");
        assert_eq!(file_stem("console", "x"), "console");
        assert_eq!(file_stem(&"é".repeat(300), "x").chars().count(), 100);
    }

    #[test]
    fn oversized_compositions_are_refused_not_allocated() {
        let member = Member {
            role: "R".into(),
            ..Default::default()
        };
        let comp = Composition {
            name: "Énorme".into(),
            members: vec![member; 5000],
            ..Default::default()
        };
        let err = render_comp(&comp, &[], &Items::default(), &Icons::new()).unwrap_err();
        assert!(err.to_string().contains("trop grande"), "{err}");
    }
    #[test]
    fn icons_load_from_cache_then_local_fallback_without_network() {
        let dir = std::env::temp_dir().join(format!("aocb-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = Paths::new(&dir);
        std::fs::create_dir_all(&paths.cache).unwrap();
        std::fs::create_dir_all(paths.icons.join("items")).unwrap();
        RgbaImage::from_pixel(217, 217, Rgba([1, 2, 3, 255]))
            .save(paths.cache.join("T8_A_e2.png"))
            .unwrap();
        RgbaImage::from_pixel(64, 64, Rgba([9, 9, 9, 255]))
            .save(paths.icons.join("items").join("T8_B.png"))
            .unwrap();

        // Both icons are on disk, and the network is off anyway.
        let keys = [
            IconKey {
                icon: "T8_A".into(),
                enchant: 2,
            },
            IconKey {
                icon: "T8_B".into(),
                enchant: 0,
            },
        ];
        let mut steps = Vec::new();
        let icons = load_icons_with(&keys, &paths, &mut |p| steps.push(p), false);
        assert_eq!(icons[&keys[0]].dimensions(), (217, 217));
        assert_eq!(icons[&keys[1]].dimensions(), (64, 64));
        assert_eq!(steps.last(), Some(&Progress::Icons { done: 2, total: 2 }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_writes_a_valid_png() {
        let c = catalog();
        let b = sample(&c, 2);
        let dir = std::env::temp_dir().join(format!("aocb-export-png-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("out.png");
        let img = render_build(&b, &its(&b, &c), &fake_icons(&b, &c)).unwrap();
        save(&img, &dest, &mut |_| {}).unwrap();
        let back = image::open(&dest).unwrap();
        assert_eq!((back.width(), back.height()), (img.width(), img.height()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Writes a sample build and composition (local icons only) to `$AOCB_SAMPLE_DIR` for a visual check:
    /// `cargo test sample_images -- --ignored`.
    #[test]
    #[ignore]
    fn sample_images() {
        let Some(dir) = std::env::var_os("AOCB_SAMPLE_DIR").map(PathBuf::from) else {
            return;
        };
        let c = catalog();
        let b = sample(&c, 3);
        let paths = Paths::new(Path::new("."));
        let icons = load_icons_with(&build_keys(&b, &its(&b, &c)), &paths, &mut |_| {}, false);
        render_build(&b, &its(&b, &c), &icons)
            .unwrap()
            .save(dir.join("build.png"))
            .unwrap();
        let member = |role: &str| Member {
            role: role.into(),
            build: Some(b.name.clone()),
            player: "Joueur".into(),
            note: "note".into(),
        };
        let comp = Composition {
            name: "ZvZ".into(),
            notes: "Rendez-vous 20h".into(),
            members: ["Tank", "Heal", "DPS", "Scout"].map(member).to_vec(),
        };
        render_comp(&comp, std::slice::from_ref(&b), &its(&b, &c), &icons)
            .unwrap()
            .save(dir.join("comp.png"))
            .unwrap();
    }
}
