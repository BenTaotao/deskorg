#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod desktop;
mod fences;
mod shell;
mod single_instance;
mod theme;
mod widgets;

use std::sync::Arc;

fn main() -> eframe::Result {
    // 崩溃/panic 时把 DeskOrg 自己隐藏的桌面图标恢复，避免用户只剩空桌面。
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        shell::restore_icons_after_panic();
        default_hook(info);
    }));

    let args: Vec<String> = std::env::args().collect();
    let elevated = args.iter().any(|arg| arg == "--elevated");
    let start_background = args.iter().any(|arg| arg == "--background");

    if !single_instance::acquire(elevated) {
        shell::focus_existing_window();
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("DeskOrg · 桌面整理与美化")
            .with_inner_size([1040.0, 680.0])
            .with_min_inner_size([1000.0, 680.0])
            .with_icon(Arc::new(app_icon())),
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        "DeskOrg",
        options,
        Box::new(move |cc| Ok(Box::new(app::DeskOrgApp::new(cc, start_background)))),
    )
}

fn app_icon() -> eframe::egui::IconData {
    const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");
    let image = image::load_from_memory(ICON_PNG)
        .expect("bundled app icon is a valid image")
        .to_rgba8();
    let (width, height) = image.dimensions();
    eframe::egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}
