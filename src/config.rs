//! Runtime configuration for eva-navigation-unit.
//!
//! Values are resolved with the following precedence (highest wins):
//!   1. CLI arguments        (e.g. `--min-dpi 120`)
//!   2. Environment variables (`EVA_MIN_DPI`, `EVA_MAX_DPI`)
//!   3. Config file (TOML)   (`--config <path>` or `EVA_CONFIG`; else a local
//!      `config.toml` in the working directory when present, otherwise the
//!      per-user `~/.config/eva-ui/config.toml`)
//!   4. Built-in defaults

use std::path::PathBuf;

use clap::Parser;
use serde::{Deserialize, Serialize};

/// Default minimum selectable Android Auto DPI.
pub(crate) const DEFAULT_MIN_DPI: i32 = 80;
/// Default maximum selectable Android Auto DPI.
pub(crate) const DEFAULT_MAX_DPI: i32 = 320;
/// Default current Android Auto DPI.
pub(crate) const DEFAULT_DPI: i32 = 160;
/// Default view transition mode (0 = CRT | 1 = FADE | 2 = SLIDE).
pub(crate) const DEFAULT_TRANSITION_MODE: i32 = 0;
/// Default Android Auto video start/stop transition (0 = CRT | 1 = FADE | 2 = SLIDE).
pub(crate) const DEFAULT_AA_VIDEO_TRANSITION_MODE: i32 = 1;
/// Default transition speed multiplier (1.0 = base timings; higher = faster).
pub(crate) const DEFAULT_TRANSITION_SPEED: f32 = 1.0;
/// Default Android Auto video transition speed multiplier.
pub(crate) const DEFAULT_AA_VIDEO_TRANSITION_SPEED: f32 = 1.0;
/// Minimum / maximum selectable transition speed multiplier.
pub(crate) const MIN_TRANSITION_SPEED: f32 = 0.25;
pub(crate) const MAX_TRANSITION_SPEED: f32 = 3.0;
/// Whether wireless Android Auto is enabled by default.
pub(crate) const DEFAULT_WIRELESS: bool = true;
/// Whether USB (wired) Android Auto is enabled by default.
pub(crate) const DEFAULT_USB: bool = true;
/// Whether to reset a USB phone left in AOA accessory mode at startup. This
/// clears a stale Android Auto session inherited from a previous run (the
/// programmatic equivalent of unplugging/replugging). Disable on controllers
/// that misbehave on USB reset (e.g. the Nintendo Switch Tegra xHCI).
pub(crate) const DEFAULT_RESET_STALE_ACCESSORY: bool = true;
/// Default Android Auto video resolution (vertical lines: 720 or 1080).
pub(crate) const DEFAULT_RESOLUTION: i32 = 720;
/// Default Android Auto video frame rate (30 or 60 fps).
pub(crate) const DEFAULT_FPS: i32 = 30;
/// Default color theme (0 = NERV-HQ | 1 = MATRIX).
pub(crate) const DEFAULT_THEME: i32 = 0;
/// Default GL underlay wireframe model (0 = sphere | 1 = cube | 2 = car | 3 = speaker).
pub(crate) const DEFAULT_GFX_MODEL: i32 = 0;
/// Whether the window starts in fullscreen mode by default.
pub(crate) const DEFAULT_FULLSCREEN: bool = false;
/// Default Wi-Fi hotspot backend (0 = NetworkManager | 1 = hostapd).
pub(crate) const DEFAULT_HOTSPOT_BACKEND: i32 = 0;
/// Default 5 GHz channel used by the hostapd hotspot backend (0 = automatic).
/// Ignored by the NetworkManager backend.
pub(crate) const DEFAULT_HOTSPOT_CHANNEL: i32 = 36;
/// Default global log level (`error` | `warn` | `info` | `debug` | `trace`).
pub(crate) const DEFAULT_LOG_LEVEL: &str = "info";
/// Default log output format (`text` | `json`).
pub(crate) const DEFAULT_LOG_FORMAT: &str = "text";
/// Default sidebar header/branding text.
pub(crate) const DEFAULT_CAR_NAME_SHORT: &str = "EVA-02";
/// Default Android Auto "locked terminal" overlay app name text.
pub(crate) const DEFAULT_APP_NAME: &str = "EVA NAVIGATION UNIT ";
/// Default Android Auto "locked terminal" overlay long car name text.
pub(crate) const DEFAULT_CAR_NAME_LONG: &str = "EVA-02";
/// Default Android Auto "locked terminal" overlay waiting-for-connection text.
pub(crate) const DEFAULT_AA_WAITING_TEXT: &str = "WAITING FOR ENTRY PLUG";

// ── Spectrum visualizer defaults ─────────────────────────────────────────────

