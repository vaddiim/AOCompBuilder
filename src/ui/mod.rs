//! egui front end: application state, tabs and the screens in the submodules.

mod build_detail;
mod comps;
mod dialogs;
mod editor;
mod my_builds;
mod theme;
mod widgets;

use std::path::PathBuf;

use eframe::egui::{self, Key, KeyboardShortcut, Margin, Modifiers, RichText, Stroke};

use crate::build::{Build, BuildStore, Issue};
use crate::catalog::{Catalog, Slot};
use crate::comp::CompStore;
use crate::share;

pub use theme::install as install_theme;

use build_detail::{DetailView, ExportState};
use comps::comps_snapshot;
use editor::enforce_two_handed;
use theme::{BG, BORDER, GOLD};
use widgets::{Filter, nav_tab};

const SAVE_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Editor,
    MyBuilds,
    Comps,
    /// Not a tab of the bar: the page opened from a member or a card; `App::detail` says where back goes.
    BuildDetail,
}

/// Import/export windows; they own their text fields while open.
enum Dialog {
    None,
    Export {
        build: Build,
        code: String,
        path: String,
        result: String,
    },
    Import {
        text: String,
        path: String,
        error: String,
    },
    ExportComp {
        name: String,
        code: String,
        result: String,
    },
    ImportComp {
        text: String,
        error: String,
    },
    RenameBuild {
        index: usize,
        name: String,
        error: String,
    },
    /// Choosing the item to add as a swap of `slot`; the filter is the dialog's own.
    AddSwap {
        slot: Slot,
        filter: Filter,
    },
}

/// What a confirmation window is about to do once the user agrees.
enum Confirm {
    DeleteBuild(usize),
    DeleteComp(usize),
    /// Replacing the build in the editor while it has unsaved edits.
    Discard(Pending),
}

enum Pending {
    New,
    Edit(usize),
    Import(Build, Vec<Issue>),
}

pub struct App {
    catalog: Catalog,
    store: BuildStore,
    comps: CompStore,
    comp_sel: Option<usize>,
    /// Compositions as last written, serialized; autosave fires when they differ.
    comps_saved: String,
    comps_dirty_since: Option<f64>,
    comp_status: String,
    icons: PathBuf,
    /// Default folder for exported `.json` files.
    exports: PathBuf,
    dialog: Dialog,
    /// Lines shown under the build name: startup repairs, import issues.
    report: Vec<String>,
    tab: Tab,
    build: Build,
    selected: Slot,
    filter: Filter,
    spell_search: String,
    status: String,
    /// The build as last saved or loaded, serialized; differs from `build` when edits are unsaved.
    saved: String,
    confirm: Option<Confirm>,
    /// The build shown on the detail page (`Tab::BuildDetail`).
    detail: Option<DetailView>,
    /// Image export in progress or just finished.
    export: Option<ExportState>,
}

impl App {
    pub fn new(catalog: Catalog, mut store: BuildStore, comps: CompStore, assets: PathBuf) -> Self {
        let mut status = String::new();
        let mut report = Vec::new();
        let repaired = store.validate_all(&catalog);
        if !repaired.is_empty() {
            status = match store.save() {
                Ok(()) => format!("{} build(s) réparé(s) au chargement.", repaired.len()),
                Err(e) => format!(
                    "{} build(s) réparé(s), mais l'enregistrement a échoué : {e}",
                    repaired.len()
                ),
            };
            report = issue_lines(
                repaired
                    .iter()
                    .map(|(name, issues)| (name.as_str(), issues.as_slice())),
            );
        }
        let exports = assets
            .parent()
            .unwrap_or(&assets)
            .join("userdata")
            .join("exports");
        let comps_saved = comps_snapshot(&comps);
        let comp_status = match comps.check_all(&store.builds).len() {
            0 => String::new(),
            n => format!("{n} composition(s) référencent un build introuvable."),
        };
        Self {
            catalog,
            store,
            comps,
            comp_sel: None,
            comps_saved,
            comps_dirty_since: None,
            comp_status,
            icons: assets.join("icons"),
            exports,
            dialog: Dialog::None,
            report,
            tab: Tab::Editor,
            build: Build::default(),
            selected: Slot::MainHand,
            filter: Filter::default(),
            spell_search: String::new(),
            status,
            saved: snapshot(&Build::default()),
            confirm: None,
            detail: None,
            export: None,
        }
    }

    fn dirty(&self) -> bool {
        snapshot(&self.build) != self.saved
    }

    fn load_build(&mut self, mut build: Build) {
        // `saved` keeps the build as it was, so an off-hand dropped below shows up as an unsaved change.
        self.saved = snapshot(&build);
        let dropped = enforce_two_handed(&mut build, &self.catalog);
        self.build = build;
        self.status = if dropped {
            "Main secondaire retirée : l'arme équipée se tient à deux mains.".into()
        } else {
            String::new()
        };
        self.report.clear();
    }

    fn open_export(&mut self, build: Build) {
        let file = format!("{}.json", sanitize_file_name(&build.name));
        self.dialog = Dialog::Export {
            code: share::encode(&build),
            path: self.exports.join(file).to_string_lossy().into_owned(),
            result: String::new(),
            build,
        };
    }

