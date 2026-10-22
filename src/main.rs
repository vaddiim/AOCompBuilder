mod build;
mod catalog;
mod comp;
mod export_image;
mod share;
mod ui;

use anyhow::Result;

fn main() -> Result<()> {
    let root = std::env::current_dir()?;
    let assets = root.join("assets");
    let catalog = catalog::Catalog::load(&assets.join("data"))?;
    let store = build::BuildStore::load(root.join("userdata").join("builds.json"));
    let comps = comp::CompStore::load(root.join("userdata").join("comps.json"));

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "AO Comp Builder",
        options,
        Box::new(|cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            ui::install_theme(&cc.egui_ctx, &assets);
            Ok(Box::new(ui::App::new(catalog, store, comps, assets)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