/// Number of frequency bands shown by the visualizer.
pub(crate) const DEFAULT_VIZ_BANDS: u32 = 32;
/// Maximum band count (also the SpectrumData array size).
pub(crate) const MAX_VIZ_BANDS: u32 = 64;
/// FFT window size in samples (power of two).
pub(crate) const DEFAULT_VIZ_FFT_SIZE: u32 = 2048;
/// Hop size in samples — how many new samples trigger one FFT update.
/// Smaller = faster response, higher CPU cost. Must be ≤ fft_size / 2.
pub(crate) const DEFAULT_VIZ_HOP: u32 = 256;
/// Lowest frequency included in the spectrum display (Hz).
pub(crate) const DEFAULT_VIZ_FREQ_MIN: f32 = 20.0;
/// Highest frequency included in the spectrum display (Hz).
pub(crate) const DEFAULT_VIZ_FREQ_MAX: f32 = 20_000.0;
/// Input pre-smoother attack time constant (ms).
/// Controls how quickly bars rise in response to audio transients.
pub(crate) const DEFAULT_VIZ_INPUT_ATTACK_MS: f32 = 20.0;
/// Input pre-smoother release time constant (ms).
/// Controls how fast noise between FFT frames is suppressed on the falling edge.
pub(crate) const DEFAULT_VIZ_INPUT_RELEASE_MS: f32 = 60.0;
/// Gravity fall-speed multiplier (1.0 = CAVA default).
/// Higher values make bars fall faster after a transient.
pub(crate) const DEFAULT_VIZ_GRAVITY: f32 = 1.0;
/// Integral (leaky-integrator) noise-reduction factor override.
/// 0.0 = automatic calibration from the measured framerate (recommended).
/// Values in (0, 1) override the auto value: higher = heavier bars / more smoothing.
pub(crate) const DEFAULT_VIZ_NOISE_REDUCTION: f32 = 0.0;
/// Horizontal gap between bar columns as a fraction of the slot width (0.0–0.45).
pub(crate) const DEFAULT_VIZ_BAR_GAP: f32 = 0.08;
/// Vertical gap between segment rows in pixels (0.0–20.0).
pub(crate) const DEFAULT_VIZ_SEG_GAP_PX: f32 = 2.0;
/// Number of discrete vertical VFD segments per bar column (8..=120).
pub(crate) const DEFAULT_VIZ_SEG_COUNT: u32 = 50;

// ── OBD2 defaults ────────────────────────────────────────────────────────────

/// Whether the OBD2 worker is enabled by default.
#[cfg(feature = "obd2")]
pub(crate) const DEFAULT_OBD2_ENABLED: bool = false;
/// Default poll interval for configured PIDs, in milliseconds.
#[cfg(feature = "obd2")]
pub(crate) const DEFAULT_OBD2_POLL_INTERVAL_MS: u32 = 250;

/// Command-line arguments. `clap` also reads the listed environment variables,
/// with CLI flags taking precedence over the environment.
#[derive(Parser, Debug)]
#[command(name = "eva-navigation-unit", about = "Android Auto head unit")]
struct Cli {
    /// Path to a TOML configuration file.
    #[arg(long, env = "EVA_CONFIG")]
    config: Option<PathBuf>,

    /// Minimum selectable Android Auto DPI.
    #[arg(long, env = "EVA_MIN_DPI")]
    min_dpi: Option<i32>,

    /// Maximum selectable Android Auto DPI.
    #[arg(long, env = "EVA_MAX_DPI")]
    max_dpi: Option<i32>,

    /// Current Android Auto DPI.
    #[arg(long, env = "EVA_DPI")]
    dpi: Option<i32>,

    /// Enable wireless Android Auto.
    #[arg(long, env = "EVA_WIRELESS")]
    wireless: Option<bool>,

    /// Enable USB (wired) Android Auto.
    #[arg(long, env = "EVA_USB")]
    usb: Option<bool>,

    /// Reset a USB phone left in AOA accessory mode at startup to clear a stale
    /// Android Auto session (disable on the Nintendo Switch Tegra xHCI).
    #[arg(long, env = "EVA_RESET_STALE_ACCESSORY")]
    reset_stale_accessory: Option<bool>,

    /// Android Auto video resolution (720 or 1080).
    #[arg(long, env = "EVA_RESOLUTION")]
    resolution: Option<i32>,

    /// Android Auto video frame rate (30 or 60).
    #[arg(long, env = "EVA_FPS")]
    fps: Option<i32>,

    /// View transition mode (0 = CRT | 1 = FADE | 2 = SLIDE).
    #[arg(long, env = "EVA_TRANSITION_MODE")]
    transition_mode: Option<i32>,

    /// Android Auto video start/stop transition (0 = CRT | 1 = FADE | 2 = SLIDE).
    #[arg(long, env = "EVA_AA_VIDEO_TRANSITION_MODE")]
    aa_video_transition_mode: Option<i32>,

    /// View transition speed multiplier (higher = faster).
    #[arg(long, env = "EVA_TRANSITION_SPEED")]
    transition_speed: Option<f32>,

    /// Android Auto video transition speed multiplier (higher = faster).
    #[arg(long, env = "EVA_AA_VIDEO_TRANSITION_SPEED")]
    aa_video_transition_speed: Option<f32>,

    /// Color theme (0 = NERV-HQ | 1 = MATRIX).
    #[arg(long, env = "EVA_THEME")]
    theme: Option<i32>,

    /// GL underlay wireframe model (0 = sphere | 1 = cube | 2 = car | 3 = speaker).
    #[arg(long, env = "EVA_GFX_MODEL")]
    gfx_model: Option<i32>,

    /// Display the window in fullscreen mode.
    #[arg(long, env = "EVA_FULLSCREEN")]
    fullscreen: Option<bool>,

    /// Wi-Fi hotspot backend (0 = NetworkManager | 1 = hostapd).
    #[arg(long, env = "EVA_HOTSPOT_BACKEND")]
    hotspot_backend: Option<i32>,

    /// 5 GHz channel for the hostapd hotspot backend (0 = automatic).
    #[arg(long, env = "EVA_HOTSPOT_CHANNEL")]
    hotspot_channel: Option<i32>,

