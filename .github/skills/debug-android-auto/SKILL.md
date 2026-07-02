---
name: debug-android-auto
description: 'Debug the eva-navigation-unit / eva-ui app using its tracing logs and trace tooling. Use to investigate Android Auto protocol issues (USB/Wi-Fi/Bluetooth handshake, SSL frames, channel open, video/audio, pings), diagnose crashes, panics or hangs, capture a JSON bug-report log, or profile async tasks. Covers per-component log levels (UI/Audio/AA/BT), EVA_LOG_* env vars, --log-* CLI flags, config.toml [log] table, RUST_LOG, plus lnav, Perfetto/chrome-trace and tokio-console.'
argument-hint: 'what you are investigating (e.g. "wireless handshake never completes", "crash on video channel open")'
---

# Debugging eva-navigation-unit with Logs & Traces

`eva-navigation-unit` (the eva-ui binary) and the `android-auto` library both emit structured
[`tracing`](https://docs.rs/tracing) events through a single subscriber. Use this
skill to pick the right verbosity, capture the right signal, and pin down Android
Auto protocol failures or crashes.

Full reference: [docs/debug-pipeline.md](../../../docs/debug-pipeline.md).

## When to Use
- An Android Auto session won't start, drops, or stalls (USB/Wi-Fi/Bluetooth pairing, handshake, channel open, pings).
- Video or audio channels fail to open or behave oddly.
- The app panics, hangs, or exits unexpectedly and you need the surrounding events.
- You need a clean, shareable log capture for a bug report.
- You want to inspect async task state or span timings.

## Component Map (what to turn up)

| Component | Module paths | Turn up for… |
|-----------|--------------|--------------|
| **AA**    | `eva_navigation_unit::container`, `eva_navigation_unit::protocol`, `android_auto::{lib, control, ssl, common, video}` | session lifecycle, SSL frames, channel open, version handshake |
| **BT**    | `eva_navigation_unit::hostapd`, `android_auto::{bluetooth, usb}` | pairing, RFCOMM, USB/AOA transport, raw transfer bytes |
| **Audio** | `eva_navigation_unit::audio`, `android_auto::{mediaaudio, speechaudio, sysaudio}` | media/voice/system audio streams |
| **UI**    | `eva_navigation_unit::ui`, `eva_navigation_unit::controls`, `eva_navigation_unit::gfx` | Slint UI, controls, rendering |

Levels (low → high verbosity): `error`, `warn`, `info`, `debug`, `trace`.

> `android_auto::usb` dumps raw transfer bytes at `trace` (very noisy). It is
> pinned to `warn` unless you set a **BT** level explicitly.

## Symptom → what to trace

| Symptom | Likely stage / module | Start here |
|---------|-----------------------|------------|
| Phone never detected over USB | AOA enumeration, `android_auto::usb`, `android_auto::lib` | `--log-bt trace` |
| Wireless device won't pair / connect | Bluetooth RFCOMM + Wi-Fi listener, `android_auto::{bluetooth, lib}` | `--log-bt trace` |
| Connects then stalls before UI | version handshake / SSL, `android_auto::{lib, ssl, control}` | `--log-aa trace` |
| `SSL Handshake complete` never logged | SSL setup, `android_auto::ssl` | `RUST_LOG="android_auto::ssl=trace"` |
| Session drops after a few seconds | ping/handshake watchdogs, `android_auto::lib` | `--log-aa debug` (watch for watchdog errors) |
| Black screen / no video | channel open + video, `android_auto::video`, `eva_navigation_unit::protocol` | `--log-aa debug` |
| No / choppy audio | `android_auto::{mediaaudio, speechaudio, sysaudio}`, `eva_navigation_unit::audio` | `--log-audio debug` |
| Sluggish UI / rendering glitches | `eva_navigation_unit::{ui, controls, gfx}` | `--log-ui debug` |
| Panic / unexpected exit | whole stack | see crash procedure below |

## Procedure: Investigate an Android Auto protocol issue

1. **Reproduce at info first** to see the lifecycle skeleton (server start, device
   found, client connected, version, handshake complete, disconnect):
   ```sh
   cargo run -- --log-level info
   ```
2. **Narrow to the failing subsystem.** Keep global noise low, turn the suspect
   component to `debug`, then `trace` if needed:
   - Pairing / transport (USB, Wi-Fi listener, Bluetooth):
     ```sh
     cargo run -- --log-level warn --log-bt trace
     ```
   - Handshake / SSL frames / channel open / session lifecycle:
     ```sh
     cargo run -- --log-level warn --log-aa trace
     ```
   - Audio streams:
     ```sh
     cargo run -- --log-level warn --log-audio debug
     ```
3. **Read the story in order.** Key info-level signposts: `Running android auto
   server`, device-found events, `Android Auto client version`, `SSL Handshake
   complete`, `Video channel opened`, `Android Auto session ended`. A missing
   signpost localizes the failure stage.
4. **Pinpoint with `RUST_LOG`** when you know the exact module, silencing the rest:
   ```sh
   RUST_LOG="android_auto::ssl=trace,android_auto::lib=debug,eva_navigation_unit::ui=off" cargo run
   ```
5. **Inspect frame/byte detail at `trace`** (TX/RX control frames in `ssl`, raw
   USB bytes in `usb`) only once you've localized the stage — these are high-volume.

## Procedure: Diagnose a crash, panic, or hang

1. **Capture full backtraces** alongside debug logs:
   ```sh
   RUST_BACKTRACE=full cargo run -- --log-level debug
   ```
2. **Record to a JSON file** so the events right before the crash survive and are
   tool-filterable:
   ```sh
   cargo run -- --log-level debug --log-file /tmp/eva-crash.log --log-format json
   ```
3. **Read the tail** to see the last events before exit:
   ```sh
   lnav /tmp/eva-crash.log        # or: tail -n 50 /tmp/eva-crash.log
   ```
4. **Suspected async hang/deadlock?** Watch live task state with `tokio-console`:
   ```sh
   # Terminal 1
   RUSTFLAGS="--cfg tokio_unstable" cargo run --features tokio-console
   # Terminal 2
   tokio-console
   ```
   Look for tasks stuck `Idle`/`Busy` that never complete (e.g. a watchdog, pinger,
   or SSL reader task).

## Capture a clean bug report

```sh
cargo run -- --log-level debug --log-file /tmp/eva-bug.log --log-format json
```
Attach `/tmp/eva-bug.log`. Bump the relevant component to `trace` (`--log-aa trace`
or `--log-bt trace`) if the issue is protocol-specific.

## Profile span timings

```sh
cargo run --features chrome-trace
# → writes trace-<timestamp>.json in the CWD; open at https://ui.perfetto.dev
```

## Setting levels persistently

CLI > `EVA_*` env vars > `config.toml [log]` table > defaults; `RUST_LOG` layers on
top. For a recurring investigation, set it once in `config.toml`:

```toml
[log]
level  = "warn"
aa     = "trace"
file   = "/tmp/eva-ui.log"
format = "json"
```

Env-var equivalents: `EVA_LOG_LEVEL`, `EVA_LOG_AA`, `EVA_LOG_BT`, `EVA_LOG_AUDIO`,
`EVA_LOG_UI`, `EVA_LOG_FILE`, `EVA_LOG_FORMAT`.

## On a deployed device

The release build is installed as a desktop app (`~/.local/bin/eva-ui`, config at
`~/.config/eva-ui/config.toml`), so there is no journald unit for the app itself —
its stdout is swallowed by the desktop launcher. To debug on-target:

1. **Run it from a terminal/SSH** instead of the launcher so you see console output:
   ```sh
   EVA_LOG_LEVEL=debug EVA_LOG_AA=trace ~/.local/bin/eva-ui
   ```
2. **Or enable persistent file logging** in `~/.config/eva-ui/config.toml`, then
   reproduce via the normal launcher and pull the file off afterwards:
   ```toml
   [log]
   level  = "warn"
   aa     = "trace"
   file   = "/tmp/eva-ui.log"
   format = "json"
   ```
   ```sh
   # from your dev machine
   scp <device>:/tmp/eva-ui.log .
   lnav eva-ui.log
   ```
3. **Hotspot / pairing side** runs as a real systemd service — check it when
   wireless pairing or the AP misbehaves:
   ```sh
   journalctl -u eva-hotspot.service -b --no-pager      # this boot
   journalctl -u eva-hotspot.service -f                 # live follow
   ```

Remember to lower the level again after capturing — `trace` file logging grows fast.