    /// Collapsible list of repairs and import issues, shared by the editor and Compositions tabs.
    fn report_ui(&mut self, ui: &mut egui::Ui) {
        if self.report.is_empty() {
            return;
        }
        egui::CollapsingHeader::new(format!("Rapport ({})", self.report.len()))
            .default_open(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(110.0)
                    .show(ui, |ui| {
                        for line in &self.report {
                            ui.label(RichText::new(line).small());
                        }
                    });
                if ui.small_button("Fermer le rapport").clicked() {
                    self.report.clear();
                }
            });
    }

    /// Replaces the editor's build, asking first when that would drop unsaved edits.
    fn request_replace(&mut self, pending: Pending) {
        if self.dirty() {
            self.confirm = Some(Confirm::Discard(pending));
        } else {
            self.apply_pending(pending);
        }
    }

    fn apply_pending(&mut self, pending: Pending) {
        match pending {
            Pending::New => self.load_build(Build::default()),
            Pending::Edit(i) => {
                if let Some(build) = self.store.builds.get(i).cloned() {
                    self.load_build(build);
                    self.tab = Tab::Editor;
                }
            }
            Pending::Import(build, issues) => self.adopt_import(build, issues),
        }
    }

    /// Imported builds count as unsaved: they only reach "Mes Builds" through Enregistrer.
    fn adopt_import(&mut self, build: Build, issues: Vec<Issue>) {
        self.load_build(build);
        self.saved = snapshot(&Build::default());
        self.status = "Build importé : pense à l'enregistrer.".into();
        if !issues.is_empty() {
            let name = self.build.name.clone();
            self.report = issue_lines(std::iter::once((name.as_str(), issues.as_slice())));
        }
        self.tab = Tab::Editor;
    }

    /// Title and the three tabs, in the top bar.
    fn nav_bar(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new()
            .fill(BG)
            .inner_margin(Margin::symmetric(14, 0))
            .stroke(Stroke::new(1.0, BORDER));
        egui::Panel::top("tabs").frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.available_width() > 640.0 {
                    ui.label(RichText::new("AO Comp Builder").heading().color(GOLD));
                    ui.add_space(16.0);
                }
                let tabs = [
                    (Tab::Editor, "Éditeur de build".to_owned()),
                    (
                        Tab::MyBuilds,
                        format!("Mes Builds ({})", self.store.builds.len()),
                    ),
                    (
                        Tab::Comps,
                        format!("Compositions ({})", self.comps.comps.len()),
                    ),
                ];
                for (tab, label) in tabs {
                    if nav_tab(ui, self.active_tab() == tab, &label).clicked() {
                        self.tab = tab;
                    }
                }
            });
        });
    }

    fn persist(&mut self) {
        if let Err(e) = self.store.save() {
            self.status = format!("Échec de l'enregistrement : {e}");
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.confirm.is_none()
            && matches!(self.dialog, Dialog::None)
            && self.tab == Tab::Editor
            && ui.input_mut(|i| i.consume_shortcut(&SAVE_SHORTCUT))
        {
            self.save_build();
        }

        self.nav_bar(ui);
        match self.tab {
            Tab::Editor => self.editor(ui),
            Tab::MyBuilds => {
                egui::CentralPanel::default().show(ui, |ui| self.my_builds(ui));
            }
            Tab::Comps => self.comps_tab(ui),
            Tab::BuildDetail => self.build_detail_tab(ui),
        }
        let ctx = ui.ctx().clone();
        self.confirmations(&ctx);
        self.dialogs(&ctx);
        self.export_window(&ctx);
        // Flush pending composition edits before the window closes.
        if ctx.input(|i| i.viewport().close_requested()) {
            self.save_comps();
        } else {
            self.autosave_comps(&ctx);
        }
    }
}

fn snapshot(build: &Build) -> String {
    serde_json::to_string(build).unwrap_or_default()
}

/// `base` if free, otherwise `base (2)`, `base (3)`, …
fn unique_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_owned();
    }
    (2..)
        .map(|n| format!("{base} ({n})"))
        .find(|candidate| !taken(candidate))
        .expect("unbounded range")
}

fn sanitize_file_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.trim_matches('_').is_empty() {
        "build".into()
    } else {
        clean
    }
}

fn issue_lines<'a>(groups: impl Iterator<Item = (&'a str, &'a [Issue])>) -> Vec<String> {
    groups
        .flat_map(|(name, issues)| {
            let name = if name.is_empty() { "(sans nom)" } else { name };
            issues.iter().map(move |i| format!("« {name} » : {i}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::comp::{Composition, Member};

    pub(super) fn named(name: &str) -> Build {
        Build {
            name: name.into(),
            ..Default::default()
        }
    }

    /// A store that never touches disk: the path does not exist, so `load` yields an empty one.
    pub(super) fn comps_with(members: &[(&str, Option<&str>)]) -> CompStore {
        let mut store = CompStore::load(PathBuf::from("does-not-exist/comps.json"));
        store.comps.push(Composition {
            name: "Comp".into(),
            members: members
                .iter()
                .map(|(role, build)| Member {
                    role: (*role).into(),
                    build: build.map(str::to_owned),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
        store
    }

    #[test]
    fn unique_name_appends_the_first_free_number() {
        let taken = ["Tank", "Tank (2)"];
        assert_eq!(unique_name("Heal", |n| taken.contains(&n)), "Heal");
        assert_eq!(unique_name("Tank", |n| taken.contains(&n)), "Tank (3)");
    }

    #[test]
    fn the_note_counts_as_an_unsaved_change() {
        let mut build = Build::default();
        let before = snapshot(&build);
        build.note = "Reste derrière le tank".into();
        assert_ne!(snapshot(&build), before);
    }
}