    /// Sidebar header/branding text (default: "NERV").
    #[arg(long, env = "EVA_CAR_NAME_SHORT")]
    car_name_short: Option<String>,

    /// Android Auto "locked terminal" overlay app name text (default: "EVA-02").
    #[arg(long, env = "EVA_APP_NAME")]
    app_name: Option<String>,

    /// Android Auto "locked terminal" overlay long car name text.
    #[arg(long, env = "EVA_CAR_NAME_LONG")]
    car_name_long: Option<String>,

    /// Android Auto "locked terminal" overlay waiting-for-connection text.
    #[arg(long, env = "EVA_AA_WAITING_TEXT")]
    aa_waiting_text: Option<String>,

    /// Number of visualizer frequency bands (4..=64).
    #[arg(long, env = "EVA_VIZ_BANDS")]
    viz_bands: Option<u32>,

    /// Visualizer FFT window size in samples (power of two: 512..=8192).
    #[arg(long, env = "EVA_VIZ_FFT_SIZE")]
    viz_fft_size: Option<u32>,

    /// Visualizer FFT hop size in samples — controls update rate (64..=4096).
    #[arg(long, env = "EVA_VIZ_HOP")]
    viz_hop: Option<u32>,

    /// Visualizer lowest displayed frequency in Hz.
    #[arg(long, env = "EVA_VIZ_FREQ_MIN")]
    viz_freq_min: Option<f32>,

    /// Visualizer highest displayed frequency in Hz.
    #[arg(long, env = "EVA_VIZ_FREQ_MAX")]
    viz_freq_max: Option<f32>,

    /// Visualizer input smoother attack time constant in milliseconds.
    #[arg(long, env = "EVA_VIZ_INPUT_ATTACK_MS")]
    viz_input_attack_ms: Option<f32>,

    /// Visualizer input smoother release time constant in milliseconds.
    #[arg(long, env = "EVA_VIZ_INPUT_RELEASE_MS")]
    viz_input_release_ms: Option<f32>,

    /// Visualizer gravity fall-speed multiplier (1.0 = CAVA default).
    #[arg(long, env = "EVA_VIZ_GRAVITY")]
    viz_gravity: Option<f32>,

    /// Visualizer noise-reduction factor override (0.0 = auto from framerate).
    #[arg(long, env = "EVA_VIZ_NOISE_REDUCTION")]
    viz_noise_reduction: Option<f32>,

    /// Visualizer horizontal gap between bar columns (fraction of slot width, 0.0–0.45).
    #[arg(long, env = "EVA_VIZ_BAR_GAP")]
    viz_bar_gap: Option<f32>,

    /// Visualizer vertical gap between segment rows in pixels.
    #[arg(long, env = "EVA_VIZ_SEG_GAP_PX")]
    viz_seg_gap_px: Option<f32>,

    /// Visualizer number of discrete vertical VFD segments per bar column.
    #[arg(long, env = "EVA_VIZ_SEG_COUNT")]
    viz_seg_count: Option<u32>,

    /// Global log level (error | warn | info | debug | trace).
    #[arg(long, env = "EVA_LOG_LEVEL")]
    log_level: Option<String>,

    /// Log level override for the UI component.
    #[arg(long, env = "EVA_LOG_UI")]
    log_ui: Option<String>,

    /// Log level override for the Audio component.
    #[arg(long, env = "EVA_LOG_AUDIO")]
    log_audio: Option<String>,

    /// Log level override for the Android Auto (AA) component.
    #[arg(long, env = "EVA_LOG_AA")]
    log_aa: Option<String>,

    /// Log level override for the Bluetooth/transport (BT) component.
    #[arg(long, env = "EVA_LOG_BT")]
    log_bt: Option<String>,

    /// Also write logs to this file (omit for console only).
    #[arg(long, env = "EVA_LOG_FILE")]
    log_file: Option<PathBuf>,

    /// Log output format (text | json).
    #[arg(long, env = "EVA_LOG_FORMAT")]
    log_format: Option<String>,

    /// Enable the OBD2 worker (connects to a paired ELM327 over Bluetooth).
    #[cfg(feature = "obd2")]
    #[arg(long, env = "EVA_OBD2_ENABLED")]
    obd2_enabled: Option<bool>,

    /// Bluetooth MAC address of the paired ELM327 adapter (e.g. "AA:BB:CC:DD:EE:FF").
    #[cfg(feature = "obd2")]
    #[arg(long, env = "EVA_OBD2_DEVICE_ADDRESS")]
    obd2_device_address: Option<String>,

    /// Poll interval for the configured OBD2 PIDs, in milliseconds.
    #[cfg(feature = "obd2")]
    #[arg(long, env = "EVA_OBD2_POLL_INTERVAL_MS")]
    obd2_poll_interval_ms: Option<u32>,
}

/// Shape of the optional TOML configuration file.
#[derive(Deserialize, Serialize, Default, Debug)]
struct FileConfig {
    min_dpi: Option<i32>,
    max_dpi: Option<i32>,
    dpi: Option<i32>,
    wireless: Option<bool>,
    usb: Option<bool>,
    reset_stale_accessory: Option<bool>,
    resolution: Option<i32>,
    fps: Option<i32>,
    transition_mode: Option<i32>,
    aa_video_transition_mode: Option<i32>,
    transition_speed: Option<f32>,
    aa_video_transition_speed: Option<f32>,
    theme: Option<i32>,
    gfx_model: Option<i32>,
    fullscreen: Option<bool>,
    hotspot_backend: Option<i32>,
    hotspot_channel: Option<i32>,
    car_name_short: Option<String>,
    app_name: Option<String>,
    car_name_long: Option<String>,
    aa_waiting_text: Option<String>,
    log: Option<LogFileConfig>,
    viz: Option<VizFileConfig>,
    #[cfg(feature = "obd2")]
    obd2: Option<Obd2FileConfig>,
}

