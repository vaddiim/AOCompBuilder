//! The build detail page (reached from a composition member or a "Mes Builds" card) and the
//! background image export with its progress window.
//!
//! The page shows the equipment as a mannequin; for each item only the spells and passives the
//! build picked, with their stats (the description is in the tooltip). Swaps follow below.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use eframe::egui::{self, Align2, Key, Modifiers, RichText};

use crate::build::{Build, Equipped};
use crate::catalog::{Catalog, Slot, Spell};
use crate::export_image::{Job, Paths, Progress};
use crate::share;

use super::theme::{COPPER, DANGER, GOLD, MUTED, SPELL_ICON, card_frame};
use super::widgets::{
    enchant_badge, item_icon, item_name, item_tooltip, primary_button, spell_icon, spell_name,
    spell_tooltip, tier_badge, tier_to_show,
};
use super::{App, Dialog, Pending, Tab};

const ITEM_ICON: f32 = 64.0;
/// Below this width the mannequin becomes a single column.
const WIDE: f32 = 760.0;

/// Mannequin rows as in the game's equipment screen; `None` pads a row.
const MANNEQUIN: [[Option<Slot>; 3]; 4] = [
    [Some(Slot::Bag), Some(Slot::Head), Some(Slot::Cape)],
    [Some(Slot::MainHand), Some(Slot::Armor), Some(Slot::OffHand)],
    [Some(Slot::Potion), Some(Slot::Shoes), Some(Slot::Food)],
    [None, Some(Slot::Mount), None],
];

/// Which build the page shows and where "Retour" leads.
pub(super) struct DetailView {
    build: String,
    back: Tab,
    /// `(composition, member)` when opened from a composition: gives the role and player, and the
    /// member to relink if the build is gone.
    member: Option<(usize, usize)>,
    copied: bool,
}

enum Msg {
    Progress(Progress),
    Done(Result<PathBuf, String>),
}

/// A running or finished image export, shown in a small window.
pub(super) struct ExportState {
    title: String,
    progress: Progress,
    rx: Receiver<Msg>,
    result: Option<Result<PathBuf, String>>,
    /// Outcome of the last "Copier l'image".
    copied: Option<Result<(), String>>,
}

impl ExportState {
    fn apply(&mut self, msg: Msg) {
        match msg {
            Msg::Progress(p) => self.progress = p,
            Msg::Done(r) => self.result = Some(r),
        }
    }

    /// Takes every message the worker has sent; a vanished worker counts as a failure.
    fn poll(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(msg) => self.apply(msg),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    if self.result.is_none() {
                        self.result =
                            Some(Err("L'export s'est interrompu de façon inattendue.".into()));
                    }
                    return;
                }
            }
        }
    }
}

enum Action {
    Back,
    Edit,
    Export,
    Copy,
    Relink(String),
}

impl App {
    /// Opens the detail page of the build called `build`, remembering the current tab to go back to.
    pub(super) fn open_detail(&mut self, build: &str, member: Option<(usize, usize)>) {
        let back = if self.tab == Tab::BuildDetail {
            self.detail.as_ref().map_or(Tab::MyBuilds, |d| d.back)
        } else {
            self.tab
        };
        self.detail = Some(DetailView {
            build: build.to_owned(),
            back,
            member,
            copied: false,
        });
        self.tab = Tab::BuildDetail;
    }

    /// The tab to highlight in the bar: the page's origin while the detail page is open.
    pub(super) fn active_tab(&self) -> Tab {
        match (self.tab, &self.detail) {
            (Tab::BuildDetail, Some(d)) => d.back,
            (tab, _) => tab,
        }
    }

    fn close_detail(&mut self) {
        self.tab = self.detail.take().map_or(Tab::MyBuilds, |d| d.back);
    }

