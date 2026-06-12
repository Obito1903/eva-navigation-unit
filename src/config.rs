//! Runtime configuration for a310.
//!
//! Values are resolved with the following precedence (highest wins):
//!   1. CLI arguments        (e.g. `--min-dpi 120`)
//!   2. Environment variables (`EVA_MIN_DPI`, `EVA_MAX_DPI`)
//!   3. Config file (TOML)   (`--config <path>` or `EVA_CONFIG`, else `config.toml`)
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
/// Default Android Auto video resolution (vertical lines: 720 or 1080).
pub(crate) const DEFAULT_RESOLUTION: i32 = 720;

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

    /// Android Auto video resolution (720 or 1080).
    #[arg(long, env = "EVA_RESOLUTION")]
    resolution: Option<i32>,

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
}

/// Shape of the optional TOML configuration file.
#[derive(Deserialize, Serialize, Default, Debug)]
struct FileConfig {
    min_dpi: Option<i32>,
    max_dpi: Option<i32>,
    dpi: Option<i32>,
    wireless: Option<bool>,
    resolution: Option<i32>,
    transition_mode: Option<i32>,
    aa_video_transition_mode: Option<i32>,
    transition_speed: Option<f32>,
    aa_video_transition_speed: Option<f32>,
}

/// Fully resolved runtime configuration.
#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub(crate) min_dpi: i32,
    pub(crate) max_dpi: i32,
    pub(crate) dpi: i32,
    pub(crate) wireless: bool,
    pub(crate) resolution: i32,
    pub(crate) transition_mode: i32,
    pub(crate) aa_video_transition_mode: i32,
    pub(crate) transition_speed: f32,
    pub(crate) aa_video_transition_speed: f32,
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
        let resolution = cli
            .resolution
            .or(file.resolution)
            .unwrap_or(DEFAULT_RESOLUTION);
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

        Self::sanitised(
            min_dpi,
            max_dpi,
            dpi,
            wireless,
            resolution,
            transition_mode,
            aa_video_transition_mode,
            transition_speed,
            aa_video_transition_speed,
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
        resolution: i32,
        transition_mode: i32,
        aa_video_transition_mode: i32,
        transition_speed: f32,
        aa_video_transition_speed: f32,
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
            resolution: if resolution >= 1080 {
                1080
            } else if resolution >= 720 {
                720
            } else {
                480
            },
            transition_mode: transition_mode.clamp(0, 2),
            aa_video_transition_mode: aa_video_transition_mode.clamp(0, 2),
            transition_speed: transition_speed.clamp(MIN_TRANSITION_SPEED, MAX_TRANSITION_SPEED),
            aa_video_transition_speed: aa_video_transition_speed
                .clamp(MIN_TRANSITION_SPEED, MAX_TRANSITION_SPEED),
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
            resolution: Some(self.resolution),
            transition_mode: Some(self.transition_mode),
            aa_video_transition_mode: Some(self.aa_video_transition_mode),
            transition_speed: Some(self.transition_speed),
            aa_video_transition_speed: Some(self.aa_video_transition_speed),
        };
        match toml::to_string_pretty(&file) {
            Ok(contents) => {
                if let Err(e) = std::fs::write(&self.path, contents) {
                    log::warn!("Failed to write config file {}: {e}", self.path.display());
                }
            }
            Err(e) => log::warn!("Failed to serialise config: {e}"),
        }
    }
}

/// Resolve the configuration file path: explicit (CLI/`EVA_CONFIG`) if given,
/// otherwise the default `config.toml` in the working directory.
fn config_path(explicit: Option<&PathBuf>) -> PathBuf {
    explicit
        .cloned()
        .unwrap_or_else(|| PathBuf::from("config.toml"))
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