/// Shape of the optional `[log]` table in the TOML configuration file.
#[derive(Deserialize, Serialize, Default, Debug)]
struct LogFileConfig {
    level: Option<String>,
    ui: Option<String>,
    audio: Option<String>,
    aa: Option<String>,
    bt: Option<String>,
    file: Option<PathBuf>,
    format: Option<String>,
}

/// Shape of the optional `[viz]` table in the TOML configuration file.
#[derive(Deserialize, Serialize, Default, Debug)]
struct VizFileConfig {
    bands: Option<u32>,
    fft_size: Option<u32>,
    hop: Option<u32>,
    freq_min: Option<f32>,
    freq_max: Option<f32>,
    input_attack_ms: Option<f32>,
    input_release_ms: Option<f32>,
    gravity: Option<f32>,
    noise_reduction: Option<f32>,
    bar_gap: Option<f32>,
    seg_gap_px: Option<f32>,
    seg_count: Option<u32>,
}

/// Shape of the optional `[obd2]` table in the TOML configuration file.
#[cfg(feature = "obd2")]
#[derive(Deserialize, Serialize, Default, Debug)]
struct Obd2FileConfig {
    enabled: Option<bool>,
    device_address: Option<String>,
    poll_interval_ms: Option<u32>,
    /// User-defined PIDs, e.g.:
    /// ```toml
    /// [[obd2.pids]]
    /// name = "engine_rpm"
    /// service = 1
    /// pid = "0C"
    /// formula = "(A * 256 + B) / 4"
    /// unit = "rpm"
    /// ```
    /// `service` is the OBD-II service/mode (`1` = show current data, `0x22`
    /// = VAG/enhanced read-by-identifier, ...) and `pid` is a hex string of
    /// the request data that follows it: a single byte for standard Mode 01
    /// PIDs (e.g. `"0C"`), or a 2-byte DID for enhanced Mode 22 PIDs (e.g.
    /// `"100C"`). The formula is evaluated with response data bytes bound
    /// to `A`, `B`, `C`, `D`, ... matching the SAE/Wikipedia OBD-II PID
    /// convention, so formulas from
    /// https://en.wikipedia.org/wiki/OBD-II_PIDs can be pasted in directly.
    #[serde(default)]
    pids: Vec<Obd2PidFileConfig>,
}

/// Shape of a single `[[obd2.pids]]` entry in the TOML configuration file.
#[cfg(feature = "obd2")]
#[derive(Deserialize, Serialize, Debug, Clone)]
struct Obd2PidFileConfig {
    name: String,
    service: u8,
    /// Hex string of the request data following the service byte, e.g.
    /// `"0C"` (Mode 01 PID 0x0C) or `"100C"` (Mode 22 DID 0x100C).
    pid: String,
    formula: String,
    unit: String,
}

/// Parse a `pid` hex string (e.g. `"0C"` or `"100C"`) into raw bytes.
/// Returns `None` (and logs a warning identifying `name`) if the string is
/// empty, has an odd number of hex digits, or contains invalid digits.
#[cfg(feature = "obd2")]
fn parse_pid_hex(name: &str, hex: &str) -> Option<Vec<u8>> {
    let hex = hex.trim().strip_prefix("0x").unwrap_or(hex.trim());
    if hex.is_empty() || !hex.len().is_multiple_of(2) {
        log::warn!(
            "obd2: PID '{name}' has invalid hex '{hex}' \
             (must be a non-empty, even-length hex string); skipping"
        );
        return None;
    }
    let bytes: Result<Vec<u8>, _> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect();
    match bytes {
        Ok(b) => Some(b),
        Err(e) => {
            log::warn!("obd2: PID '{name}' has invalid hex '{hex}' ({e}); skipping");
            None
        }
    }
}

/// Fully resolved logging / debug-pipeline configuration.
#[derive(Debug, Clone)]
pub(crate) struct LogConfig {
    /// Global default level applied to every component.
    pub(crate) level: String,
    /// Optional per-component level overrides.
    pub(crate) ui: Option<String>,
    pub(crate) audio: Option<String>,
    pub(crate) aa: Option<String>,
    pub(crate) bt: Option<String>,
    /// Optional file to also write logs to (console output is always on).
    pub(crate) file: Option<PathBuf>,
    /// Output format: `text` or `json`.
    pub(crate) format: String,
}

/// Fully resolved spectrum visualizer tuning parameters.
#[derive(Debug, Clone)]
pub(crate) struct VizConfig {
    /// Active band count (1..=MAX_VIZ_BANDS).
    pub(crate) bands: usize,
    /// FFT window size in samples (power of two).
    pub(crate) fft_size: usize,
    /// Hop size: a new FFT frame is triggered every this many mono samples.
    pub(crate) hop: usize,
    /// Lowest displayed frequency (Hz).
    pub(crate) freq_min: f32,
    /// Highest displayed frequency (Hz).
    pub(crate) freq_max: f32,
    /// Input pre-smoother attack time constant (ms).
    pub(crate) input_attack_ms: f32,
    /// Input pre-smoother release time constant (ms).
    pub(crate) input_release_ms: f32,
    /// Gravity fall-speed multiplier (1.0 = CAVA default).
    pub(crate) gravity: f32,
    /// Noise-reduction factor override (0.0 = auto from framerate).
    pub(crate) noise_reduction: f32,
    /// Horizontal gap between bar columns (fraction of slot width).
    pub(crate) bar_gap: f32,
    /// Vertical gap between segment rows in pixels.
    pub(crate) seg_gap_px: f32,
    /// Number of discrete vertical VFD segments per bar column.
    pub(crate) seg_count: usize,
}