    pub(super) fn build_detail_tab(&mut self, ui: &mut egui::Ui) {
        let Some(view) = &self.detail else {
            self.tab = Tab::MyBuilds;
            return;
        };
        let modal_open = self.confirm.is_some() || !matches!(self.dialog, Dialog::None);
        let escape = !modal_open && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));

        let index = self.store.builds.iter().position(|b| b.name == view.build);
        let member = view.member.and_then(|(c, m)| {
            Some((
                self.comps.comps.get(c)?,
                self.comps.comps[c].members.get(m)?,
            ))
        });
        let mut action = escape.then_some(Action::Back);

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let back_label = match view.back {
                        Tab::Comps => "Compositions",
                        _ => "Mes Builds",
                    };
                    ui.horizontal(|ui| {
                        if ui.button("‹ Retour").clicked() {
                            action = Some(Action::Back);
                        }
                        if ui.link(back_label).clicked() {
                            action = Some(Action::Back);
                        }
                        if let Some((comp, _)) = member {
                            ui.weak("›");
                            ui.label(if comp.name.trim().is_empty() {
                                "(sans nom)"
                            } else {
                                comp.name.as_str()
                            });
                        }
                        ui.weak("›");
                        ui.label(RichText::new(&view.build).color(GOLD));
                    });
                    ui.add_space(6.0);

                    let Some(build) = index.map(|i| &self.store.builds[i]) else {
                        missing_build(
                            ui,
                            &view.build,
                            member.is_some(),
                            &self.store.builds,
                            &mut action,
                        );
                        return;
                    };

                    ui.horizontal_wrapped(|ui| {
                        ui.heading(RichText::new(&build.name).color(GOLD));
                        if let Some((_, m)) = member {
                            if !m.role.trim().is_empty() {
                                super::widgets::badge(ui, m.role.trim(), COPPER);
                            }
                            if !m.player.trim().is_empty() {
                                ui.label(RichText::new(m.player.trim()).strong());
                            }
                        }
                    });
                    if let Some((_, m)) = member
                        && !m.note.trim().is_empty()
                    {
                        ui.label(RichText::new(&m.note).color(MUTED));
                    }
                    if !build.note.trim().is_empty() {
                        ui.add_space(4.0);
                        card_frame(false).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(&build.note);
                        });
                    }
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui.add(primary_button("Modifier dans l'éditeur")).clicked() {
                            action = Some(Action::Edit);
                        }
                        if ui
                            .button("Exporter en image")
                            .on_hover_text("PNG dans userdata/exports")
                            .clicked()
                        {
                            action = Some(Action::Export);
                        }
                        if ui
                            .button("Copier la chaîne")
                            .on_hover_text("Chaîne de partage AOB1")
                            .clicked()
                        {
                            action = Some(Action::Copy);
                        }
                        if view.copied {
                            ui.weak("Chaîne copiée.");
                        }
                    });
                    ui.add_space(10.0);

                    mannequin(ui, &self.catalog, &self.icons, build);
                    swaps_section(ui, &self.catalog, &self.icons, build);
                });
        });

        match action {
            Some(Action::Back) => self.close_detail(),
            Some(Action::Edit) => {
                if let Some(i) = index {
                    self.request_replace(Pending::Edit(i));
                }
            }
            Some(Action::Export) => {
                if let Some(build) = index.map(|i| &self.store.builds[i]) {
                    let job = Job::build(build, &self.catalog, &self.export_paths());
                    self.start_export(ui.ctx(), job);
                }
            }
            Some(Action::Copy) => {
                if let Some(build) = index.map(|i| &self.store.builds[i]) {
                    ui.ctx().copy_text(share::encode(build));
                    if let Some(view) = &mut self.detail {
                        view.copied = true;
                    }
                }
            }
            Some(Action::Relink(name)) => {
                let target = self.detail.as_ref().and_then(|d| d.member);
                if let Some(slot) =
                    target.and_then(|(c, m)| self.comps.comps.get_mut(c)?.members.get_mut(m))
                {
                    slot.build = Some(name.clone());
                }
                if let Some(view) = &mut self.detail {
                    view.build = name;
                }
            }
            None => {}
        }
    }

    /// Where exported images and the HD icon cache live, derived from the `userdata/exports` folder.
    pub(super) fn export_paths(&self) -> Paths {
        let userdata = self.exports.parent().unwrap_or(&self.exports);
        Paths {
            icons: self.icons.clone(),
            cache: userdata.join("cache").join("icons_hd"),
            exports: self.exports.clone(),
        }
    }

    /// Runs `job` on a worker thread; progress and the result show up in [`App::export_window`].
    /// Ignored while another export is still running.
    pub(super) fn start_export(&mut self, ctx: &egui::Context, job: Job) {
        if self.export.as_ref().is_some_and(|e| e.result.is_none()) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let dest = job.default_dest();
        let title = job.title().to_owned();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let report = tx.clone();
            let repaint = ctx.clone();
            let result = job.run(&dest, &mut |p| {
                let _ = report.send(Msg::Progress(p));
                repaint.request_repaint();
            });
            let _ = tx.send(Msg::Done(
                result.map(|()| dest).map_err(|e| format!("{e:#}")),
            ));
            ctx.request_repaint();
        });
        self.export = Some(ExportState {
            title,
            progress: Progress::Icons { done: 0, total: 0 },
            rx,
            result: None,
            copied: None,
        });
    }

    /// Floating window with the export progress, then the outcome and shortcuts to the file.
    pub(super) fn export_window(&mut self, ctx: &egui::Context) {
        let Some(state) = &mut self.export else {
            return;
        };
        state.poll();
        let mut close = false;
        egui::Window::new("Export en image")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::RIGHT_BOTTOM, [-16.0, -16.0])
            .show(ctx, |ui| {
                ui.set_width(300.0);
                ui.label(RichText::new(&state.title).strong());
                match &state.result {
                    None => progress_ui(ui, state.progress),
                    Some(Ok(path)) => {
                        ui.label("Image enregistrée :");
                        ui.label(
                            RichText::new(path.display().to_string())
                                .small()
                                .color(MUTED),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .add(primary_button("Copier l'image"))
                                .on_hover_text(
                                    "Pour la coller directement dans Discord, un message…",
                                )
                                .clicked()
                            {
                                state.copied =
                                    Some(clipboard_image(path).map(|img| ui.ctx().copy_image(img)));
                            }
                            match &state.copied {
                                Some(Ok(())) => {
                                    ui.weak("Image copiée.");
                                }
                                Some(Err(e)) => {
                                    ui.colored_label(DANGER, e.as_str());
                                }
                                None => {}
                            }
                        });
                        ui.horizontal(|ui| {
                            if ui.button("Ouvrir le dossier").clicked() {
                                reveal(path);
                            }
                            if ui.button("Ouvrir l'image").clicked() {
                                open(path);
                            }
                        });
                    }
                    Some(Err(e)) => {
                        ui.colored_label(DANGER, format!("Échec de l'export : {e}"));
                    }
                }
                if state.result.is_some() && ui.button("Fermer").clicked() {
                    close = true;
                }
            });
        if close {
            self.export = None;
        }
    }
}

