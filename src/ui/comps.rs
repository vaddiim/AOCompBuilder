//! The "Compositions" tab: list, member editor, import/export and autosave of `comps.json`.

use eframe::egui::{self, Margin, RichText};

use crate::build::Build;
use crate::comp::{self, CompStore, Composition, Member};
use crate::export_image::Job;

use super::theme::{BG, DANGER, GOLD, HIGHLIGHT, MUTED, PANEL, TEXT, card_frame};
use super::widgets::{badge, hover_outline, mannequin_preview, primary_button};
use super::{App, Confirm, Dialog, issue_lines, unique_name};

/// Seconds without further edits before compositions are written to disk.
pub(super) const COMP_SAVE_DELAY: f64 = 0.8;

impl App {
    pub(super) fn comps_tab(&mut self, ui: &mut egui::Ui) {
        let side = egui::Frame::new()
            .fill(PANEL)
            .inner_margin(Margin::same(12));
        egui::Panel::left("comp_list")
            .resizable(true)
            .default_size(250.0)
            .size_range(200.0..=380.0)
            .frame(side)
            .show(ui, |ui| {
                ui.label(RichText::new("Compositions").heading().color(GOLD));
                ui.horizontal_wrapped(|ui| {
                    if ui.add(primary_button("Nouvelle")).clicked() {
                        self.new_comp();
                    }
                    if ui.button("Importer").clicked() {
                        self.dialog = Dialog::ImportComp {
                            text: String::new(),
                            error: String::new(),
                        };
                    }
                });
                if !self.comp_status.is_empty() {
                    ui.label(RichText::new(&self.comp_status).small().color(MUTED));
                }
                self.report_ui(ui);
                ui.add_space(6.0);
                if self.comps.comps.is_empty() {
                    ui.label(
                        RichText::new(
                            "Aucune composition. Crée-en une ou importe une chaîne AOC1.",
                        )
                        .color(MUTED),
                    );
                }
                egui::ScrollArea::vertical()
                    .id_salt("comp_list_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (i, comp) in self.comps.comps.iter().enumerate() {
                            let missing = comp
                                .check(&self.store.builds)
                                .iter()
                                .any(|c| matches!(c, comp::CompIssue::MissingBuild { .. }));
                            let selected = self.comp_sel == Some(i);
                            let name = if comp.name.trim().is_empty() {
                                "(sans nom)"
                            } else {
                                comp.name.as_str()
                            };
                            let scope = ui.scope_builder(
                                egui::UiBuilder::new().sense(egui::Sense::click()),
                                |ui| {
                                    card_frame(selected).inner_margin(Margin::same(8)).show(
                                        ui,
                                        |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new(name)
                                                        .strong()
                                                        .color(if selected { GOLD } else { TEXT }),
                                                );
                                                if missing {
                                                    badge(ui, "build manquant", DANGER);
                                                }
                                            });
                                            ui.label(
                                                RichText::new(format!(
                                                    "{} membre(s)",
                                                    comp.members.len()
                                                ))
                                                .small()
                                                .color(MUTED),
                                            );
                                        },
                                    );
                                },
                            );
                            let response = scope
                                .response
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            if response.hovered() && !selected {
                                hover_outline(ui, &response);
                            }
                            if response.clicked() {
                                self.comp_sel = Some(i);
                            }
                            ui.add_space(4.0);
                        }
                    });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::same(14)))
            .show(ui, |ui| self.comp_editor(ui));
    }

    pub(super) fn comp_editor(&mut self, ui: &mut egui::Ui) {
        let Some(index) = self.comp_sel.filter(|&i| i < self.comps.comps.len()) else {
            ui.weak("Sélectionne une composition dans la liste, ou crée-en une.");
            return;
        };
        let duplicate_name = {
            let name = &self.comps.comps[index].name;
            self.comps
                .comps
                .iter()
                .enumerate()
                .any(|(j, c)| j != index && &c.name == name)
        };
        let issues = self.comps.comps[index].check(&self.store.builds);

        let mut action = None;
        enum Action {
            Duplicate,
            Export,
            Image,
            Delete,
        }
        let catalog = &self.catalog;
        let icons = &self.icons;
        let builds = &self.store.builds;
        let comp = &mut self.comps.comps[index];

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut comp.name)
                    .hint_text("Nom de la composition")
                    .font(egui::TextStyle::Heading)
                    .desired_width(320.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(RichText::new("Supprimer").color(DANGER)))
                    .clicked()
                {
                    action = Some(Action::Delete);
                }
                if ui.button("Exporter").clicked() {
                    action = Some(Action::Export);
                }
                if ui
                    .button("Image")
                    .on_hover_text("Exporter la composition en image PNG")
                    .clicked()
                {
                    action = Some(Action::Image);
                }
                if ui.button("Dupliquer").clicked() {
                    action = Some(Action::Duplicate);
                }
            });
        });
        if duplicate_name {
            ui.colored_label(
                HIGHLIGHT,
                "Une autre composition porte déjà ce nom : l'import et l'export les confondraient.",
            );
        }
        for issue in &issues {
            ui.colored_label(HIGHLIGHT, format!("⚠ {issue}"));
        }
        ui.add(
            egui::TextEdit::multiline(&mut comp.notes)
                .hint_text("Notes générales (stratégie, contenu visé…)")
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        ui.separator();

        enum MemberOp {
            Up(usize),
            Down(usize),
            Remove(usize),
        }
        let mut op = None;
        let mut open_detail = None;
        let count = comp.members.len();
        egui::ScrollArea::vertical()
            .id_salt("members")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (k, member) in comp.members.iter_mut().enumerate() {
                    let linked = member
                        .build
                        .as_deref()
                        .and_then(|n| builds.iter().find(|b| b.name == n));
                    // The click area is registered before the card's content, so the buttons, fields and
                    // icon tooltips inside it keep working; only the free space opens the build page.
                    let scope = ui.scope_builder(
                        egui::UiBuilder::new().sense(egui::Sense::click()),
                        |ui| {
                            card_frame(false).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal_top(|ui| {
                                    ui.vertical(|ui| match linked {
                                        Some(build) => {
                                            mannequin_preview(ui, catalog, icons, build, 30.0)
                                        }
                                        None => {
                                            ui.add_sized(
                                                [98.0, 60.0],
                                                egui::Label::new(
                                                    RichText::new("Pas de build")
                                                        .small()
                                                        .color(MUTED),
                                                ),
                                            );
                                        }
                                    });
                                    ui.add_space(10.0);
                                    ui.vertical(|ui| {
                                        ui.horizontal(|ui| {
                                            ui.add(
                                                egui::TextEdit::singleline(&mut member.role)
                                                    .hint_text("Rôle (Tank, Soigneur…)")
                                                    .text_color(GOLD)
                                                    .desired_width(150.0),
                                            );
                                            ui.add(
                                                egui::TextEdit::singleline(&mut member.player)
                                                    .hint_text("Joueur")
                                                    .desired_width(130.0),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    if ui
                                                        .small_button("✕")
                                                        .on_hover_text("Retirer ce membre")
                                                        .clicked()
                                                    {
                                                        op = Some(MemberOp::Remove(k));
                                                    }
                                                    if ui
                                                        .add_enabled(
                                                            k + 1 < count,
                                                            egui::Button::new("▼").small(),
                                                        )
                                                        .clicked()
                                                    {
                                                        op = Some(MemberOp::Down(k));
                                                    }
                                                    if ui
                                                        .add_enabled(
                                                            k > 0,
                                                            egui::Button::new("▲").small(),
                                                        )
                                                        .clicked()
                                                    {
                                                        op = Some(MemberOp::Up(k));
                                                    }
                                                },
                                            );
                                        });
                                        ui.horizontal(|ui| {
                                            build_picker(
                                                ui,
                                                ("member_build", index, k),
                                                member,
                                                builds,
                                            );
                                            if let Some(name) = &member.build
                                                && ui
                                                    .small_button("Détail ›")
                                                    .on_hover_text("Ouvrir la page de ce build")
                                                    .clicked()
                                            {
                                                open_detail = Some((k, name.clone()));
                                            }
                                        });
                                        ui.add(
                                            egui::TextEdit::singleline(&mut member.note)
                                                .hint_text("Note")
                                                .desired_width(f32::INFINITY),
                                        );
                                        if linked.is_none() && member.build.is_some() {
                                            ui.colored_label(
                                                DANGER,
                                                "⚠ Ce build n'existe plus dans Mes Builds.",
                                            );
                                        }
                                    });
                                });
                            });
                        },
                    );
                    let response = scope
                        .response
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if linked.is_some() && response.hovered() {
                        hover_outline(ui, &response);
                    }
                    if response.clicked()
                        && linked.is_some()
                        && let Some(name) = &member.build
                    {
                        open_detail = Some((k, name.clone()));
                    }
                    ui.add_space(6.0);
                }
                if ui.add(primary_button("＋ Ajouter un membre")).clicked() {
                    comp.members.push(Member::default());
                }
            });
        match op {
            Some(MemberOp::Up(k)) => comp.members.swap(k, k - 1),
            Some(MemberOp::Down(k)) => comp.members.swap(k, k + 1),
            Some(MemberOp::Remove(k)) => {
                comp.members.remove(k);
            }
            None => {}
        }
        if let Some((k, name)) = open_detail {
            self.open_detail(&name, Some((index, k)));
            return;
        }

        match action {
            Some(Action::Duplicate) => self.duplicate_comp(index),
            Some(Action::Image) => {
                let job = Job::comp(
                    &self.comps.comps[index],
                    &self.store.builds,
                    &self.catalog,
                    &self.export_paths(),
                );
                self.start_export(ui.ctx(), job);
            }
            Some(Action::Export) => {
                let comp = &self.comps.comps[index];
                self.dialog = Dialog::ExportComp {
                    name: comp.name.clone(),
                    code: comp::encode(comp, &self.store.builds),
                    result: String::new(),
                };
            }
            Some(Action::Delete) => self.confirm = Some(Confirm::DeleteComp(index)),
            None => {}
        }
    }

    pub(super) fn save_comps(&mut self) {
        match self.comps.save() {
            Ok(()) => self.comps_saved = comps_snapshot(&self.comps),
            Err(e) => {
                self.comp_status = format!("Échec de l'enregistrement des compositions : {e}")
            }
        }
        self.comps_dirty_since = None;
    }

    /// Writes compositions shortly after the last edit instead of on every keystroke.
    pub(super) fn autosave_comps(&mut self, ctx: &egui::Context) {
        if comps_snapshot(&self.comps) == self.comps_saved {
            self.comps_dirty_since = None;
            return;
        }
        let now = ctx.input(|i| i.time);
        let since = *self.comps_dirty_since.get_or_insert(now);
        if now - since >= COMP_SAVE_DELAY {
            self.save_comps();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    pub(super) fn new_comp(&mut self) {
        let name = unique_name("Nouvelle composition", |n| {
            self.comps.comps.iter().any(|c| c.name == n)
        });
        self.comps.comps.push(Composition {
            name,
            ..Default::default()
        });
        self.comp_sel = Some(self.comps.comps.len() - 1);
        self.save_comps();
    }

    pub(super) fn duplicate_comp(&mut self, index: usize) {
        let Some(original) = self.comps.comps.get(index) else {
            return;
        };
        let mut copy = original.clone();
        copy.name = unique_name(&format!("{} (copie)", copy.name), |n| {
            self.comps.comps.iter().any(|c| c.name == n)
        });
        self.comps.comps.insert(index + 1, copy);
        self.comp_sel = Some(index + 1);
        self.save_comps();
    }

    pub(super) fn delete_comp(&mut self, index: usize) {
        if index >= self.comps.comps.len() {
            return;
        }
        self.comps.comps.remove(index);
        self.comp_sel = match self.comp_sel {
            Some(s) if s == index => None,
            Some(s) if s > index => Some(s - 1),
            other => other,
        };
        self.save_comps();
    }

    /// Adds an imported composition and its builds, then saves both stores.
    pub(super) fn adopt_comp(&mut self, import: comp::CompImport) {
        let report = issue_lines(
            import
                .issues
                .iter()
                .map(|(name, issues)| (name.as_str(), issues.as_slice())),
        );
        let build_count = import.builds.len();
        let mut comp = import.apply(&mut self.store);
        let base = if comp.name.is_empty() {
            "Composition importée"
        } else {
            comp.name.as_str()
        };
        comp.name = unique_name(base, |n| self.comps.comps.iter().any(|c| c.name == n));
        self.comp_status = format!(
            "Composition « {} » importée ({build_count} build(s) reçus).",
            comp.name
        );
        self.comps.comps.push(comp);
        self.comp_sel = Some(self.comps.comps.len() - 1);
        self.persist();
        self.save_comps();
        if !report.is_empty() {
            self.report = report;
        }
    }
}

pub(super) fn comps_snapshot(comps: &CompStore) -> String {
    serde_json::to_string(&comps.comps).unwrap_or_default()
}

/// Build chooser for a composition member: "no build", every saved build, or the dangling name.
pub(super) fn build_picker(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    member: &mut Member,
    builds: &[Build],
) {
    let none = "— aucun build —";
    let selected = match &member.build {
        None => none.to_owned(),
        Some(name) if builds.iter().any(|b| &b.name == name) => name.clone(),
        Some(name) => format!("⚠ {name} (introuvable)"),
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected)
        .width(220.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut member.build, None, none);
            for build in builds {
                ui.selectable_value(&mut member.build, Some(build.name.clone()), &build.name);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::super::tests::comps_with;
    use super::*;

    #[test]
    fn comps_snapshot_changes_with_every_edit() {
        let mut comps = comps_with(&[("tank", None)]);
        let before = comps_snapshot(&comps);
        comps.comps[0].members[0].player = "Alice".into();
        assert_ne!(comps_snapshot(&comps), before);
    }
}