/// Raw (unclamped) parameters for [`VizConfig::new`], bundled to keep the
/// call site readable and avoid a long positional argument list.
struct VizConfigRaw {
    bands: u32,
    fft_size: u32,
    hop: u32,
    freq_min: f32,
    freq_max: f32,
    input_attack_ms: f32,
    input_release_ms: f32,
    gravity: f32,
    noise_reduction: f32,
    bar_gap: f32,
    seg_gap_px: f32,
    seg_count: u32,
}

impl VizConfig {
    fn new(raw: VizConfigRaw) -> Self {
        let VizConfigRaw {
            bands, fft_size, hop,
            freq_min, freq_max,
            input_attack_ms, input_release_ms,
            gravity, noise_reduction,
            bar_gap, seg_gap_px, seg_count,
        } = raw;
        // Round fft_size down to the nearest power of two within [512, 8192].
        let fft_size_raw = fft_size.clamp(512, 8192) as usize;
        let mut fft_size = 512usize;
        while fft_size * 2 <= fft_size_raw { fft_size *= 2; }

        let bands = (bands as usize).clamp(4, MAX_VIZ_BANDS as usize);
        let hop = (hop as usize).clamp(64, fft_size / 2).max(1);
        let freq_min = freq_min.clamp(1.0, 23_000.0);
        let freq_max = freq_max.clamp(freq_min + 100.0, 24_000.0);
        Self {
            bands,
            fft_size,
            hop,
            freq_min,
            freq_max,
            input_attack_ms:   input_attack_ms.clamp(1.0, 500.0),
            input_release_ms:  input_release_ms.clamp(1.0, 2_000.0),
            gravity:           gravity.clamp(0.1, 10.0),
            noise_reduction:   noise_reduction.clamp(0.0, 0.99),
            bar_gap:           bar_gap.clamp(0.0, 0.45),
            seg_gap_px:        seg_gap_px.clamp(0.0, 20.0),
            seg_count:         seg_count.clamp(8, 120) as usize,
        }
    }
}

/// Fully resolved OBD2 configuration.
#[cfg(feature = "obd2")]
#[derive(Debug, Clone, Default)]
pub(crate) struct Obd2Config {
    /// Whether the OBD2 worker is enabled.
    pub(crate) enabled: bool,
    /// Bluetooth MAC address of the paired ELM327 adapter. Pairing itself is
    /// a manual/OS-level step for now; there is no in-app discovery yet.
    pub(crate) device_address: Option<String>,
    /// Poll interval for the configured PIDs, in milliseconds.
    pub(crate) poll_interval_ms: u32,
    /// User-defined PIDs to poll (see `[[obd2.pids]]` in the config file).
    pub(crate) pids: Vec<Obd2PidConfig>,
}

/// A single user-defined OBD-II PID: which service/data to request over the
/// raw-request escape hatch, and how to turn the raw response bytes into a
/// physical value.
#[cfg(feature = "obd2")]
#[derive(Debug, Clone)]
pub(crate) struct Obd2PidConfig {
    /// Human-readable identifier (e.g. "engine_rpm").
    pub(crate) name: String,
    /// OBD-II service/mode (e.g. 1 for "show current data", 0x22 for VAG's
    /// enhanced read-by-identifier).
    pub(crate) service: u8,
    /// Request data following the service byte: a single-byte PID for
    /// standard Mode 01 PIDs (e.g. `[0x0C]` for engine RPM), or a 2-byte DID
    /// for enhanced Mode 22 PIDs (e.g. `[0x10, 0x0C]`).
    pub(crate) data: Vec<u8>,
    /// Expression computing the physical value from response bytes bound to
    /// `A`, `B`, `C`, `D`, ... (the SAE/Wikipedia OBD-II PID convention).
    /// Compiled to a [`meval::Expr`] by the OBD2 worker at startup.
    pub(crate) formula: String,
    /// Arbitrary physical unit label (e.g. "rpm", "\u00b0C", "km/h").
    pub(crate) unit: String,
}

