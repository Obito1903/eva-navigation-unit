//! eva-navigation-unit — Android Auto head unit with a Slint GUI.
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
#[cfg(feature = "jamesdsp")]
mod jamesdsp;
mod logging;
mod messages;
#[cfg(feature = "networkmanager-hotspot")]
mod nmrs_extensions;
mod protocol;
mod spectrum;
mod ui;
mod video;
mod visualizer;

use slint::ComponentHandle;
use slint::Global;
use std::sync::atomic::AtomicI32;
use std::sync::Arc;

slint::include_modules!();
fn main() -> Result<(), slint::PlatformError> {
    let cfg = config::Config::load();
    let _log_guards = logging::init(&cfg);
    log::info!(
        "eva-navigation-unit v{} starting — wireless={}, usb={}",
        env!("CARGO_PKG_VERSION"),
        cfg.wireless,
        cfg.usb
    );
    log::debug!(
        "Video config: {}p@{}fps, dpi current={} min={} max={}",
        cfg.resolution,
        cfg.fps,
        cfg.dpi,
        cfg.min_dpi,
        cfg.max_dpi
    );

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
    // Lets the DSP/EQ settings tab tell "not compiled in" apart from "not
    // currently running" (`dsp-connected`, updated live from ui.rs).
    window.set_dsp_available(cfg!(feature = "jamesdsp"));
    window.set_car_name_short(cfg.car_name_short.as_str().into());
    window.set_app_name(cfg.app_name.as_str().into());
    window.set_car_name_long(cfg.car_name_long.as_str().into());
    window.set_aa_waiting_text(cfg.aa_waiting_text.as_str().into());
    // Always reflect the actual build version rather than a configurable
    // value, so it can't drift from what was actually built.
    window.set_aa_version_text(env!("CARGO_PKG_VERSION").into());

    // Start audio capture. The consumer is passed directly to gfx::install
    // which moves it into VisualizerSystem on first VIZ view activation.
    let viz_cfg = Arc::new(cfg.viz.clone());
    let (_spectrum_capture, consumer) = spectrum::start_capture(&cfg.viz);
    let viz_renderer_id = Arc::new(AtomicI32::new(0));
    let viz_theme = Arc::new(AtomicI32::new(0));

    ui::wire(&window, setup, cfg, viz_renderer_id.clone(), viz_theme.clone());
    gfx::install(&window, consumer, viz_renderer_id, viz_theme, viz_cfg);
    window.run()
}
