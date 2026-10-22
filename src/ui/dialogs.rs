//! Modal windows: import/export, renaming, adding a swap, and the shared confirmation.

use std::path::Path;

use eframe::egui::{self, Key, RichText, Vec2};

use crate::comp;
use crate::share;

use super::editor::add_swap_to;
use super::theme::{DANGER, GOLD};
use super::widgets::{danger_button, pick_item, primary_button};
use super::{App, Confirm, Dialog, Pending};

impl App {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        let mut dialog = std::mem::replace(&mut self.dialog, Dialog::None);
        let mut close = false;
        let mut imported = None;
        let mut comp_imported = None;
        let mut rename = None;
        let mut swap_picked: Option<String> = None;
        let catalog = &self.catalog;
        let icons = &self.icons;
        let modal = egui::Modal::new(egui::Id::unique("share_dialog"));
        match &mut dialog {
            Dialog::None => return,
            Dialog::Export {
                build,
                code,
                path,
                result,
            } => {
                let response = modal.show(ctx, |ui| {
                    ui.set_width(460.0);
                    ui.label(
                        RichText::new(format!("Exporter « {} »", build.name))
                            .heading()
                            .color(GOLD),
                    );
                    ui.label("Chaîne de partage :");
                    ui.add(
                        egui::TextEdit::multiline(&mut code.as_str())
                            .desired_rows(4)
                            .desired_width(f32::INFINITY),
                    );
                    if ui.add(primary_button("Copier la chaîne")).clicked() {
                        ctx.copy_text(code.clone());
                        *result = "Chaîne copiée dans le presse-papiers.".into();
                    }
                    ui.separator();
                    ui.label("Fichier .json :");
                    ui.add(egui::TextEdit::singleline(path).desired_width(f32::INFINITY));
                    if ui.button("Enregistrer le fichier").clicked() {
                        *result = match share::export_file(build, Path::new(path.as_str())) {
                            Ok(()) => format!("Fichier enregistré : {path}"),
                            Err(e) => format!("Échec de l'export : {e}"),
                        };
                    }
                    if !result.is_empty() {
                        ui.label(RichText::new(result.as_str()).small());
                    }
                    ui.add_space(6.0);
                    if ui.button("Fermer").clicked() {
                        close = true;
                    }
                });
                close |= response.should_close();
            }
            Dialog::Import { text, path, error } => {
                let response = modal.show(ctx, |ui| {
                    ui.set_width(460.0);
                    ui.label(RichText::new("Importer un build").heading().color(GOLD));
                    ui.label("Colle une chaîne de partage (AOB1:…) ou un JSON :");
                    ui.add(
                        egui::TextEdit::multiline(text)
                            .desired_rows(4)
                            .desired_width(f32::INFINITY),
                    );
                    if ui
                        .add_enabled(!text.trim().is_empty(), primary_button("Importer le texte"))
                        .clicked()
                    {
                        match share::import(text, catalog) {
                            Ok(done) => imported = Some(done),
                            Err(e) => *error = format!("{e:#}"),
                        }
                    }
                    ui.separator();
                    ui.label("Ou ouvre un fichier .json :");
                    ui.add(egui::TextEdit::singleline(path).desired_width(f32::INFINITY));
                    if ui
                        .add_enabled(!path.trim().is_empty(), primary_button("Ouvrir le fichier"))
                        .clicked()
                    {
                        match share::import_file(Path::new(path.trim()), catalog) {
                            Ok(done) => imported = Some(done),
                            Err(e) => *error = format!("{e:#}"),
                        }
                    }
                    if !error.is_empty() {
                        ui.colored_label(DANGER, error.as_str());
                    }
                    ui.add_space(6.0);
                    if ui.button("Annuler").clicked() {
                        close = true;
                    }
                });
                close |= response.should_close();
            }
            Dialog::ExportComp { name, code, result } => {
                let response = modal.show(ctx, |ui| {
                    ui.set_width(460.0);
                    ui.label(
                        RichText::new(format!("Exporter « {name} »"))
                            .heading()
                            .color(GOLD),
                    );
                    ui.label("Chaîne de partage (la composition et les builds qu'elle utilise) :");
                    ui.add(
                        egui::TextEdit::multiline(&mut code.as_str())
                            .desired_rows(5)
                            .desired_width(f32::INFINITY),
                    );
                    if ui.add(primary_button("Copier la chaîne")).clicked() {
                        ctx.copy_text(code.clone());
                        *result = "Chaîne copiée dans le presse-papiers.".into();
                    }
                    if !result.is_empty() {
                        ui.label(RichText::new(result.as_str()).small());
                    }
                    ui.add_space(6.0);
                    if ui.button("Fermer").clicked() {
                        close = true;
                    }
                });
                close |= response.should_close();
            }
            Dialog::ImportComp { text, error } => {
                let response = modal.show(ctx, |ui| {
                    ui.set_width(460.0);
                    ui.label(
                        RichText::new("Importer une composition")
                            .heading()
                            .color(GOLD),
                    );
                    ui.label("Colle une chaîne de partage (AOC1:…) :");
                    ui.add(
                        egui::TextEdit::multiline(text)
                            .desired_rows(5)
                            .desired_width(f32::INFINITY),
                    );
                    if ui
                        .add_enabled(!text.trim().is_empty(), primary_button("Importer"))
                        .clicked()
                    {
                        match comp::import(text, catalog) {
                            Ok(done) => comp_imported = Some(done),
                            Err(e) => *error = format!("{e:#}"),
                        }
                    }
                    if !error.is_empty() {
                        ui.colored_label(DANGER, error.as_str());
                    }
                    ui.add_space(6.0);
                    if ui.button("Annuler").clicked() {
                        close = true;
                    }
                });
                close |= response.should_close();
            }
            Dialog::RenameBuild { index, name, error } => {
                let response = modal.show(ctx, |ui| {
                    ui.set_width(360.0);
                    ui.label(RichText::new("Renommer le build").heading().color(GOLD));
                    let edit =
                        ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
                    let submitted = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if !error.is_empty() {
                        ui.colored_label(DANGER, error.as_str());
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.add(primary_button("Renommer")).clicked() || submitted {
                            rename = Some((*index, name.clone()));
                        }
                        if ui.button("Annuler").clicked() {
                            close = true;
                        }
                    });
                });
                close |= response.should_close();
            }
            Dialog::AddSwap { slot, filter } => {
                let response = modal.show(ctx, |ui| {
                    ui.set_width(700.0);
                    ui.label(
                        RichText::new(format!("Ajouter un swap — {}", slot.label()))
                            .heading()
                            .color(GOLD),
                    );
                    ui.allocate_ui(Vec2::new(700.0, 460.0), |ui| {
                        if let Some(item) = pick_item(ui, catalog, icons, *slot, filter, None) {
                            swap_picked = Some(item.id.clone());
                        }
                    });
                    ui.add_space(6.0);
                    if ui.button("Annuler").clicked() {
                        close = true;
                    }
                });
                close |= response.should_close();
            }
        }

        let mut keep = !close;
        if let Some(id) = swap_picked {
            if let Some(item) = self.catalog.item(&id) {
                add_swap_to(&mut self.build, item);
            }
            keep = false;
        }
        if let Some((build, issues)) = imported {
            self.request_replace(Pending::Import(build, issues));
            keep = false;
        }
        if let Some(done) = comp_imported {
            self.adopt_comp(done);
            keep = false;
        }
        if let Some((index, new)) = rename {
            match self.rename_build(index, &new) {
                Ok(()) => keep = false,
                Err(e) => {
                    if let Dialog::RenameBuild { error, .. } = &mut dialog {
                        *error = e;
                    }
                    keep = true;
                }
            }
        }
        if keep {
            self.dialog = dialog;
        }
    }

    /// Title, text and confirm-button label for a pending confirmation; `None` if its target is gone.
    pub(super) fn confirm_text(
        &self,
        confirm: &Confirm,
    ) -> Option<(&'static str, String, &'static str)> {
        match confirm {
            Confirm::DeleteBuild(i) => {
                let build = self.store.builds.get(*i)?;
                let used = self
                    .comps
                    .comps
                    .iter()
                    .filter(|c| c.build_names().contains(&build.name.as_str()))
                    .count();
                let mut text = format!("« {} » sera définitivement supprimé.", build.name);
                if used > 0 {
                    text.push_str(&format!(
                        "\n{used} composition(s) l'utilisent : leurs membres resteront sans build."
                    ));
                }
                Some(("Supprimer le build ?", text, "Supprimer"))
            }
            Confirm::DeleteComp(i) => {
                let comp = self.comps.comps.get(*i)?;
                Some((
                    "Supprimer la composition ?",
                    format!("« {} » sera définitivement supprimée.", comp.name),
                    "Supprimer",
                ))
            }
            Confirm::Discard(_) => Some((
                "Modifications non enregistrées",
                "Le build en cours a des modifications non enregistrées. Les abandonner ?".into(),
                "Abandonner",
            )),
        }
    }

    /// Modal shared by every destructive action; runs it only once the user agrees.
    pub(super) fn confirmations(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &self.confirm else { return };
        let Some((title, text, button)) = self.confirm_text(confirm) else {
            self.confirm = None;
            return;
        };
        let mut confirmed = false;
        let mut cancelled = false;
        egui::Modal::new(egui::Id::unique("confirm_action")).show(ctx, |ui| {
            ui.label(RichText::new(title).heading().color(GOLD));
            ui.label(text);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(danger_button(button)).clicked() {
                    confirmed = true;
                }
                if ui.button("Annuler").clicked() {
                    cancelled = true;
                }
            });
        });
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            cancelled = true;
        }
        if confirmed {
            match self.confirm.take() {
                Some(Confirm::DeleteBuild(i)) => self.delete_build(i),
                Some(Confirm::DeleteComp(i)) => self.delete_comp(i),
                Some(Confirm::Discard(pending)) => self.apply_pending(pending),
                None => {}
            }
        } else if cancelled {
            self.confirm = None;
        }
    }
}
