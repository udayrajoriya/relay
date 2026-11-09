#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
mod assets;
mod desktop;
use anyhow::{Context, Result};
use relay::{bridge::Bridge, engine::Engine, profile};
use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        eprintln!("Relay: {error:#}");
        rfd::MessageDialog::new()
            .set_title("Relay could not start")
            .set_description(format!(
                "{error:#}\nPreserve your profile before attempting recovery."
            ))
            .set_level(rfd::MessageLevel::Error)
            .show();
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let mut data = profile::default_profile()?;
    let mut downloads = profile::default_downloads()?;
    let mut smoke = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--profile" => {
                data = PathBuf::from(args.next().context("--profile needs a directory")?)
            }
            "--downloads" => {
                downloads = PathBuf::from(args.next().context("--downloads needs a directory")?)
            }
            "--smoke-test" => {
                smoke = Some(PathBuf::from(
                    args.next()
                        .context("--smoke-test needs an output marker path")?,
                ))
            }
            "--version" => {
                println!("Relay {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!(
                    "Relay — download manager\n\nrelay [--profile DIRECTORY] [--downloads DIRECTORY]\nrelay --version"
                );
                return Ok(());
            }
            _ => anyhow::bail!("Unknown option: {arg}"),
        }
    }
    anyhow::ensure!(
        data.is_absolute() && downloads.is_absolute(),
        "Profile and download directories must be absolute."
    );
    let _lock = profile::ProfileLock::acquire(&data)?;
    let token = profile::pairing_token(&data)?;
    let extension_dir = data.join("extensions").join(env!("CARGO_PKG_VERSION"));
    assets::extract(&extension_dir)?;
    let runtime = tokio::runtime::Runtime::new()?;
    let _enter = runtime.enter();
    let engine = Engine::open(data, downloads)?;
    let (send, folders) = tokio::sync::mpsc::unbounded_channel();
    let bridge = runtime.block_on(Bridge::start(engine.clone(), token.clone(), 17864, send));
    let bridge_status = match &bridge {
        Ok(bridge) => format!("Listening on {}", bridge.address),
        Err(error) => format!("Browser bridge unavailable: {error}"),
    };
    let icon = image::load_from_memory(include_bytes!("assets/icon.png"))?.into_rgba8();
    let (width, height) = icon.dimensions();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Relay — Download Manager")
            .with_app_id("local.relay.downloads")
            .with_inner_size([1380.0, 960.0])
            .with_min_inner_size([1050.0, 700.0])
            .with_icon(eframe::egui::IconData {
                rgba: icon.into_raw(),
                width,
                height,
            }),
        ..Default::default()
    };
    let ui_engine = engine.clone();
    let result = eframe::run_native(
        "Relay",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(eframe::egui::Visuals::light());
            cc.egui_ctx.style_mut(|style| {
                style.spacing.button_padding = eframe::egui::vec2(10.0, 6.0);
                style.spacing.item_spacing = eframe::egui::vec2(10.0, 8.0);
                style.visuals.override_text_color =
                    Some(eframe::egui::Color32::from_rgb(43, 48, 60));
                style.visuals.selection.bg_fill = eframe::egui::Color32::from_rgb(222, 216, 250);
                style.visuals.selection.stroke.color = eframe::egui::Color32::from_rgb(82, 63, 155);
                style.visuals.window_fill = eframe::egui::Color32::WHITE;
                style.visuals.text_edit_bg_color =
                    Some(eframe::egui::Color32::from_rgb(241, 243, 247));
            });
            Ok(Box::new(desktop::Desktop::new(
                ui_engine,
                token,
                extension_dir,
                bridge_status,
                folders,
                smoke,
            )))
        }),
    );
    drop(bridge);
    runtime.block_on(engine.close())?;
    result.map_err(|error| anyhow::anyhow!(error.to_string()))
}
