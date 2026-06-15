# Status & Tracking

_Last updated: 2026-06-12_

## Summary

The project builds and runs. Core plumbing — Slint UI shell, background `android-auto`
runtime, threaded video decode, touch forwarding, and audio I/O — is in place, and the
codebase has been refactored into focused Rust modules and split Slint files. Remaining
work is primarily feature build-out (settings, navigation, refinements).

## Implementation status

| Area | Status | Notes |
|------|--------|-------|
| Project scaffolding (crate, build script) | ✅ Done | `Cargo.toml`, `build.rs` |
| Modular Rust layout | ✅ Done | `messages` / `audio` / `video` / `protocol` / `container` / `ui` |
| Slint UI split (theme / components / views) | ✅ Done | `ui/theme.slint`, `ui/components/`, `ui/views/` |
| Slint UI shell (sidebar + view switching) | ✅ Done | Model-driven `Sidebar` + `ViewSlot`s |
| Android Auto video view | ✅ Done | H.264 → RGB → `Image` |
| Settings view | 🟡 Placeholder | Static content |
| Background protocol runtime | ✅ Done | Tokio thread + `AndroidAutoContainer` |
| UI ↔ protocol bridge | ✅ Done | mpsc channels + 16 ms timer |
| Threaded video decoder | ✅ Done | Dedicated decoder thread (`video::spawn_decoder`) |
| Touch input forwarding | ✅ Done | `pointer-event` → `Wifi::TouchEvent` |
| Audio output (media/system/speech) | ✅ Done | `cpal` + `ringbuf` |
| Audio input (microphone) | ✅ Done | 16 kHz mono capture |
| Sensors (driving status, night mode) | ✅ Done | Reported on request |
| Wireless transport (Bluetooth + Wi-Fi) | ✅ Done | NetworkManager hotspot |
| USB transport | ✅ Done | Via `usb` feature |
| NERV "Central Dogma" theme | ✅ Done | Centralized in `ui/theme.slint` |
| View / connect / disconnect transitions | ✅ Done | Slide+fade views, video fade-in, overlay crossfade |
| Disconnect/reconnect robustness | ✅ Done | Frame cleanup; library no longer panics on transient USB/channel faults |

Legend: ✅ Done · 🟡 Partial / placeholder · ⬜ Not started

## Build prerequisites

System libraries required on the build host (Fedora package names):

- `dbus-devel`, `pkgconf-pkg-config` — D-Bus / pkg-config (libdbus-sys)
- `clang`, `clang-devel` — bindgen for `aws-lc-sys`
- `protobuf-compiler` — `protoc` for the `android-auto` build script
- `fontconfig-devel` — Slint font rendering (pulls in cairo/harfbuzz/freetype devel)

## Verification performed

- `cargo build --release` succeeds with no errors (only upstream `android-auto` warnings
  remain).
- Binary produced at `target/release/a310` / `target/debug/a310`.
- Connected to a Pixel 8 Pro over USB: handshake completes, video streams, audio plays,
  and the connect/disconnect cycle recovers cleanly.

## Roadmap / TODO

### Near term

- [ ] Flesh out the Settings view (real, interactive options).
- [ ] Surface connection state / errors in the UI beyond the waiting overlay.
- [ ] Make hotspot SSID/PSK configurable instead of hard-coded.
- [ ] Respect `HeadUnitInfo.left_hand` to mirror the sidebar for RHD.
- [ ] Complete logs/debug pipeline
  - [ ] Add "debug" view that allows change log level of different components of the app (BT, AA, Audio, UI)
- [ ] Scale setting for the Slint UI

### Medium term

- [ ] Navigation channel UI (turn-by-turn display).
- [ ] Multi-touch support (currently single pointer id 0).
- [ ] Configurable video resolution / DPI from settings.
- [ ] OBD2 Integration
  - [ ] Dedicated view
  - [ ] Send car info to AA
  - [ ] Gauge display
- [ ] BT managment view
- [ ] Import any 3D mesh for background render
- [ ] Improve UI for CAR use

### Longer term

- [ ] Persisted user preferences.
- [ ] OpenGL/Vulkan audio visualizer

## Known limitations

- Single-pointer touch only.
