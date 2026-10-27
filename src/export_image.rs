//! PNG export of a build or a composition, drawn off-screen into memory (never a screen capture).
//!
//! Equipment and swaps only: no spells or passives. Item icons come from render.albiononline.com at
//! 217 px, downloaded on demand into `userdata/cache/icons_hd`; offline, the 64 px icons of
//! `assets/icons` are used instead. Nothing here depends on egui, so it can run on a worker thread.

use std::borrow::Cow;
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

// Warm greys with gold accents, like the app itself (`ui/theme.rs`).
const BG_TOP: [u8; 3] = [30, 26, 23];
const BG_BOTTOM: [u8; 3] = [12, 11, 10];
/// Copper haze behind the title.
const GLOW: [u8; 3] = [184, 115, 51];
const CARD: [u8; 3] = [36, 32, 28];
const CARD_EMPTY: [u8; 3] = [26, 23, 20];
const CARD_BORDER: [u8; 3] = [70, 60, 48];
/// The inventory-slot square every icon sits in.
const WELL: [u8; 3] = [17, 15, 13];
const WELL_BORDER: [u8; 3] = [56, 48, 40];
const PILL: [u8; 3] = [44, 39, 33];
const TEXT: [u8; 3] = [235, 227, 212];
const DIM: [u8; 3] = [158, 147, 131];
/// Names of empty slots.
const FAINT: [u8; 3] = [98, 89, 78];
const GOLD: [u8; 3] = [232, 184, 86];
const DANGER: [u8; 3] = [222, 92, 78];
/// Text and outlines on coloured fills.
const INK: [u8; 3] = [24, 18, 12];

/// In-game enchantment colours, as in the app.
fn enchant_color(enchant: u8) -> [u8; 3] {
    match enchant {
        1 => [88, 168, 84],
        2 => [70, 140, 214],
        3 => [160, 96, 210],
        _ => [222, 170, 52],
    }
}

/// Accent of a role, guessed from its name; gold for roles it does not recognise.
fn role_color(role: &str) -> [u8; 3] {
    let role = role.to_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| role.contains(w));
    if any(&["heal", "soin", "soign"]) {
        [116, 192, 104]
    } else if any(&["tank"]) {
        [86, 150, 222]
    } else if any(&["supp", "util"]) {
        [176, 124, 224]
    } else if any(&["dps", "dmg", "damage", "dégât", "degat"]) {
        [226, 100, 82]
    } else {
        GOLD
    }
}

fn font() -> FontRef<'static> {
    FontRef::try_from_slice(FONT).expect("embedded font is valid")
}

fn lerp(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8)
}

fn lighten(color: [u8; 3], by: u8) -> [u8; 3] {
    color.map(|c| c.saturating_add(by))
}

