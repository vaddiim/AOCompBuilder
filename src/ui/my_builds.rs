//! "Mes Builds": the saved builds as cards, and renaming/deleting them.

use eframe::egui::{self, RichText};

use crate::build::Build;
use crate::comp::CompStore;
use crate::export_image::Job;

use super::theme::{DANGER, GOLD, MUTED, card_frame};
use super::widgets::{mannequin_preview, primary_button, spell_strip};
use super::{App, Confirm, Dialog, Pending, snapshot, unique_name};

impl App {
    pub(super) fn my_builds(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Mes Builds").heading().color(GOLD));
        ui.add_space(4.0);
        if self.store.builds.is_empty() {
            ui.weak("Aucun build enregistré. Crée-en un dans l'éditeur.");
            return;
        }

        enum Action {
            Edit(usize),
            Duplicate(usize),
            Delete(usize),
            Export(usize),
            Rename(usize),
            Detail(usize),
            Image(usize),
        }
        let mut action = None;

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for (idx, build) in self.store.builds.iter().enumerate() {
                    card_frame(false).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_top(|ui| {
                            ui.vertical(|ui| {
                                mannequin_preview(ui, &self.catalog, &self.icons, build, 46.0)
                            });
                            ui.add_space(12.0);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&build.name).heading().color(GOLD));
                                if !build.note.trim().is_empty() {
                                    ui.label(RichText::new(&build.note).small().color(MUTED));
                                }
                                spell_strip(ui, &self.catalog, &self.icons, build, 30.0);
                                ui.add_space(6.0);
                                ui.horizontal_wrapped(|ui| {
                                    if ui.add(primary_button("Modifier")).clicked() {
                                        action = Some(Action::Edit(idx));
                                    }
                                    if ui
                                        .button("Détails")
                                        .on_hover_text("Sorts et passifs choisis, swaps")
                                        .clicked()
                                    {
                                        action = Some(Action::Detail(idx));
                                    }
                                    if ui
                                        .button("Image")
                                        .on_hover_text("Exporter en image PNG")
                                        .clicked()
                                    {
                                        action = Some(Action::Image(idx));
                                    }
                                    if ui.button("Dupliquer").clicked() {
                                        action = Some(Action::Duplicate(idx));
                                    }
                                    if ui.button("Renommer").clicked() {
                                        action = Some(Action::Rename(idx));
                                    }
                                    if ui.button("Exporter").clicked() {
                                        action = Some(Action::Export(idx));
                                    }
                                    if ui
                                        .add(egui::Button::new(
                                            RichText::new("Supprimer").color(DANGER),
                                        ))
                                        .clicked()
                                    {
                                        action = Some(Action::Delete(idx));
                                    }
                                });
                            });
                        });
                    });
                    ui.add_space(8.0);
                }
            });

        match action {
            Some(Action::Edit(i)) => self.request_replace(Pending::Edit(i)),
            Some(Action::Duplicate(i)) => {
                let mut copy = self.store.builds[i].clone();
                copy.name = unique_name(&format!("{} (copie)", copy.name), |n| {
                    self.store.builds.iter().any(|b| b.name == n)
                });
                self.store.builds.insert(i + 1, copy);
                self.persist();
            }
            Some(Action::Detail(i)) => {
                let name = self.store.builds[i].name.clone();
                self.open_detail(&name, None);
            }
            Some(Action::Image(i)) => {
                let job = Job::build(&self.store.builds[i], &self.catalog, &self.export_paths());
                self.start_export(ui.ctx(), job);
            }
            Some(Action::Delete(i)) => self.confirm = Some(Confirm::DeleteBuild(i)),
            Some(Action::Export(i)) => self.open_export(self.store.builds[i].clone()),
            Some(Action::Rename(i)) => {
                let name = self.store.builds[i].name.clone();
                self.dialog = Dialog::RenameBuild {
                    index: i,
                    name,
                    error: String::new(),
                };
            }
            None => {}
        }
    }

    pub(super) fn rename_build(&mut self, index: usize, new: &str) -> Result<(), String> {
        let (old, updated) = rename_in(&mut self.store.builds, &mut self.comps, index, new)?;
        let new = new.trim();
        if self.build.name == old {
            self.build.name = new.to_owned();
            self.saved = rename_in_snapshot(&self.saved, new);
        }
        self.persist();
        self.status = format!("Build « {old} » renommé en « {new} ».");
        if updated > 0 {
            self.comp_status = format!("{updated} membre(s) de composition mis à jour.");
            self.save_comps();
        }
        Ok(())
    }

    pub(super) fn delete_build(&mut self, index: usize) {
        if index >= self.store.builds.len() {
            return;
        }
        let removed = self.store.builds.remove(index);
        let unlinked = self.comps.unlink_build(&removed.name);
        self.persist();
        if unlinked > 0 {
            self.comp_status = format!(
                "{unlinked} membre(s) de composition sans build après la suppression de « {} ».",
                removed.name
            );
            self.save_comps();
        }
    }
}

