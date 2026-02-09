#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod canvas;
mod preview;
mod capture;
mod window_picker;
mod persistence;
mod tray;

use app::PluriviewApp;
use eframe::egui;

fn main() -> eframe::Result<()> {
    env_logger::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 720.0])
            .with_min_inner_size([800.0, 600.0])
            .with_title("Pluriview"),
        ..Default::default()
    };

    eframe::run_native(
        "Pluriview",
        options,
        Box::new(|cc| Ok(Box::new(PluriviewApp::new(cc)))),
    )
}