/// Fully resolved runtime configuration.
#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub(crate) min_dpi: i32,
    pub(crate) max_dpi: i32,
    pub(crate) dpi: i32,
    pub(crate) wireless: bool,
    pub(crate) usb: bool,
    /// Whether to reset a USB phone left in AOA accessory mode at startup to
    /// clear a stale Android Auto session inherited from a previous run.
    pub(crate) reset_stale_accessory: bool,
    pub(crate) resolution: i32,
    pub(crate) fps: i32,
    pub(crate) transition_mode: i32,
    pub(crate) aa_video_transition_mode: i32,
    pub(crate) transition_speed: f32,
    pub(crate) aa_video_transition_speed: f32,
    /// Active color theme (0 = NERV-HQ | 1 = MATRIX).
    pub(crate) theme: i32,
    /// GL underlay wireframe model (0 = sphere | 1 = cube | 2 = car).
    pub(crate) gfx_model: i32,
    /// Whether the window is shown in fullscreen mode.
    pub(crate) fullscreen: bool,
    /// Wi-Fi hotspot backend (0 = NetworkManager | 1 = hostapd).
    pub(crate) hotspot_backend: i32,
    /// 5 GHz channel for the hostapd hotspot backend (0 = automatic).
    pub(crate) hotspot_channel: i32,
    /// Sidebar header/branding text.
    pub(crate) car_name_short: String,
    /// Android Auto "locked terminal" overlay app name text.
    pub(crate) app_name: String,
    /// Android Auto "locked terminal" overlay long car name text.
    pub(crate) car_name_long: String,
    /// Android Auto "locked terminal" overlay waiting-for-connection text.
    pub(crate) aa_waiting_text: String,
    /// Logging / debug-pipeline configuration.
    pub(crate) log: LogConfig,
    /// Spectrum visualizer tuning parameters.
    pub(crate) viz: VizConfig,
    /// OBD2 telemetry configuration.
    #[cfg(feature = "obd2")]
    pub(crate) obd2: Obd2Config,
    /// Path the configuration is loaded from and saved back to.
    pub(crate) path: PathBuf,
}

impl Config {
    /// Parse CLI arguments and environment, merge with the config file, and
    /// fall back to defaults. Invalid values are sanitised so the UI always
    /// receives a coherent `min <= max` range.
    pub(crate) fn load() -> Self {
        let cli = Cli::parse();
        let path = config_path(cli.config.as_ref());
        let file = load_file_config(&path);

        let min_dpi = cli.min_dpi.or(file.min_dpi).unwrap_or(DEFAULT_MIN_DPI);
        let max_dpi = cli.max_dpi.or(file.max_dpi).unwrap_or(DEFAULT_MAX_DPI);
        let dpi = cli.dpi.or(file.dpi).unwrap_or(DEFAULT_DPI);
        let wireless = cli.wireless.or(file.wireless).unwrap_or(DEFAULT_WIRELESS);
        let usb = cli.usb.or(file.usb).unwrap_or(DEFAULT_USB);
        let reset_stale_accessory = cli
            .reset_stale_accessory
            .or(file.reset_stale_accessory)
            .unwrap_or(DEFAULT_RESET_STALE_ACCESSORY);
        let resolution = cli
            .resolution
            .or(file.resolution)
            .unwrap_or(DEFAULT_RESOLUTION);
        let fps = cli.fps.or(file.fps).unwrap_or(DEFAULT_FPS);
        let transition_mode = cli
            .transition_mode
            .or(file.transition_mode)
            .unwrap_or(DEFAULT_TRANSITION_MODE);
        let aa_video_transition_mode = cli
            .aa_video_transition_mode
            .or(file.aa_video_transition_mode)
            .unwrap_or(DEFAULT_AA_VIDEO_TRANSITION_MODE);
        let transition_speed = cli
            .transition_speed
            .or(file.transition_speed)
            .unwrap_or(DEFAULT_TRANSITION_SPEED);
        let aa_video_transition_speed = cli
            .aa_video_transition_speed
            .or(file.aa_video_transition_speed)
            .unwrap_or(DEFAULT_AA_VIDEO_TRANSITION_SPEED);

        let theme = cli.theme.or(file.theme).unwrap_or(DEFAULT_THEME);
        let gfx_model = cli.gfx_model.or(file.gfx_model).unwrap_or(DEFAULT_GFX_MODEL);
        let fullscreen = cli
            .fullscreen
            .or(file.fullscreen)
            .unwrap_or(DEFAULT_FULLSCREEN);
        let hotspot_backend = cli
            .hotspot_backend
            .or(file.hotspot_backend)
            .unwrap_or(DEFAULT_HOTSPOT_BACKEND);
        let hotspot_channel = cli
            .hotspot_channel
            .or(file.hotspot_channel)
            .unwrap_or(DEFAULT_HOTSPOT_CHANNEL);
        let car_name_short = cli
            .car_name_short
            .or(file.car_name_short)
            .unwrap_or_else(|| DEFAULT_CAR_NAME_SHORT.to_string());
        let app_name = cli
            .app_name
            .or(file.app_name)
            .unwrap_or_else(|| DEFAULT_APP_NAME.to_string());
        let car_name_long = cli
            .car_name_long
            .or(file.car_name_long)
            .unwrap_or_else(|| DEFAULT_CAR_NAME_LONG.to_string());
        let aa_waiting_text = cli
            .aa_waiting_text
            .or(file.aa_waiting_text)
            .unwrap_or_else(|| DEFAULT_AA_WAITING_TEXT.to_string());

        let file_log = file.log.unwrap_or_default();
        let log = LogConfig {
            level: cli
                .log_level
                .or(file_log.level)
                .unwrap_or_else(|| DEFAULT_LOG_LEVEL.to_string()),
            ui: cli.log_ui.or(file_log.ui),
            audio: cli.log_audio.or(file_log.audio),
            aa: cli.log_aa.or(file_log.aa),
            bt: cli.log_bt.or(file_log.bt),
            file: cli.log_file.or(file_log.file),
            format: cli
                .log_format
                .or(file_log.format)
                .unwrap_or_else(|| DEFAULT_LOG_FORMAT.to_string()),
        };

        let file_viz = file.viz.unwrap_or_default();
        let viz = VizConfig::new(VizConfigRaw {
            bands: cli.viz_bands.or(file_viz.bands).unwrap_or(DEFAULT_VIZ_BANDS),
            fft_size: cli.viz_fft_size.or(file_viz.fft_size).unwrap_or(DEFAULT_VIZ_FFT_SIZE),
            hop: cli.viz_hop.or(file_viz.hop).unwrap_or(DEFAULT_VIZ_HOP),
            freq_min: cli.viz_freq_min.or(file_viz.freq_min).unwrap_or(DEFAULT_VIZ_FREQ_MIN),
            freq_max: cli.viz_freq_max.or(file_viz.freq_max).unwrap_or(DEFAULT_VIZ_FREQ_MAX),
            input_attack_ms: cli.viz_input_attack_ms.or(file_viz.input_attack_ms).unwrap_or(DEFAULT_VIZ_INPUT_ATTACK_MS),
            input_release_ms: cli.viz_input_release_ms.or(file_viz.input_release_ms).unwrap_or(DEFAULT_VIZ_INPUT_RELEASE_MS),
            gravity: cli.viz_gravity.or(file_viz.gravity).unwrap_or(DEFAULT_VIZ_GRAVITY),
            noise_reduction: cli.viz_noise_reduction.or(file_viz.noise_reduction).unwrap_or(DEFAULT_VIZ_NOISE_REDUCTION),
            bar_gap: cli.viz_bar_gap.or(file_viz.bar_gap).unwrap_or(DEFAULT_VIZ_BAR_GAP),
            seg_gap_px: cli.viz_seg_gap_px.or(file_viz.seg_gap_px).unwrap_or(DEFAULT_VIZ_SEG_GAP_PX),
            seg_count: cli.viz_seg_count.or(file_viz.seg_count).unwrap_or(DEFAULT_VIZ_SEG_COUNT),
        });

        #[cfg(feature = "obd2")]
        let obd2 = {
            let file_obd2 = file.obd2.unwrap_or_default();
            Obd2Config {
                enabled: cli
                    .obd2_enabled
                    .or(file_obd2.enabled)
                    .unwrap_or(DEFAULT_OBD2_ENABLED),
                device_address: cli.obd2_device_address.or(file_obd2.device_address),
                poll_interval_ms: cli
                    .obd2_poll_interval_ms
                    .or(file_obd2.poll_interval_ms)
                    .unwrap_or(DEFAULT_OBD2_POLL_INTERVAL_MS),
                pids: file_obd2
                    .pids
                    .into_iter()
                    .filter_map(|p| {
                        let data = parse_pid_hex(&p.name, &p.pid)?;
                        Some(Obd2PidConfig {
                            name: p.name,
                            service: p.service,
                            data,
                            formula: p.formula,
                            unit: p.unit,
                        })
                    })
                    .collect(),
            }
        };

        Self::sanitised(Self {
            min_dpi,
            max_dpi,
            dpi,
            wireless,
            usb,
            reset_stale_accessory,
            resolution,
            fps,
            transition_mode,
            aa_video_transition_mode,
            transition_speed,
            aa_video_transition_speed,
            theme,
            gfx_model,
            fullscreen,
            hotspot_backend,
            hotspot_channel,
            car_name_short,
            app_name,
            car_name_long,
            aa_waiting_text,
            log,
            viz,
            #[cfg(feature = "obd2")]
            obd2,
            path,
        })
    }

