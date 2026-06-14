//! Runtime configuration for a310.
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

/// Command-line arguments. `clap` also reads the listed environment variables,
/// with CLI flags taking precedence over the environment.
#[derive(Parser, Debug)]
#[command(name = "a310", about = "Android Auto head unit")]
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
}

/// Shape of the optional TOML configuration file.
#[derive(Deserialize, Serialize, Default, Debug)]
struct FileConfig {
    min_dpi: Option<i32>,
    max_dpi: Option<i32>,
    dpi: Option<i32>,
    wireless: Option<bool>,
    usb: Option<bool>,
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
}

/// Fully resolved runtime configuration.
#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub(crate) min_dpi: i32,
    pub(crate) max_dpi: i32,
    pub(crate) dpi: i32,
    pub(crate) wireless: bool,
    pub(crate) usb: bool,
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

        Self::sanitised(
            min_dpi,
            max_dpi,
            dpi,
            wireless,
            usb,
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
            path,
        )
    }

    /// Ensure DPI bounds are positive and ordered (`min <= max`), and that the
    /// current DPI falls within those bounds. Transition modes are clamped to
    /// the valid 0..=2 range.
    fn sanitised(
        min_dpi: i32,
        max_dpi: i32,
        dpi: i32,
        wireless: bool,
        usb: bool,
        resolution: i32,
        fps: i32,
        transition_mode: i32,
        aa_video_transition_mode: i32,
        transition_speed: f32,
        aa_video_transition_speed: f32,
        theme: i32,
        gfx_model: i32,
        fullscreen: bool,
        hotspot_backend: i32,
        hotspot_channel: i32,
        path: PathBuf,
    ) -> Self {
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
        };
        match toml::to_string_pretty(&file) {
            Ok(contents) => {
                if let Some(parent) = self.path.parent() {
                    if !parent.as_os_str().is_empty() {
                        if let Err(e) = std::fs::create_dir_all(parent) {
                            log::warn!(
                                "Failed to create config directory {}: {e}",
                                parent.display()
                            );
                        }
                    }
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
