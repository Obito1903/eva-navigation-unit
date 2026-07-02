//! Logging / debug pipeline.
//!
//! Builds a [`tracing`] subscriber with:
//!   • a console layer (always on),
//!   • an optional file layer (text or JSON) when `log.file` is configured,
//!   • optional, feature-gated layers for local visualisation
//!     (`chrome-trace` → Perfetto, `tokio-console` → live async view).
//!
//! Both this crate (`eva-navigation-unit`) and the `android-auto` crate emit through the
//! `log` facade / `tracing` macros; the `tracing-log` feature bridges `log`
//! records into `tracing`, so no changes are needed in `android-auto`.
//!
//! Filtering is per-component. Components map to module-path globs so log
//! call sites stay untouched:
//!
//! | Component | Module paths |
//! |-----------|--------------|
//! | UI    | `eva_navigation_unit::ui`, `eva_navigation_unit::controls`, `eva_navigation_unit::gfx` |
//! | Audio | `eva_navigation_unit::audio`, `android_auto::{mediaaudio,speechaudio,sysaudio}` |
//! | AA    | `eva_navigation_unit::container`, `eva_navigation_unit::protocol`, `android_auto::{lib,control,ssl,common,video}` |
//! | BT    | `eva_navigation_unit::hostapd`, `android_auto::{bluetooth,usb}` |

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, Layer, Registry, fmt};

use crate::config::{Config, LogConfig};

/// Module paths that make up the UI component.
const UI_MODULES: &[&str] = &[
    "eva_navigation_unit::ui",
    "eva_navigation_unit::controls",
    "eva_navigation_unit::gfx",
];
/// Module paths that make up the Audio component.
const AUDIO_MODULES: &[&str] = &[
    "eva_navigation_unit::audio",
    "android_auto::mediaaudio",
    "android_auto::speechaudio",
    "android_auto::sysaudio",
];
/// Module paths that make up the Android Auto (AA) component.
const AA_MODULES: &[&str] = &[
    "eva_navigation_unit::container",
    "eva_navigation_unit::protocol",
    "android_auto::lib",
    "android_auto::control",
    "android_auto::ssl",
    "android_auto::common",
    "android_auto::video",
];
/// Module paths that make up the Bluetooth/transport (BT) component.
const BT_MODULES: &[&str] = &[
    "eva_navigation_unit::hostapd",
    "android_auto::bluetooth",
    "android_auto::usb",
];

/// Guards that must be kept alive for the lifetime of the process so buffered
/// log output is flushed on exit. Dropping these flushes pending records.
#[derive(Default)]
pub(crate) struct LogGuards {
    _file: Option<WorkerGuard>,
    #[cfg(feature = "chrome-trace")]
    _chrome: Option<tracing_chrome::FlushGuard>,
}

/// Boxed per-layer alias to keep the layer vector heterogeneous.
type BoxLayer = Box<dyn Layer<Registry> + Send + Sync>;

/// Initialise the global tracing subscriber from the resolved configuration.
///
/// Returns guards that the caller must hold for the duration of the program.
pub(crate) fn init(cfg: &Config) -> LogGuards {
    let log = &cfg.log;
    let mut guards = LogGuards::default();
    let mut layers: Vec<BoxLayer> = Vec::new();

    // Console layer — always on.
    layers.push(
        fmt::layer()
            .with_target(true)
            .with_filter(build_filter(log))
            .boxed(),
    );

    // Optional file layer (opt-in via `log.file`).
    if let Some(path) = &log.file {
        match build_file_layer(log, path) {
            Ok((layer, guard)) => {
                guards._file = Some(guard);
                layers.push(layer);
            }
            Err(e) => eprintln!("Failed to set up log file {}: {e}", path.display()),
        }
    }

    #[cfg(feature = "chrome-trace")]
    {
        let (layer, guard) = tracing_chrome::ChromeLayerBuilder::new().build();
        guards._chrome = Some(guard);
        layers.push(layer.boxed());
    }

    #[cfg(feature = "tokio-console")]
    {
        layers.push(console_subscriber::spawn().boxed());
    }

    Registry::default().with(layers).init();
    guards
}

/// Build a non-blocking file layer (text or JSON) for the given path.
fn build_file_layer(
    log: &LogConfig,
    path: &std::path::Path,
) -> std::io::Result<(BoxLayer, WorkerGuard)> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let name = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("eva-ui.log"));
    std::fs::create_dir_all(dir)?;

    let appender = tracing_appender::rolling::never(dir, name);
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let layer: BoxLayer = if log.format.eq_ignore_ascii_case("json") {
        fmt::layer()
            .with_ansi(false)
            .with_writer(writer)
            .json()
            .with_filter(build_filter(log))
            .boxed()
    } else {
        fmt::layer()
            .with_ansi(false)
            .with_writer(writer)
            .with_filter(build_filter(log))
            .boxed()
    };

    Ok((layer, guard))
}

/// Translate the component-oriented [`LogConfig`] into an [`EnvFilter`].
///
/// `RUST_LOG`, when set, is layered on top so power users keep full control.
fn build_filter(log: &LogConfig) -> EnvFilter {
    let mut directives: Vec<String> = vec![log.level.clone()];

    push_component(&mut directives, &log.ui, UI_MODULES);
    push_component(&mut directives, &log.audio, AUDIO_MODULES);
    push_component(&mut directives, &log.aa, AA_MODULES);
    push_component(&mut directives, &log.bt, BT_MODULES);

    // `android_auto::usb` logs raw transfer bytes at info level, which is very
    // noisy. Quiet it by default unless the BT component level was set.
    if log.bt.is_none() {
        directives.push("android_auto::usb=warn".to_string());
    }

    let mut filter = EnvFilter::new(directives.join(","));

    if let Ok(env) = std::env::var("RUST_LOG") {
        for directive in env.split(',').filter(|d| !d.is_empty()) {
            match directive.parse() {
                Ok(parsed) => filter = filter.add_directive(parsed),
                Err(e) => eprintln!("Ignoring invalid RUST_LOG directive '{directive}': {e}"),
            }
        }
    }

    filter
}

/// Append `module=level` directives for each module in a component when an
/// override level is configured.
fn push_component(directives: &mut Vec<String>, level: &Option<String>, modules: &[&str]) {
    if let Some(level) = level {
        for module in modules {
            directives.push(format!("{module}={level}"));
        }
    }
}