    /// Ensure DPI bounds are positive and ordered (`min <= max`), and that the
    /// current DPI falls within those bounds. Transition modes are clamped to
    /// the valid 0..=2 range. Takes an unsanitised `Self` so the caller
    /// doesn't have to pass every field positionally.
    fn sanitised(raw: Self) -> Self {
        let Config {
            min_dpi,
            max_dpi,
            dpi,
            wireless,
            usb,
            reset_stale_accessory,
            resolution,
            fps,
            transition_mode,
            aa_video_transition_mode,
            transition_speed,
            aa_video_transition_speed,
            theme,
            gfx_model,
            fullscreen,
            hotspot_backend,
            hotspot_channel,
            car_name_short,
            app_name,
            car_name_long,
            aa_waiting_text,
            log,
            viz,
            #[cfg(feature = "obd2")]
            obd2,
            path,
        } = raw;
        let mut min_dpi = min_dpi.max(1);
        let mut max_dpi = max_dpi.max(1);
        if min_dpi > max_dpi {
            log::warn!(
                "min_dpi ({min_dpi}) > max_dpi ({max_dpi}); swapping to keep a valid range"
            );
            std::mem::swap(&mut min_dpi, &mut max_dpi);
        }
        let dpi = dpi.clamp(min_dpi, max_dpi);
        Self {
            min_dpi,
            max_dpi,
            dpi,
            wireless,
            usb,
            reset_stale_accessory,
            resolution: if resolution >= 1080 {
                1080
            } else if resolution >= 720 {
                720
            } else {
                480
            },
            fps: if fps >= 60 { 60 } else { 30 },
            transition_mode: transition_mode.clamp(0, 2),
            aa_video_transition_mode: aa_video_transition_mode.clamp(0, 2),
            transition_speed: transition_speed.clamp(MIN_TRANSITION_SPEED, MAX_TRANSITION_SPEED),
            aa_video_transition_speed: aa_video_transition_speed
                .clamp(MIN_TRANSITION_SPEED, MAX_TRANSITION_SPEED),
            theme: theme.max(0),
            gfx_model: gfx_model.clamp(0, 3),
            fullscreen,
            hotspot_backend: hotspot_backend.clamp(0, 1),
            hotspot_channel: hotspot_channel.max(0),
            car_name_short,
            app_name,
            car_name_long,
            aa_waiting_text,
            log,
            viz,
            #[cfg(feature = "obd2")]
            obd2,
            path,
        }
    }

