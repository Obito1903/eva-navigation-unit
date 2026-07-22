# Configuration

`eva-navigation-unit` is configured through a TOML file, environment
variables, and CLI flags. Settings changed from the in-app Settings screen are
persisted back to the same TOML file.

## Precedence

Highest wins:

1. **CLI arguments** — e.g. `--min-dpi 120`
2. **Environment variables** — e.g. `EVA_MIN_DPI=120`
3. **Config file (TOML)** — see [Config file location](#config-file-location)
4. **Built-in defaults**

## Config file location

Resolved in this order:

1. An explicit path passed via `--config <path>` or the `EVA_CONFIG`
   environment variable.
2. A `config.toml` file in the current working directory, if present
   (convenient for development — e.g. running via `cargo run` from the repo
   root).
3. The per-user config at `$XDG_CONFIG_HOME/eva-ui/config.toml`, falling back
   to `~/.config/eva-ui/config.toml` when `XDG_CONFIG_HOME` isn't set.

If none of the above exist yet, the app falls back to built-in defaults and
will create the per-user path the first time it saves configuration (e.g.
from the Settings screen).

## Options

All options are optional — omit a key/flag to keep its default. Env vars and
CLI flags always take precedence over the config file.

### General

| Config key (TOML) | CLI flag | Env var | Default | Description |
|---|---|---|---|---|
| `min_dpi` | `--min-dpi` | `EVA_MIN_DPI` | `80` | Minimum selectable Android Auto DPI. |
| `max_dpi` | `--max-dpi` | `EVA_MAX_DPI` | `320` | Maximum selectable Android Auto DPI. |
| `dpi` | `--dpi` | `EVA_DPI` | `160` | Current Android Auto DPI (clamped to `[min_dpi, max_dpi]`). |
| `wireless` | `--wireless` | `EVA_WIRELESS` | `true` | Enable wireless Android Auto. |
| `usb` | `--usb` | `EVA_USB` | `true` | Enable USB (wired) Android Auto. |
| `reset_stale_accessory` | `--reset-stale-accessory` | `EVA_RESET_STALE_ACCESSORY` | `true` | Reset a USB phone left in AOA accessory mode at startup, clearing a stale session from a previous run. Disable on controllers that misbehave on USB reset (e.g. Nintendo Switch Tegra xHCI). |
| `resolution` | `--resolution` | `EVA_RESOLUTION` | `720` | Android Auto video vertical resolution. Snapped to `480`, `720`, or `1080`. |
| `fps` | `--fps` | `EVA_FPS` | `30` | Android Auto video frame rate. Snapped to `30` or `60`. |
| `transition_mode` | `--transition-mode` | `EVA_TRANSITION_MODE` | `0` | View transition mode: `0` = CRT, `1` = FADE, `2` = SLIDE. |
| `aa_video_transition_mode` | `--aa-video-transition-mode` | `EVA_AA_VIDEO_TRANSITION_MODE` | `1` | Android Auto video start/stop transition: `0` = CRT, `1` = FADE, `2` = SLIDE. |
| `transition_speed` | `--transition-speed` | `EVA_TRANSITION_SPEED` | `1.0` | View transition speed multiplier. Range `0.25`–`3.0`; higher is faster. |
| `aa_video_transition_speed` | `--aa-video-transition-speed` | `EVA_AA_VIDEO_TRANSITION_SPEED` | `1.0` | Android Auto video transition speed multiplier. Range `0.25`–`3.0`. |
| `theme` | `--theme` | `EVA_THEME` | `0` | Color theme: `0` = NERV-HQ, `1` = MATRIX. |
| `gfx_model` | `--gfx-model` | `EVA_GFX_MODEL` | `0` | GL underlay wireframe model: `0` = sphere, `1` = cube, `2` = car, `3` = speaker. |
| `fullscreen` | `--fullscreen` | `EVA_FULLSCREEN` | `false` | Start the window in fullscreen mode. |
| `hotspot_backend` | `--hotspot-backend` | `EVA_HOTSPOT_BACKEND` | `0` | Wi-Fi hotspot backend for Android Auto wireless: `0` = NetworkManager, `1` = hostapd. See [hostapd install instructions](../README.md#installing-the-wi-fi-hotspot-service-for-android-auto-wireless). |
| `hotspot_channel` | `--hotspot-channel` | `EVA_HOTSPOT_CHANNEL` | `36` | 5 GHz Wi-Fi channel used by the `hostapd` backend (`0` = automatic). Ignored by the NetworkManager backend. |
| `car_name_short` | `--car-name-short` | `EVA_CAR_NAME_SHORT` | `"NERV"` | Header/branding text shown at the top of the sidebar. |
| `app_name` | `--app-name` | `EVA_APP_NAME` | `"EVA-02"` | App name text shown on the Android Auto "locked terminal" overlay. |
| `car_name_long` | `--car-name-long` | `EVA_CAR_NAME_LONG` | `"EVA NAVIGATION UNIT"` | Long car name text shown on the Android Auto "locked terminal" overlay. |
| `aa_waiting_text` | `--aa-waiting-text` | `EVA_AA_WAITING_TEXT` | `"WAITING FOR ENTRY PLUG"` | Waiting-for-connection text shown on the Android Auto "locked terminal" overlay. |

The version badge on the same overlay always reflects the actual build
version (`CARGO_PKG_VERSION`) and is not configurable.

Additionally, `--config <path>` / `EVA_CONFIG` selects an explicit config file
path (see [Config file location](#config-file-location)); it has no
corresponding config-file key.

### Logging (`[log]` table)

See [Debug & Logging Pipeline](debug-pipeline.md) for the full guide to
per-component log filtering.

| Config key (TOML) | CLI flag | Env var | Default | Description |
|---|---|---|---|---|
| `log.level` | `--log-level` | `EVA_LOG_LEVEL` | `"info"` | Global log level: `error`, `warn`, `info`, `debug`, `trace`. |
| `log.ui` | `--log-ui` | `EVA_LOG_UI` | _(unset)_ | Log level override for the UI component. |
| `log.audio` | `--log-audio` | `EVA_LOG_AUDIO` | _(unset)_ | Log level override for the Audio component. |
| `log.aa` | `--log-aa` | `EVA_LOG_AA` | _(unset)_ | Log level override for the Android Auto (AA) component. |
| `log.bt` | `--log-bt` | `EVA_LOG_BT` | _(unset)_ | Log level override for the Bluetooth/transport (BT) component. |
| `log.file` | `--log-file` | `EVA_LOG_FILE` | _(unset)_ | Also write logs to this file path (console output is always on). |
| `log.format` | `--log-format` | `EVA_LOG_FORMAT` | `"text"` | Log output format: `text` or `json`. |

### Spectrum visualizer (`[viz]` table)

| Config key (TOML) | CLI flag | Env var | Default | Description |
|---|---|---|---|---|
| `viz.bands` | `--viz-bands` | `EVA_VIZ_BANDS` | `32` | Number of frequency bands shown. Clamped to `4..=64`. |
| `viz.fft_size` | `--viz-fft-size` | `EVA_VIZ_FFT_SIZE` | `2048` | FFT window size in samples. Rounded down to the nearest power of two in `512..=8192`. |
| `viz.hop` | `--viz-hop` | `EVA_VIZ_HOP` | `256` | Hop size in samples — how many new samples trigger one FFT update. Smaller = lower latency, more CPU. Clamped to `64..=fft_size/2`. |
| `viz.freq_min` | `--viz-freq-min` | `EVA_VIZ_FREQ_MIN` | `20.0` | Lowest frequency shown (Hz). Clamped to `1.0..=23000.0`. |
| `viz.freq_max` | `--viz-freq-max` | `EVA_VIZ_FREQ_MAX` | `20000.0` | Highest frequency shown (Hz). Clamped to `(freq_min + 100.0)..=24000.0`. |
| `viz.input_attack_ms` | `--viz-input-attack-ms` | `EVA_VIZ_INPUT_ATTACK_MS` | `20.0` | Input pre-smoother attack time constant (ms) — how quickly bars rise on transients. Clamped to `1.0..=500.0`. |
| `viz.input_release_ms` | `--viz-input-release-ms` | `EVA_VIZ_INPUT_RELEASE_MS` | `60.0` | Input pre-smoother release time constant (ms) — how fast noise between FFT frames is suppressed on the falling edge. Clamped to `1.0..=2000.0`. |
| `viz.gravity` | `--viz-gravity` | `EVA_VIZ_GRAVITY` | `1.0` | Gravity fall-speed multiplier (`1.0` = CAVA default). Higher = bars fall faster after a transient. Clamped to `0.1..=10.0`. |
| `viz.noise_reduction` | `--viz-noise-reduction` | `EVA_VIZ_NOISE_REDUCTION` | `0.0` | Leaky-integrator noise-reduction factor. `0.0` = auto-calibrate from the measured framerate (recommended); values in `(0, 1)` override it — higher is heavier smoothing. Clamped to `0.0..=0.99`. |
| `viz.bar_gap` | `--viz-bar-gap` | `EVA_VIZ_BAR_GAP` | `0.08` | Horizontal gap between bar columns, as a fraction of slot width. Clamped to `0.0..=0.45`. |
| `viz.seg_gap_px` | `--viz-seg-gap-px` | `EVA_VIZ_SEG_GAP_PX` | `2.0` | Vertical gap between segment rows, in pixels. Clamped to `0.0..=20.0`. |
| `viz.seg_count` | `--viz-seg-count` | `EVA_VIZ_SEG_COUNT` | `50` | Number of discrete vertical VFD segments per bar column. Clamped to `8..=120`. |

### OBD2 telemetry (`[obd2]` table, requires the `obd2` feature)

See [OBD2 Telemetry](obd2.md) for the full guide — connecting to a Bluetooth
ELM327, defining PIDs with custom formulas, and known limitations (this is
early plumbing with no UI yet).

| Config key (TOML) | CLI flag | Env var | Default | Description |
|---|---|---|---|---|
| `obd2.enabled` | `--obd2-enabled` | `EVA_OBD2_ENABLED` | `false` | Enable the OBD2 worker. |
| `obd2.device_address` | `--obd2-device-address` | `EVA_OBD2_DEVICE_ADDRESS` | _(unset)_ | Bluetooth MAC address of the paired ELM327. |
| `obd2.poll_interval_ms` | `--obd2-poll-interval-ms` | `EVA_OBD2_POLL_INTERVAL_MS` | `250` | Poll interval for all configured PIDs, in milliseconds. |
| `obd2.pids` | _(none)_ | _(none)_ | `[]` | Array of PID definitions (`[[obd2.pids]]`); TOML-file only. See [OBD2 Telemetry](obd2.md#defining-pids-obd2pids). |

## Example `config.toml`

```toml
min_dpi = 80
max_dpi = 320
dpi = 160
wireless = true
usb = true
reset_stale_accessory = true
resolution = 720
fps = 30
transition_mode = 0
aa_video_transition_mode = 1
transition_speed = 1.0
aa_video_transition_speed = 1.0
theme = 0
gfx_model = 0
fullscreen = false
hotspot_backend = 0
hotspot_channel = 36
car_name_short = "NERV"
app_name = "EVA-02"
car_name_long = "EVA NAVIGATION UNIT"
aa_waiting_text = "WAITING FOR ENTRY PLUG"

[log]
level  = "info"
aa     = "debug"
file   = "/tmp/eva-ui.log"
format = "text"

[viz]
bands              = 32
fft_size           = 2048
hop                = 256
freq_min           = 20.0
freq_max           = 20000.0
input_attack_ms    = 20.0
input_release_ms   = 60.0
gravity            = 1.0
noise_reduction    = 0.0
bar_gap            = 0.08
seg_gap_px         = 2.0
seg_count          = 50
```

Every key is optional — start from a minimal file (or none at all) and only
add the options you want to override.
