//! System volume and screen backlight controls for the quick-controls view.
//!
//! Volume is driven through whichever mixer CLI is available (PipeWire's
//! `wpctl`, PulseAudio's `pactl`, or ALSA's `amixer`). Screen brightness is
//! driven directly through the kernel backlight sysfs interface so it changes
//! the *physical* panel brightness, not just the UI.

use std::path::PathBuf;
use std::process::Command;

/// Screen backlight controlled via `/sys/class/backlight/<device>`.
pub(crate) struct Backlight {
    /// Path to the writable `brightness` attribute.
    brightness_path: PathBuf,
    /// Maximum raw brightness value (`max_brightness`).
    max: u32,
}

impl Backlight {
    /// Discover the first usable backlight device, if any.
    pub(crate) fn discover() -> Option<Self> {
        let entries = std::fs::read_dir("/sys/class/backlight").ok()?;
        for entry in entries.flatten() {
            let base = entry.path();
            let Ok(max_str) = std::fs::read_to_string(base.join("max_brightness")) else {
                continue;
            };
            let Ok(max) = max_str.trim().parse::<u32>() else {
                continue;
            };
            if max == 0 {
                continue;
            }
            log::info!("Backlight device: {}", base.display());
            return Some(Self {
                brightness_path: base.join("brightness"),
                max,
            });
        }
        log::info!("No backlight device found under /sys/class/backlight");
        None
    }

    /// Current brightness as a 0.0–1.0 fraction of the maximum.
    pub(crate) fn get_fraction(&self) -> Option<f32> {
        let cur: u32 = std::fs::read_to_string(&self.brightness_path)
            .ok()?
            .trim()
            .parse()
            .ok()?;
        Some((cur as f32 / self.max as f32).clamp(0.0, 1.0))
    }

    /// Set brightness from a 0.0–1.0 fraction. A small floor keeps the panel
    /// from going fully dark (which would make the UI unreadable).
    pub(crate) fn set_fraction(&self, fraction: f32) {
        let raw = ((fraction.clamp(0.0, 1.0) * self.max as f32).round() as u32).max(1);
        if let Err(e) = std::fs::write(&self.brightness_path, raw.to_string()) {
            log::warn!(
                "Failed to set screen brightness ({}): {e}",
                self.brightness_path.display()
            );
        }
    }
}

/// Which mixer CLI backend drives the system volume.
enum VolumeBackend {
    /// PipeWire's `wpctl` (volume expressed as a 0.0–1.0 float).
    WpCtl,
    /// PulseAudio's `pactl` (volume expressed as a percentage).
    PaCtl,
    /// ALSA's `amixer` (volume expressed as a percentage).
    Amixer,
}

/// System audio volume controlled via the first available mixer CLI.
pub(crate) struct Volume {
    backend: VolumeBackend,
}

impl Volume {
    /// Pick the first available mixer backend in preference order.
    pub(crate) fn discover() -> Option<Self> {
        let backend = if command_exists("wpctl") {
            VolumeBackend::WpCtl
        } else if command_exists("pactl") {
            VolumeBackend::PaCtl
        } else if command_exists("amixer") {
            VolumeBackend::Amixer
        } else {
            log::info!("No volume mixer (wpctl/pactl/amixer) found");
            return None;
        };
        Some(Self { backend })
    }

    /// Current system volume as a 0.0–1.0 fraction, best effort.
    pub(crate) fn get_fraction(&self) -> Option<f32> {
        match self.backend {
            VolumeBackend::WpCtl => {
                let out = Command::new("wpctl")
                    .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
                    .output()
                    .ok()?;
                let text = String::from_utf8_lossy(&out.stdout);
                // Format: "Volume: 0.65" (possibly followed by "[MUTED]").
                text.split_whitespace()
                    .nth(1)?
                    .parse::<f32>()
                    .ok()
                    .map(|v| v.clamp(0.0, 1.0))
            }
            VolumeBackend::PaCtl => {
                let out = Command::new("pactl")
                    .args(["get-sink-volume", "@DEFAULT_SINK@"])
                    .output()
                    .ok()?;
                parse_percent(&String::from_utf8_lossy(&out.stdout))
            }
            VolumeBackend::Amixer => {
                let out = Command::new("amixer").args(["get", "Master"]).output().ok()?;
                parse_percent(&String::from_utf8_lossy(&out.stdout))
            }
        }
    }

    /// Set the system volume from a 0.0–1.0 fraction.
    pub(crate) fn set_fraction(&self, fraction: f32) {
        let frac = fraction.clamp(0.0, 1.0);
        let percent = (frac * 100.0).round() as u32;
        let result = match self.backend {
            VolumeBackend::WpCtl => Command::new("wpctl")
                .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{frac:.2}")])
                .status(),
            VolumeBackend::PaCtl => Command::new("pactl")
                .args(["set-sink-volume", "@DEFAULT_SINK@", &format!("{percent}%")])
                .status(),
            VolumeBackend::Amixer => Command::new("amixer")
                .args(["set", "Master", &format!("{percent}%")])
                .status(),
        };
        if let Err(e) = result {
            log::warn!("Failed to set system volume: {e}");
        }
    }
}

/// Return true if `cmd` is found as an executable file on `$PATH`.
fn command_exists(cmd: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(cmd).is_file())
}

/// Extract the first `NN%` value from mixer output as a 0.0–1.0 fraction.
fn parse_percent(text: &str) -> Option<f32> {
    let idx = text.find('%')?;
    let digits_start = text[..idx]
        .rfind(|c: char| !c.is_ascii_digit())
        .map(|i| i + 1)
        .unwrap_or(0);
    let pct: f32 = text[digits_start..idx].parse().ok()?;
    Some((pct / 100.0).clamp(0.0, 1.0))
}
