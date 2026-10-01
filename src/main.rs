// Hide the console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod args;
mod clip;
mod logging;
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
    logging::init();
    logging::write(concat!(
        "YT Downloader ",
        env!("CARGO_PKG_VERSION"),
        " starting"
    ));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(concat!("YT Downloader ", env!("CARGO_PKG_VERSION")))
            .with_inner_size([940.0, 640.0])
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };
    let result = eframe::run_native(
        "YT Downloader",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    );
    // E.g. no usable graphics driver: without a console, the log is the only trace.
    if let Err(e) = &result {
        logging::write(&format!("Could not open the window: {e}"));
    }
    result
}