/// The saved-build snapshot after the build it describes was renamed.
pub(super) fn rename_in_snapshot(saved: &str, new: &str) -> String {
    match serde_json::from_str::<Build>(saved) {
        Ok(mut build) => {
            build.name = new.to_owned();
            snapshot(&build)
        }
        Err(_) => saved.to_owned(),
    }
}

/// Renames `builds[index]` and the compositions pointing at it. Returns the old name and how many
/// composition members were updated, or a message for the user when the new name is not usable.
pub(super) fn rename_in(
    builds: &mut [Build],
    comps: &mut CompStore,
    index: usize,
    new: &str,
) -> Result<(String, usize), String> {
    let new = new.trim();
    let old = builds.get(index).ok_or("Build introuvable.")?.name.clone();
    if new.is_empty() {
        return Err("Le nom ne peut pas être vide.".into());
    }
    if new != old && builds.iter().any(|b| b.name == new) {
        return Err(format!("Un build « {new} » existe déjà."));
    }
    builds[index].name = new.to_owned();
    let updated = if new == old {
        0
    } else {
        comps.rename_build(&old, new)
    };
    Ok((old, updated))
}

#[cfg(test)]
mod tests {
    use super::super::tests::{comps_with, named};
    use super::*;

    #[test]
    fn rename_updates_the_build_and_the_members_using_it() {
        let mut builds = vec![named("A"), named("B")];
        let mut comps = comps_with(&[("tank", Some("A")), ("heal", Some("B")), ("dps", None)]);
        let (old, updated) = rename_in(&mut builds, &mut comps, 0, "  Tank 2 ").unwrap();
        assert_eq!((old.as_str(), updated), ("A", 1));
        assert_eq!(builds[0].name, "Tank 2");
        let members = &comps.comps[0].members;
        assert_eq!(members[0].build.as_deref(), Some("Tank 2"));
        assert_eq!(members[1].build.as_deref(), Some("B"));
        assert!(
            comps.check_all(&builds).is_empty(),
            "no member left pointing at a missing build"
        );
    }

    #[test]
    fn rename_rejects_empty_and_taken_names_without_changes() {
        let mut builds = vec![named("A"), named("B")];
        let mut comps = comps_with(&[("tank", Some("A"))]);
        assert!(rename_in(&mut builds, &mut comps, 0, "   ").is_err());
        assert!(rename_in(&mut builds, &mut comps, 0, "B").is_err());
        assert!(rename_in(&mut builds, &mut comps, 9, "C").is_err());
        assert_eq!(builds[0].name, "A");
        assert_eq!(comps.comps[0].members[0].build.as_deref(), Some("A"));
        // Keeping the same name is accepted and touches nothing.
        assert_eq!(rename_in(&mut builds, &mut comps, 0, "A").unwrap().1, 0);
    }

    #[test]
    fn renamed_snapshot_matches_a_build_with_the_new_name() {
        let before = named("A");
        let after = named("Z");
        assert_eq!(
            rename_in_snapshot(&snapshot(&before), "Z"),
            snapshot(&after)
        );
        assert_eq!(rename_in_snapshot("not json", "Z"), "not json");
    }
}