    /// Persist the current configuration back to its TOML file.
    pub(crate) fn save(&self) {
        let file = FileConfig {
            min_dpi: Some(self.min_dpi),
            max_dpi: Some(self.max_dpi),
            dpi: Some(self.dpi),
            wireless: Some(self.wireless),
            usb: Some(self.usb),
            reset_stale_accessory: Some(self.reset_stale_accessory),
            resolution: Some(self.resolution),
            fps: Some(self.fps),
            transition_mode: Some(self.transition_mode),
            aa_video_transition_mode: Some(self.aa_video_transition_mode),
            transition_speed: Some(self.transition_speed),
            aa_video_transition_speed: Some(self.aa_video_transition_speed),
            theme: Some(self.theme),
            gfx_model: Some(self.gfx_model),
            fullscreen: Some(self.fullscreen),
            hotspot_backend: Some(self.hotspot_backend),
            hotspot_channel: Some(self.hotspot_channel),
            car_name_short: Some(self.car_name_short.clone()),
            app_name: Some(self.app_name.clone()),
            car_name_long: Some(self.car_name_long.clone()),
            aa_waiting_text: Some(self.aa_waiting_text.clone()),
            log: Some(LogFileConfig {
                level: Some(self.log.level.clone()),
                ui: self.log.ui.clone(),
                audio: self.log.audio.clone(),
                aa: self.log.aa.clone(),
                bt: self.log.bt.clone(),
                file: self.log.file.clone(),
                format: Some(self.log.format.clone()),
            }),
            viz: Some(VizFileConfig {
                bands: Some(self.viz.bands as u32),
                fft_size: Some(self.viz.fft_size as u32),
                hop: Some(self.viz.hop as u32),
                freq_min: Some(self.viz.freq_min),
                freq_max: Some(self.viz.freq_max),
                input_attack_ms: Some(self.viz.input_attack_ms),
                input_release_ms: Some(self.viz.input_release_ms),
                gravity: Some(self.viz.gravity),
                noise_reduction: Some(self.viz.noise_reduction),
                bar_gap: Some(self.viz.bar_gap),
                seg_gap_px: Some(self.viz.seg_gap_px),
                seg_count: Some(self.viz.seg_count as u32),
            }),
            #[cfg(feature = "obd2")]
            obd2: Some(Obd2FileConfig {
                enabled: Some(self.obd2.enabled),
                device_address: self.obd2.device_address.clone(),
                poll_interval_ms: Some(self.obd2.poll_interval_ms),
                pids: self
                    .obd2
                    .pids
                    .iter()
                    .map(|p| {
                        let hex = p.data.iter().map(|b| format!("{b:02X}")).collect();
                        Obd2PidFileConfig {
                            name: p.name.clone(),
                            service: p.service,
                            pid: hex,
                            formula: p.formula.clone(),
                            unit: p.unit.clone(),
                        }
                    })
                    .collect(),
            }),
        };
        match toml::to_string_pretty(&file) {
            Ok(contents) => {
                if let Some(parent) = self.path.parent()
                    && !parent.as_os_str().is_empty()
                        && let Err(e) = std::fs::create_dir_all(parent) {
                            log::warn!(
                                "Failed to create config directory {}: {e}",
                                parent.display()
                            );
                        }
                if let Err(e) = std::fs::write(&self.path, contents) {
                    log::warn!("Failed to write config file {}: {e}", self.path.display());
                }
            }
            Err(e) => log::warn!("Failed to serialise config: {e}"),
        }
    }
}

/// Resolve the configuration file path with the following precedence:
///   1. Explicit path from `--config` / `EVA_CONFIG`.
///   2. A `config.toml` in the current working directory (development).
///   3. The per-user config at `$XDG_CONFIG_HOME/eva-ui/config.toml`
///      (defaulting to `~/.config/eva-ui/config.toml`) once installed.
fn config_path(explicit: Option<&PathBuf>) -> PathBuf {
    if let Some(path) = explicit {
        return path.clone();
    }

    // Keep configuration local while developing: if a `config.toml` already
    // sits in the working directory (the repo root when run via `cargo run`),
    // use it instead of the per-user location.
    let local = PathBuf::from("config.toml");
    if local.exists() {
        return local;
    }

    user_config_path().unwrap_or(local)
}

/// The per-user configuration path: `$XDG_CONFIG_HOME/eva-ui/config.toml`,
/// falling back to `$HOME/.config/eva-ui/config.toml`.
fn user_config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("eva-ui").join("config.toml"))
}

/// Load the config file at `path`. A missing file yields defaults; a present
/// file that fails to read or parse is reported and ignored.
fn load_file_config(path: &PathBuf) -> FileConfig {
    if !path.exists() {
        return FileConfig::default();
    }

    match std::fs::read_to_string(path) {
        Ok(contents) => match toml::from_str(&contents) {
            Ok(cfg) => cfg,
            Err(e) => {
                log::warn!("Failed to parse config file {}: {e}", path.display());
                FileConfig::default()
            }
        },
        Err(e) => {
            log::warn!("Failed to read config file {}: {e}", path.display());
            FileConfig::default()
        }
    }
}
