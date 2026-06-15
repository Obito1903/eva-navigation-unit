# Debug & Logging Pipeline

`a310` uses [`tracing`](https://docs.rs/tracing) with
[`tracing-subscriber`](https://docs.rs/tracing-subscriber) for structured,
per-component logging. Logs from the [`android-auto`](../../android-auto) library
(which emits through the `log` facade) are bridged in automatically, so a single
configuration controls the whole stack.

Console output is always on. File output and the local visualisation backends
are opt-in.

## Components

Filtering is organised around four logical components, each mapped to a set of
module paths. This keeps log call sites untouched while still allowing you to
raise or lower verbosity per subsystem.

| Component | Module paths |
|-----------|--------------|
| **UI**    | `a310::ui`, `a310::controls`, `a310::gfx` |
| **Audio** | `a310::audio`, `android_auto::{mediaaudio, speechaudio, sysaudio}` |
| **AA**    | `a310::container`, `a310::protocol`, `android_auto::{lib, control, ssl, common, video}` |
| **BT**    | `a310::hostapd`, `android_auto::{bluetooth, usb}` |

Levels (lowest → highest verbosity): `error`, `warn`, `info`, `debug`, `trace`.

> `android_auto::usb` logs raw transfer bytes at `info`, which is extremely
> noisy. It is pinned to `warn` by default unless you set a **BT** component
> level explicitly.

## Controlling log levels

Configuration follows the usual precedence (highest wins):

1. CLI arguments
2. Environment variables (`EVA_*`)
3. Config file `[log]` table
4. Built-in defaults (`level = "info"`, `format = "text"`)

### CLI flags

```sh
# Global level
cargo run -- --log-level debug

# Per-component overrides
cargo run -- --log-bt trace --log-ui warn

# Write to a file as JSON
cargo run -- --log-file /tmp/eva-ui.log --log-format json
```

### Environment variables

```sh
EVA_LOG_LEVEL=debug   # global default
EVA_LOG_UI=warn       # UI component
EVA_LOG_AUDIO=debug   # Audio component
EVA_LOG_AA=trace      # Android Auto component
EVA_LOG_BT=trace      # Bluetooth/transport component
EVA_LOG_FILE=/tmp/eva-ui.log
EVA_LOG_FORMAT=json   # text | json
```

### Config file

Add a `[log]` table to `config.toml` (all keys optional):

```toml
[log]
level  = "info"            # global default
ui     = "debug"           # per-component overrides
audio  = "info"
aa     = "debug"
bt     = "trace"
file   = "/tmp/eva-ui.log" # omit for console only
format = "json"            # text | json
```

### `RUST_LOG` escape hatch

`RUST_LOG`, when set, is layered on top of the resolved configuration, so you
keep full [`EnvFilter`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)
control for ad-hoc debugging:

```sh
RUST_LOG="android_auto::ssl=trace,a310::ui=off" cargo run
```

## File logging

File output is **opt-in**. Set `--log-file` / `EVA_LOG_FILE` / `[log].file` to a
path; the parent directory is created if needed. Console output continues
regardless. Use `format = "json"` for machine-readable, tool-friendly logs.

## Local visualisation

No external infrastructure is required — these are all local tools.

### `lnav` — log file navigator

Best paired with JSON file output. Provides filtering, histograms and SQL over
your logs.

```sh
cargo run -- --log-file /tmp/eva-ui.log --log-format json
lnav /tmp/eva-ui.log
```

### Perfetto / Chrome trace — span timelines

Build with the `chrome-trace` feature to emit a Chrome-format trace file
(`trace-*.json`) into the working directory. Open it in
<https://ui.perfetto.dev> (or `chrome://tracing`) to inspect span timelines and
flamegraphs.

```sh
cargo run --features chrome-trace
# → produces trace-<timestamp>.json in the CWD
```

### `tokio-console` — live async task view

Build with the `tokio-console` feature and the required unstable cfg, then
attach the [`tokio-console`](https://docs.rs/tokio-console) CLI to watch the
async protocol runtime live.

```sh
# Terminal 1 — run the app
RUSTFLAGS="--cfg tokio_unstable" cargo run --features tokio-console

# Terminal 2 — attach the inspector
tokio-console
```

## Recipes

**Trace only Bluetooth/transport:**

```sh
cargo run -- --log-level warn --log-bt trace
```

**Capture a JSON log file for a bug report:**

```sh
cargo run -- --log-level debug --log-file /tmp/eva-bug.log --log-format json
```

**Inspect the live async runtime:**

```sh
RUSTFLAGS="--cfg tokio_unstable" cargo run --features tokio-console
tokio-console
```

**Profile span timings in Perfetto:**

```sh
cargo run --features chrome-trace
# open the generated trace-*.json at https://ui.perfetto.dev
```