fn progress_ui(ui: &mut egui::Ui, progress: Progress) {
    match progress {
        Progress::Icons { done, total } => {
            let fraction = if total == 0 {
                0.0
            } else {
                done as f32 / total as f32
            };
            ui.add(egui::ProgressBar::new(fraction).text(format!("Icônes HD : {done}/{total}")));
            ui.weak("Téléchargées au besoin ; hors ligne, les petites icônes sont utilisées.");
        }
        Progress::Rendering => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Création de l'image…");
            });
        }
        Progress::Saving => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Enregistrement…");
            });
        }
    }
}

/// The exported PNG, read back for the clipboard.
fn clipboard_image(path: &Path) -> Result<egui::ColorImage, String> {
    let img = image::open(path)
        .map_err(|e| format!("Impossible de lire l'image : {e}"))?
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Ok(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

/// Shows `path` selected in the file manager.
fn reveal(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();
}

/// Opens `path` with the default application.
fn open(path: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

/// Error state: the build the page was opened for is not in "Mes Builds" any more.
fn missing_build(
    ui: &mut egui::Ui,
    name: &str,
    from_member: bool,
    builds: &[Build],
    action: &mut Option<Action>,
) {
    card_frame(false).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.heading(RichText::new("Build introuvable").color(DANGER));
        ui.label(format!(
            "Le build « {name} » n'existe plus dans Mes Builds (supprimé ou renommé)."
        ));
        if from_member {
            ui.add_space(6.0);
            if builds.is_empty() {
                ui.weak("Mes Builds est vide : crée ou importe d'abord un build.");
            } else {
                ui.label("Relier ce membre à un autre build :");
                egui::ComboBox::from_id_salt("relink_build")
                    .selected_text("Choisir un build…")
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        for build in builds {
                            if ui.selectable_label(false, &build.name).clicked() {
                                *action = Some(Action::Relink(build.name.clone()));
                            }
                        }
                    });
            }
        }
    });
}

/// The ten slots as cards laid out like the game's equipment screen.
fn mannequin(ui: &mut egui::Ui, catalog: &Catalog, icons: &Path, build: &Build) {
    ui.label(RichText::new("Équipement").heading().color(GOLD));
    ui.add_space(4.0);
    let two_handed = build
        .get(Slot::MainHand)
        .and_then(|e| catalog.item(&e.item))
        .is_some_and(|i| i.two_handed);
    let columns = if ui.available_width() >= WIDE { 3 } else { 1 };
    for row in mannequin_rows(columns) {
        ui.columns(columns, |cols| {
            for (col, slot) in cols.iter_mut().zip(row) {
                let Some(slot) = slot else { continue };
                card_frame(false).show(col, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(slot.label()).small().strong().color(MUTED));
                    match build.get(slot) {
                        Some(e) => loadout(
                            ui,
                            catalog,
                            icons,
                            slot,
                            &e.item,
                            e.enchant,
                            &e.spells,
                            &e.passives,
                        ),
                        None if slot == Slot::OffHand && two_handed => {
                            ui.weak("Arme à deux mains");
                        }
                        None => {
                            ui.weak("—");
                        }
                    }
                });
            }
        });
        ui.add_space(6.0);
    }
}

