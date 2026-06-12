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
mod container;
mod messages;
mod nmrs_extensions;
mod protocol;
mod ui;
mod video;

use slint::ComponentHandle;

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .init()
        .unwrap();

    let setup = android_auto::setup();

    let window = AppWindow::new()?;
    ui::wire(&window, setup);
    window.run()
}
