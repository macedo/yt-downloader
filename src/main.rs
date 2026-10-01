// Hide the console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod args;
mod clip;
mod media;
mod net;
mod process;
mod settings;
mod update;

#[cfg(test)]
mod live_tests;

use eframe::egui;

use app::App;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("YT Downloader")
            .with_inner_size([940.0, 640.0])
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "YT Downloader",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
