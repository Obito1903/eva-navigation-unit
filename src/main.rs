//! a310 — Android Auto head unit with a Slint GUI.
//!
//! Architecture:
//!   • Main thread   : Slint event loop (required by most windowing systems)
//!   • Background    : std::thread → tokio::Runtime → android-auto protocol
//!   • Bridge        : mpsc channels + slint::invoke_from_event_loop
//!
//! Module layout:
//!   • [`messages`]  : message types crossing the async ↔ UI boundary
//!   • [`audio`]     : cpal output/input stream construction
//!   • [`video`]     : dedicated H.264 decoder thread
//!   • [`protocol`]  : the `AndroidAuto` handler + all android-auto trait impls
//!   • [`container`] : the background worker thread + channel plumbing
//!   • [`ui`]        : wiring Slint callbacks/timer to the worker
//!
//! Run with:
//!   cargo run --release
//!
//! The android-auto dependency is compiled with both the `usb` and `wireless`
//! features (see Cargo.toml).

mod audio;
mod config;
mod container;
mod controls;
mod gfx;
mod hostapd;
mod messages;
mod nmrs_extensions;
mod protocol;
mod ui;
mod video;

use slint::ComponentHandle;
use slint::Global;

slint::include_modules!();
fn main() -> Result<(), slint::PlatformError> {
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .init()
        .unwrap();

    let cfg = config::Config::load();
    log::info!(
        "DPI configured: current={} min={} max={}",
        cfg.dpi,
        cfg.min_dpi,
        cfg.max_dpi
    );
    log::info!("Wireless Android Auto: {}", cfg.wireless);
    log::info!("USB Android Auto: {}", cfg.usb);

    // Require an OpenGL(-ES) renderer so the wireframe-sphere underlay's
    // rendering notifier (which needs `GraphicsAPI::NativeOpenGL`) always
    // fires. A silent software-renderer fallback becomes a hard, visible
    // failure here instead of a missing 3D background.
    slint::BackendSelector::new()
        .require_opengl_es()
        .select()?;

    let setup = android_auto::setup();

    let window = AppWindow::new()?;
    window.set_aa_min_dpi(cfg.min_dpi);
    window.set_aa_max_dpi(cfg.max_dpi);
    window.set_aa_dpi(cfg.dpi);
    window.set_aa_wireless_enabled(cfg.wireless);
    window.set_aa_usb_enabled(cfg.usb);
    window.set_aa_resolution(cfg.resolution);
    window.set_aa_fps(cfg.fps);
    window.set_transition_mode(cfg.transition_mode);
    window.set_aa_video_transition_mode(cfg.aa_video_transition_mode);
    window.set_transition_speed(cfg.transition_speed);
    window.set_aa_video_transition_speed(cfg.aa_video_transition_speed);
    window.set_theme_id(cfg.theme);
    // Apply the persisted theme to the global palette at startup.
    Theme::get(&window).set_theme_id(cfg.theme);
    window.set_gfx_model(cfg.gfx_model);
    window.set_fullscreen(cfg.fullscreen);
    window.set_hotspot_backend(cfg.hotspot_backend);
    window.set_hotspot_channel(cfg.hotspot_channel);

    ui::wire(&window, setup, cfg);
    gfx::install(&window);
    window.run()
}