/// Vertical gradient with a copper haze in the top-left corner, behind the title.
fn background(w: u32, h: u32) -> RgbaImage {
    let (glow_x, glow_y) = (w as f32 * 0.15, -(h as f32) * 0.1);
    let reach = w.max(h) as f32 * 0.75;
    RgbaImage::from_fn(w, h, |x, y| {
        let base = lerp(BG_TOP, BG_BOTTOM, y as f32 / h.max(1) as f32);
        let d = (x as f32 - glow_x).hypot(y as f32 - glow_y);
        let haze = (1.0 - d / reach).clamp(0.0, 1.0).powi(2) * 0.18;
        let [r, g, b] = lerp(base, GLOW, haze);
        Rgba([r, g, b, 255])
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

/// Rectangle `(x, y, w, h)` with anti-aliased rounded corners, shaded from `top` to `bottom` and
/// laid at `alpha` opacity. `square_bottom` keeps the bottom corners square (a band across the top
/// of a card).
fn fill(
    img: &mut RgbaImage,
    (x, y, w, h): (i64, i64, i64, i64),
    r: i64,
    top: [u8; 3],
    bottom: [u8; 3],
    alpha: f32,
    square_bottom: bool,
) {
    let r = r.min(w / 2).min(h / 2).max(0);
    for py in y..y + h {
        let color = lerp(top, bottom, (py - y) as f32 / (h - 1).max(1) as f32);
        let below = if square_bottom { 0 } else { py - (y + h - r) };
        let dy = (y + r - 1 - py).max(below).max(0) as f32;
        for px in x..x + w {
            let dx = (x + r - 1 - px).max(px - (x + w - r)).max(0) as f32;
            let coverage = if dx > 0.0 && dy > 0.0 {
                (r as f32 - ((dx - 0.5).powi(2) + (dy - 0.5).powi(2)).sqrt() + 0.5).clamp(0.0, 1.0)
            } else {
                1.0
            };
            blend(img, px, py, color, coverage * alpha);
        }
    }
}

/// Filled rectangle with anti-aliased rounded corners.
fn round_rect(img: &mut RgbaImage, x: i64, y: i64, w: i64, h: i64, r: i64, color: [u8; 3]) {
    fill(img, (x, y, w, h), r, color, color, 1.0, false);
}

/// Soft shadow cast downwards by a rounded rectangle; draw it before the shape itself.
fn shadow(img: &mut RgbaImage, x: i64, y: i64, w: i64, h: i64, r: i64) {
    const BLUR: i64 = 24;
    const DROP: i64 = 10;
    const STRENGTH: f32 = 0.5;
    let (cx, cy) = (
        x as f32 + w as f32 / 2.0,
        (y + DROP) as f32 + h as f32 / 2.0,
    );
    let (half_w, half_h) = (w as f32 / 2.0 - r as f32, h as f32 / 2.0 - r as f32);
    for py in y + DROP - BLUR..y + DROP + h + BLUR {
        for px in x - BLUR..x + w + BLUR {
            // Signed distance to the rounded rectangle, negative inside.
            let qx = (px as f32 + 0.5 - cx).abs() - half_w;
            let qy = (py as f32 + 0.5 - cy).abs() - half_h;
            let d = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r as f32;
            let t = 1.0 - (d / BLUR as f32).clamp(0.0, 1.0);
            blend(img, px, py, [0, 0, 0], STRENGTH * t * t);
        }
    }
}

/// Card: soft shadow, a border-coloured round rect, and a fill a little lighter at the top.
fn card(img: &mut RgbaImage, x: i64, y: i64, w: i64, h: i64, color: [u8; 3]) {
    shadow(img, x, y, w, h, 16);
    round_rect(img, x, y, w, h, 16, CARD_BORDER);
    fill(
        img,
        (x + 2, y + 2, w - 4, h - 4),
        14,
        lighten(color, 10),
        color,
        1.0,
        false,
    );
}

/// Dark inventory-slot square behind an icon, a little deeper at the top.
fn well(img: &mut RgbaImage, x: i64, y: i64, size: i64, r: i64) {
    round_rect(img, x, y, size, size, r, WELL_BORDER);
    fill(
        img,
        (x + 2, y + 2, size - 4, size - 4),
        r - 2,
        WELL,
        lighten(WELL, 10),
        1.0,
        false,
    );
}

const PILL_H: i64 = 40;
const PILL_PX: f32 = 24.0;

fn pill_width(font: &FontRef, text: &str, dot: bool) -> i64 {
    let lead = if dot { 40 } else { 18 };
    lead + text_width(font, PILL_PX, text).ceil() as i64 + 18
}

/// Rounded label at (x, y), with a coloured dot before the text when `dot` is set.
fn pill(img: &mut RgbaImage, font: &FontRef, x: i64, y: i64, text: &str, dot: Option<[u8; 3]>) {
    let w = pill_width(font, text, dot.is_some());
    round_rect(img, x, y, w, PILL_H, PILL_H / 2, CARD_BORDER);
    round_rect(img, x + 2, y + 2, w - 4, PILL_H - 4, PILL_H / 2 - 2, PILL);
    if let Some(color) = dot {
        round_rect(img, x + 18, y + PILL_H / 2 - 6, 12, 12, 6, color);
    }
    let lead = if dot.is_some() { 40 } else { 18 };
    draw_text(
        img,
        font,
        PILL_PX,
        (x + lead) as f32,
        centered_top(PILL_PX, (y + PILL_H / 2) as f32),
        TEXT,
        text,
        false,
    );
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

/// Top of a line of `px` text whose capitals are centred on `mid` (Ubuntu: ascent 0.83 of the
/// line height, capitals 0.62).
fn centered_top(px: f32, mid: f32) -> f32 {
    mid - px * 0.52
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

// Render-service icons have a transparent margin around their frame: the frame spans 17..194 of
// the 217 px picture across and 18..191 down (it is a little shorter than wide).
const FRAME_LEFT: f32 = 17.0 / 217.0;
const FRAME_TOP: f32 = 16.0 / 217.0; // 18, less half the 4 px height difference
const FRAME_SIZE: f32 = 177.0 / 217.0;

/// Square the whole picture is drawn in so that its frame fills the `size` square at (x, y), less
/// `inset` on each side. Glows outside the frame spill over the square, as in the game.
fn icon_box(x: i64, y: i64, size: i64, inset: i64) -> (i64, i64, u32) {
    let full = (size - 2 * inset) as f32 / FRAME_SIZE;
    (
        x + inset - (full * FRAME_LEFT).round() as i64,
        y + inset - (full * FRAME_TOP).round() as i64,
        full.round() as u32,
    )
}

/// Draws `icon` with its frame fitted to the `size` square at (x, y) (see [`icon_box`]), or a
/// placeholder when the icon is missing.
fn draw_icon(
    img: &mut RgbaImage,
    font: &FontRef,
    icon: Option<&RgbaImage>,
    x: i64,
    y: i64,
    size: i64,
    inset: i64,
) {
    match icon {
        Some(icon) => {
            let (ix, iy, full) = icon_box(x, y, size, inset);
            imageops::overlay(img, &*fit(icon, full), ix, iy);
        }
        None => {
            let pad = size / 8;
            round_rect(
                img,
                x + pad,
                y + pad,
                size - 2 * pad,
                size - 2 * pad,
                10,
                [52, 46, 40],
            );
            let px = size as f32 * 0.4;
            draw_centered(
                img,
                font,
                px,
                (x + size / 2) as f32,
                centered_top(px, (y + size / 2) as f32),
                DIM,
                "?",
                false,
            );
        }
    }
}

fn fit(icon: &RgbaImage, size: u32) -> Cow<'_, RgbaImage> {
    if icon.dimensions() == (size, size) {
        Cow::Borrowed(icon)
    } else {
        Cow::Owned(imageops::resize(
            icon,
            size,
            size,
            imageops::FilterType::Lanczos3,
        ))
    }
}

/// `icon` faded to grey, placed like [`draw_icon`]: the two-handed weapon shown again in its empty
/// off-hand slot, as in the game.
fn draw_ghost(img: &mut RgbaImage, icon: &RgbaImage, x: i64, y: i64, size: i64, inset: i64) {
    let (ix, iy, full) = icon_box(x, y, size, inset);
    for (px, py, p) in fit(icon, full).enumerate_pixels() {
        let grey = (0.3 * p[0] as f32 + 0.59 * p[1] as f32 + 0.11 * p[2] as f32) as u8;
        blend(
            img,
            ix + px as i64,
            iy + py as i64,
            [grey; 3],
            p[3] as f32 / 255.0 * 0.3,
        );
    }
}

/// The build's main-hand icon when the weapon is two-handed, for the ghost in its off-hand slot.
fn two_handed_icon<'a>(build: &Build, items: &Items, icons: &'a Icons) -> Option<&'a RgbaImage> {
    let main = build.get(Slot::MainHand)?;
    let item = items.get(&main.item).filter(|i| i.two_handed)?;
    icons.get(&IconKey {
        icon: item.icon.clone(),
        enchant: main.enchant,
    })
}

/// `.3` pill at the bottom-right corner of an icon; nothing for enchant 0.
fn draw_badge(img: &mut RgbaImage, font: &FontRef, enchant: u8, right: i64, bottom: i64, px: f32) {
    if enchant == 0 {
        return;
    }
    let label = format!(".{enchant}");
    let w = (text_width(font, px, &label) + px * 0.7).round() as i64;
    let h = (px * 1.3).round() as i64;
    round_rect(img, right - w, bottom - h, w, h, h / 2, INK);
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
        INK,
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
    /// Gap between the slot square's edge and the frame of its icon.
    const ICON_INSET: i64 = 12;
    const SWAP: i64 = 72;
    const SWAP_GAP: i64 = 8;
    /// Rule and "Swaps" heading above the swap icons.
    const SWAP_HEAD: i64 = 50;
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
        SWAP_HEAD + swap_rows * SWAP + (swap_rows - 1) * SWAP_GAP
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
    let ghost = two_handed_icon(build, items, icons);

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
            if equipped.is_some() { CARD } else { CARD_EMPTY },
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
        let name_y = icon_y + ICON + 10;
        well(&mut img, icon_x, icon_y, ICON, 22);
        let Some(e) = equipped else {
            let off_hand_of_two_handed = slot == Slot::OffHand && two_handed;
            if let Some(ghost) = ghost.filter(|_| off_hand_of_two_handed) {
                draw_ghost(&mut img, ghost, icon_x, icon_y, ICON, ICON_INSET);
            }
            let (note, color) = if off_hand_of_two_handed {
                ("Arme à deux mains", DIM)
            } else {
                ("Vide", FAINT)
            };
            draw_centered(
                &mut img,
                &font,
                30.0,
                (x + card_w / 2) as f32,
                name_y as f32,
                color,
                note,
                false,
            );
            continue;
        };

        let (icon, name) = icon_for(icons, items, &e.item, e.enchant);
        draw_icon(&mut img, &font, icon, icon_x, icon_y, ICON, ICON_INSET);
        draw_badge(
            &mut img,
            &font,
            e.enchant,
            icon_x + ICON - 4,
            icon_y + ICON - 4,
            34.0,
        );

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
        if !e.swaps.is_empty() {
            let top = name_y + 72 + 6;
            fill(
                &mut img,
                (x + PAD, top, card_w - 2 * PAD, 2),
                0,
                CARD_BORDER,
                CARD_BORDER,
                1.0,
                false,
            );
            draw_text(
                &mut img,
                &font,
                24.0,
                (x + PAD) as f32,
                (top + 12) as f32,
                DIM,
                &format!("Swaps ({})", e.swaps.len()),
                false,
            );
            draw_swaps(
                &mut img,
                &font,
                items,
                icons,
                e,
                x + PAD,
                name_y + 72 + SWAP_HEAD,
                (per_row * swap_rows) as usize,
                per_row,
                (SWAP, SWAP_GAP),
            );
        }
    }

    draw_signature(
        &mut img,
        &font,
        (W - MARGIN) as f32,
        (height - footer_h + 14) as f32,
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
    (size, gap): (i64, i64),
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
    let cell = |i: i64| {
        (
            x + (i % per_row) * (size + gap),
            y + (i / per_row) * (size + gap),
        )
    };
    let radius = (size / 6).max(6);
    for (i, swap) in equipped.swaps.iter().take(shown).enumerate() {
        let (cx, cy) = cell(i as i64);
        let (icon, _) = icon_for(icons, items, &swap.item, swap.enchant);
        well(img, cx, cy, size, radius);
        // Small icons fill their square: a margin would make them unreadable.
        draw_icon(img, font, icon, cx, cy, size, 0);
        let badge_px = (size as f32 * 0.28).max(14.0);
        draw_badge(img, font, swap.enchant, cx + size, cy + size, badge_px);
    }
    if total > shown {
        let (cx, cy) = cell(shown as i64);
        well(img, cx, cy, size, radius);
        let px = size as f32 * 0.42;
        draw_centered(
            img,
            font,
            px,
            (cx + size / 2) as f32,
            centered_top(px, (cy + size / 2) as f32),
            DIM,
            &format!("+{}", total - shown),
            true,
        );
    }
}

// ---- composition image ---------------------------------------------------------------------------

/// Renders a composition as a table: one row per member (role, player and build on the left, then
/// the whole equipment on a single line with each item's swaps in small under it), one column per
/// slot, so the same piece of every member lines up.
pub fn render_comp(
    comp: &Composition,
    builds: &[Build],
    items: &Items,
    icons: &Icons,
) -> Result<RgbaImage> {
    const MIN_W: i64 = 960;
    /// Left part of a row: colour bar, role, player and build.
    const INFO_W: i64 = 330;
    const TILE: i64 = 128;
    const TILE_GAP: i64 = 16;
    /// Gap between a tile's edge and the frame of its icon.
    const TILE_INSET: i64 = 6;
    const MINI: i64 = 40;
    const MINI_GAP: i64 = 4;
    /// Room under a tile for its swaps.
    const MINI_ROW: i64 = 10 + MINI;
    const MINI_PER_TILE: usize = 3;
    const PAD: i64 = 18;
    const ROW_GAP: i64 = 16;
    /// Slot names above the columns.
    const LABELS_H: i64 = 44;
    let font = font();

    let members = comp.members.len() as i64;
    let resolved: Vec<Option<&Build>> = (0..comp.members.len())
        .map(|i| comp.resolve(i, builds))
        .collect();

    // Slots nobody fills get no column (all of them show while no member has a build yet).
    let filled = |slot: Slot, with_swaps: bool| {
        resolved
            .iter()
            .flatten()
            .filter_map(|b| b.get(slot))
            .any(|e| !with_swaps || !e.swaps.is_empty())
    };
    let mut slots: Vec<Slot> = Slot::ALL
        .into_iter()
        .filter(|&s| filled(s, false))
        .collect();
    if slots.is_empty() {
        slots = Slot::ALL.to_vec();
    }
    // All rows share one height, with room for swaps as soon as anyone has some.
    let any_swaps = slots.iter().any(|&s| filled(s, true));
    let columns = slots.len() as i64;
    let grid_w = columns * TILE + (columns - 1) * TILE_GAP;
    let grid_h = TILE + if any_swaps { MINI_ROW } else { 0 };
    let row_w = INFO_W + grid_w + PAD;
    let width = (2 * MARGIN + row_w).max(MIN_W);

    let title = if comp.name.trim().is_empty() {
        "Composition sans nom"
    } else {
        comp.name.trim()
    };
    let note_lines = wrap(&font, 30.0, &comp.notes, (width - 2 * MARGIN) as f32, 4);
    let pills_y = MARGIN + 124;
    let notes_y = pills_y
        + if comp.members.is_empty() {
            0
        } else {
            PILL_H + 24
        };
    let header_h = notes_y + note_lines.len() as i64 * 40 + 36;
    let any_player = comp.members.iter().any(|m| !m.player.trim().is_empty());
    let member_notes = comp.members.iter().any(|m| !m.note.trim().is_empty());
    let info_h = 46 + if any_player { 36 } else { 0 } + 32 + if member_notes { 56 } else { 0 };
    let row_h = info_h.max(grid_h) + 2 * PAD;
    let table_top = header_h + LABELS_H;
    let table_h = if members == 0 {
        60
    } else {
        members * row_h + (members - 1) * ROW_GAP
    };
    let footer_h = 64;
    let height = table_top + table_h + footer_h;
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
    if !comp.members.is_empty() {
        // Head count, then each role with its colour ("2 DPS", "1 Heal"), as far as the width allows.
        let count = format!(
            "{members} {}",
            if members > 1 { "joueurs" } else { "joueur" }
        );
        let roles = role_groups(comp).into_iter().map(|(role, n)| {
            let color = role_color(&role);
            (
                format!("{n} {}", ellipsize(&font, PILL_PX, &role, 280.0)),
                Some(color),
            )
        });
        let mut px = MARGIN;
        for (text, dot) in std::iter::once((count, None)).chain(roles) {
            let w = pill_width(&font, &text, dot.is_some());
            if px + w > width - MARGIN {
                break;
            }
            pill(&mut img, &font, px, pills_y, &text, dot);
            px += w + 12;
        }
    }
    for (i, line) in note_lines.iter().enumerate() {
        draw_text(
            &mut img,
            &font,
            30.0,
            MARGIN as f32,
            (notes_y + i as i64 * 40) as f32,
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

    // Rows are centred when the image is held wider than them by MIN_W.
    let left = (width - row_w) / 2;
    let grid_x = left + INFO_W;
    let column_x = |i: usize| grid_x + i as i64 * (TILE + TILE_GAP);
    if !comp.members.is_empty() {
        for (i, &slot) in slots.iter().enumerate() {
            let label = if slot == Slot::OffHand {
                "Main sec."
            } else {
                slot.label()
            };
            draw_centered(
                &mut img,
                &font,
                22.0,
                (column_x(i) + TILE / 2) as f32,
                (header_h + 6) as f32,
                DIM,
                &ellipsize(&font, 22.0, label, (TILE + TILE_GAP - 6) as f32),
                false,
            );
        }
    }

    let info_x = left + 40;
    let info_w = (INFO_W - 40 - 20) as f32;
    for (n, member) in comp.members.iter().enumerate() {
        let y = table_top + n as i64 * (row_h + ROW_GAP);
        let build = resolved[n];
        let role = if member.role.trim().is_empty() {
            "Rôle"
        } else {
            member.role.trim()
        };
        let accent = role_color(role);

        // The whole row is faintly tinted with the role colour, with a bar of it down the left edge.
        card(&mut img, left, y, row_w, row_h, lerp(CARD, accent, 0.07));
        round_rect(&mut img, left + 16, y + 16, 6, row_h - 32, 3, accent);

        let mut ty = y + PAD + (row_h - 2 * PAD - info_h) / 2;
        let mut line = |img: &mut RgbaImage, px: f32, color, text: &str, bold, step| {
            let text = ellipsize(&font, px, text, info_w);
            draw_text(img, &font, px, info_x as f32, ty as f32, color, &text, bold);
            ty += step;
        };
        line(&mut img, 38.0, accent, role, true, 46);
        if any_player {
            match member.player.trim() {
                "" => line(&mut img, 28.0, FAINT, "—", false, 36),
                name => line(&mut img, 28.0, TEXT, name, false, 36),
            }
        }
        match (&member.build, build) {
            (_, Some(b)) => {
                let color = if any_player { DIM } else { TEXT };
                line(&mut img, 26.0, color, &b.name, false, 32);
            }
            (Some(name), None) => line(
                &mut img,
                26.0,
                DANGER,
                &format!("{name} (introuvable)"),
                false,
                32,
            ),
            (None, None) => line(&mut img, 26.0, FAINT, "Aucun build", false, 32),
        }
        for note in wrap(&font, 22.0, &member.note, info_w, 2) {
            line(&mut img, 22.0, DIM, &note, false, 26);
        }

        let ghost = build.and_then(|b| two_handed_icon(b, items, icons));
        let gy = y + PAD + (row_h - 2 * PAD - grid_h) / 2;
        for (i, &slot) in slots.iter().enumerate() {
            let tx = column_x(i);
            well(&mut img, tx, gy, TILE, 16);
            let Some(e) = build.and_then(|b| b.get(slot)) else {
                match ghost.filter(|_| slot == Slot::OffHand) {
                    Some(ghost) => draw_ghost(&mut img, ghost, tx, gy, TILE, TILE_INSET),
                    None => slot_name(&mut img, &font, slot, tx, gy, TILE),
                }
                continue;
            };
            let (icon, _) = icon_for(icons, items, &e.item, e.enchant);
            draw_icon(&mut img, &font, icon, tx, gy, TILE, TILE_INSET);
            draw_badge(
                &mut img,
                &font,
                e.enchant,
                tx + TILE - 4,
                gy + TILE - 4,
                24.0,
            );
            // Swaps sit centred right under the item they replace.
            let shown = e.swaps.len().min(MINI_PER_TILE) as i64;
            let strip_w = shown * MINI + (shown - 1).max(0) * MINI_GAP;
            draw_swaps(
                &mut img,
                &font,
                items,
                icons,
                e,
                tx + (TILE - strip_w) / 2,
                gy + TILE + 10,
                MINI_PER_TILE,
                MINI_PER_TILE as i64,
                (MINI, MINI_GAP),
            );
        }
    }

    let footer_y = (height - footer_h + 18) as f32;
    if any_swaps {
        draw_text(
            &mut img,
            &font,
            24.0,
            MARGIN as f32,
            footer_y,
            DIM,
            "Petites icônes : swaps de l'objet au-dessus",
            false,
        );
    }
    draw_signature(&mut img, &font, (width - MARGIN) as f32, footer_y);
    Ok(img)
}

/// Small signature ending at `right`; `top` is where a 24 px footer line starts, so both share a baseline.
fn draw_signature(img: &mut RgbaImage, font: &FontRef, right: f32, top: f32) {
    const PX: f32 = 18.0;
    const SIGNATURE: &str = "AOCompBuilder by slayde";
    draw_text(
        img,
        font,
        PX,
        right - text_width(font, PX, SIGNATURE),
        top + (24.0 - PX) * 0.83,
        DIM,
        SIGNATURE,
        false,
    );
}

/// Name of an empty slot, faint, in the middle of its square.
fn slot_name(img: &mut RgbaImage, font: &FontRef, slot: Slot, x: i64, y: i64, size: i64) {
    const PX: f32 = 20.0;
    const LINE: f32 = 24.0;
    let lines = wrap(font, PX, slot.label(), (size - 16) as f32, 2);
    let top =
        centered_top(PX, (y + size / 2) as f32) - (lines.len() as f32 - 1.0).max(0.0) * LINE / 2.0;
    for (i, line) in lines.iter().enumerate() {
        draw_centered(
            img,
            font,
            PX,
            (x + size / 2) as f32,
            top + i as f32 * LINE,
            FAINT,
            line,
            false,
        );
    }
}

/// Roles in order of first appearance with how many members hold each; case is ignored when
/// grouping and the first spelling is kept.
fn role_groups(comp: &Composition) -> Vec<(String, usize)> {
    let mut groups: Vec<(String, usize)> = Vec::new();
    for role in comp.members.iter().map(|m| m.role.trim()) {
        if role.is_empty() {
            continue;
        }
        match groups
            .iter_mut()
            .find(|(g, _)| g.to_lowercase() == role.to_lowercase())
        {
            Some((_, n)) => *n += 1,
            None => groups.push((role.to_owned(), 1)),
        }
    }
    groups
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
        // One column per slot, whatever the member count: the sample fills all ten.
        assert_eq!(one.width(), 2 * 60 + 330 + 10 * 128 + 9 * 16 + 18);
        assert_eq!(seven.width(), one.width());
        assert!(six.height() > one.height());
        assert!(seven.height() > six.height(), "one row per member");
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
    fn comp_makes_room_for_swaps_and_skips_slots_nobody_fills() {
        let c = catalog();
        let size = |b: &Build| {
            let comp = Composition {
                name: "3v3".into(),
                members: vec![Member {
                    role: "DPS".into(),
                    build: Some(b.name.clone()),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let builds = [b.clone()];
            let items = Items::for_builds(&builds, &c);
            render_comp(&comp, &builds, &items, &fake_icons(b, &c))
                .unwrap()
                .dimensions()
        };
        let plain = sample(&c, 0);
        let mut no_bag = plain.clone();
        no_bag.clear(Slot::Bag);
        no_bag.clear(Slot::Mount);
        assert!(
            size(&sample(&c, 2)).1 > size(&plain).1,
            "room for the swaps"
        );
        assert_eq!(
            size(&plain).0 - size(&no_bag).0,
            2 * (128 + 16),
            "bag and mount columns left out"
        );
    }

    #[test]
    fn roles_are_grouped_and_coloured() {
        let member = |role: &str| Member {
            role: role.into(),
            ..Default::default()
        };
        let comp = Composition {
            members: ["DPS", "Heal", "dps", " ", "Scout"].map(member).to_vec(),
            ..Default::default()
        };
        assert_eq!(
            role_groups(&comp),
            [("DPS".into(), 2), ("Heal".into(), 1), ("Scout".into(), 1)]
        );
        assert_ne!(role_color("Main heal"), role_color("Off-tank"));
        assert_eq!(role_color("Scout"), GOLD);
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