/// The mannequin as rows of `columns` cells; a single column drops the padding cells.
fn mannequin_rows(columns: usize) -> Vec<Vec<Option<Slot>>> {
    if columns >= 3 {
        return MANNEQUIN.iter().map(|r| r.to_vec()).collect();
    }
    MANNEQUIN
        .iter()
        .flatten()
        .flatten()
        .map(|s| vec![Some(*s)])
        .collect()
}

fn swaps_section(ui: &mut egui::Ui, catalog: &Catalog, icons: &Path, build: &Build) {
    let swaps: Vec<(Slot, usize, &Equipped)> = Slot::ALL
        .into_iter()
        .filter_map(|s| build.get(s).map(|e| (s, e)))
        .flat_map(|(s, e)| (0..e.swaps.len()).map(move |k| (s, k, e)))
        .collect();
    if swaps.is_empty() {
        return;
    }
    ui.add_space(8.0);
    ui.label(RichText::new("Swaps").heading().color(GOLD));
    ui.weak("Objets alternatifs selon la situation.");
    ui.add_space(4.0);
    let columns = if ui.available_width() >= WIDE { 3 } else { 1 };
    for chunk in swaps.chunks(columns) {
        ui.columns(columns, |cols| {
            for (col, (slot, k, e)) in cols.iter_mut().zip(chunk) {
                let swap = &e.swaps[*k];
                card_frame(false).show(col, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new(format!("{} · swap {}", slot.label(), k + 1))
                            .small()
                            .strong()
                            .color(MUTED),
                    );
                    if !swap.note.trim().is_empty() {
                        ui.label(RichText::new(&swap.note).italics());
                    }
                    loadout(
                        ui,
                        catalog,
                        icons,
                        *slot,
                        &swap.item,
                        swap.enchant,
                        &swap.spells,
                        &swap.passives,
                    );
                });
            }
        });
        ui.add_space(6.0);
    }
}

/// One item (main or swap) with its chosen spells and passives; unchosen ones are not shown.
#[allow(clippy::too_many_arguments)] // the fields of an Equipped / Swap, which are separate structs
fn loadout(
    ui: &mut egui::Ui,
    catalog: &Catalog,
    icons: &Path,
    slot: Slot,
    item_id: &str,
    enchant: u8,
    spells: &[Option<String>],
    passives: &[Option<String>],
) {
    ui.horizontal(|ui| {
        let resp = ui.add(
            item_icon(catalog, icons, item_id, enchant)
                .fit_to_exact_size(egui::Vec2::splat(ITEM_ICON)),
        );
        ui.vertical(|ui| match catalog.item(item_id) {
            Some(item) => {
                ui.label(RichText::new(item_name(item)).strong().size(16.0));
                ui.horizontal(|ui| {
                    if let Some(tier) = tier_to_show(item) {
                        tier_badge(ui, tier);
                    }
                    enchant_badge(ui, enchant);
                });
                resp.on_hover_ui(|ui| item_tooltip(ui, item, enchant));
            }
            None => {
                ui.colored_label(DANGER, format!("Objet inconnu : {item_id}"));
            }
        });
    });
    for (i, id) in spells.iter().enumerate() {
        spell_line(ui, catalog, icons, active_label(slot, i), id.as_deref());
    }
    for id in passives {
        spell_line(ui, catalog, icons, "Passif", id.as_deref());
    }
}

/// Q/W/E on weapons; armour and the rest have a single unlabelled active.
fn active_label(slot: Slot, index: usize) -> &'static str {
    match (slot, index) {
        (Slot::MainHand, 0) => "Q",
        (Slot::MainHand, 1) => "W",
        (Slot::MainHand, 2) => "E",
        _ => "Sort",
    }
}

