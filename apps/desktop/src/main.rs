#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod app;
mod audio;

// The core lives in `lumen-core`; re-exported at the crate root so `crate::engine::..` paths keep working.
pub(crate) use lumen_core::{camera, config, engine, imaging, link, logfile, ptp};

fn main() -> eframe::Result {
    logfile::init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lumen Tether Studio")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1180.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native("Lumen Tether Studio", options, Box::new(|cc| Ok(Box::new(app::LumenApp::new(cc)))))
}