fn spell_line(ui: &mut egui::Ui, catalog: &Catalog, icons: &Path, label: &str, id: Option<&str>) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let Some(id) = id else {
            ui.add_sized(
                [SPELL_ICON, SPELL_ICON],
                egui::Label::new(RichText::new(label).strong().color(MUTED)),
            );
            ui.weak("aucun choisi");
            return;
        };
        match spell_icon(catalog, icons, id) {
            Some(icon) => {
                ui.add(icon.fit_to_exact_size(egui::Vec2::splat(SPELL_ICON)))
                    .on_hover_ui(|ui| spell_tooltip(ui, catalog, id));
            }
            None => {
                ui.add_sized(
                    [SPELL_ICON, SPELL_ICON],
                    egui::Label::new(RichText::new("?").color(MUTED)),
                );
            }
        }
        ui.vertical(|ui| {
            let spell = catalog.spell(id);
            ui.horizontal(|ui| {
                super::widgets::badge(ui, label, COPPER);
                ui.label(RichText::new(spell.map_or(id, spell_name)).strong());
            });
            if let Some(stats) = spell.map(spell_stats).filter(|s| !s.is_empty()) {
                ui.label(RichText::new(stats).small().color(MUTED));
            }
        });
    });
}

/// "Recharge 12 s · Énergie 30 · Portée 8 m"; stats a spell does not have are left out.
fn spell_stats(spell: &Spell) -> String {
    [
        (spell.cooldown, "Recharge", " s"),
        (spell.energy, "Énergie", ""),
        (spell.range, "Portée", " m"),
        (spell.cast_time, "Incantation", " s"),
    ]
    .into_iter()
    .filter(|(v, ..)| *v > 0.0)
    .map(|(v, label, unit)| format!("{label} {v}{unit}"))
    .collect::<Vec<_>>()
    .join("  ·  ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spell(cooldown: f32, energy: f32, range: f32) -> Spell {
        let catalog = Catalog::load(Path::new("assets/data")).expect("catalog");
        let mut spell = catalog.spells.values().next().expect("a spell").clone();
        (spell.cooldown, spell.energy, spell.range, spell.cast_time) =
            (cooldown, energy, range, 0.0);
        spell
    }

    #[test]
    fn spell_stats_skip_missing_values() {
        assert_eq!(
            spell_stats(&spell(12.0, 30.0, 8.0)),
            "Recharge 12 s  ·  Énergie 30  ·  Portée 8 m"
        );
        assert_eq!(spell_stats(&spell(0.0, 0.0, 0.0)), "");
        assert_eq!(spell_stats(&spell(5.5, 0.0, 0.0)), "Recharge 5.5 s");
    }

    #[test]
    fn mannequin_covers_every_slot_once() {
        for columns in [1, 3] {
            let slots: Vec<Slot> = mannequin_rows(columns)
                .into_iter()
                .flatten()
                .flatten()
                .collect();
            assert_eq!(slots.len(), Slot::ALL.len(), "{columns} column(s)");
            assert!(Slot::ALL.iter().all(|s| slots.contains(s)));
        }
        assert!(
            mannequin_rows(1)
                .iter()
                .all(|r| r.len() == 1 && r[0].is_some())
        );
        assert!(mannequin_rows(3).iter().all(|r| r.len() == 3));
    }

    #[test]
    fn weapons_label_their_actives_qwe() {
        assert_eq!(
            [0, 1, 2].map(|i| active_label(Slot::MainHand, i)),
            ["Q", "W", "E"]
        );
        assert_eq!(active_label(Slot::Armor, 0), "Sort");
        assert_eq!(active_label(Slot::MainHand, 3), "Sort");
    }

    #[test]
    fn export_state_follows_the_worker_messages() {
        let (tx, rx) = mpsc::channel();
        let mut state = ExportState {
            title: "t".into(),
            progress: Progress::Icons { done: 0, total: 0 },
            rx,
            result: None,
            copied: None,
        };
        tx.send(Msg::Progress(Progress::Icons { done: 1, total: 3 }))
            .unwrap();
        state.poll();
        assert_eq!(state.progress, Progress::Icons { done: 1, total: 3 });
        assert!(state.result.is_none());
        tx.send(Msg::Done(Ok(PathBuf::from("a.png")))).unwrap();
        state.poll();
        assert!(matches!(state.result, Some(Ok(_))));
    }

    #[test]
    fn exported_png_is_read_back_for_the_clipboard() {
        let dir = std::env::temp_dir().join(format!("aocb-clip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("comp.png");
        image::RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]))
            .save(&path)
            .unwrap();
        let img = clipboard_image(&path).unwrap();
        assert_eq!(img.size, [3, 2]);
        assert_eq!(img.pixels[0], egui::Color32::from_rgb(10, 20, 30));
        assert!(clipboard_image(&dir.join("absent.png")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_vanished_worker_is_reported_as_a_failure() {
        let (tx, rx) = mpsc::channel::<Msg>();
        let mut state = ExportState {
            title: "t".into(),
            progress: Progress::Rendering,
            rx,
            result: None,
            copied: None,
        };
        drop(tx);
        state.poll();
        assert!(matches!(state.result, Some(Err(_))));
    }
}
