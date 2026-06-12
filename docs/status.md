# Status & Tracking

_Last updated: 2026-06-12_

## Summary

The project builds and runs. Core plumbing — Slint UI shell, background `android-auto`
runtime, video pipeline, touch forwarding, and audio I/O — is in place. Remaining work
is primarily feature build-out (settings, navigation, refinements).

## Implementation status

| Area | Status | Notes |
|------|--------|-------|
| Project scaffolding (crate, build script) | ✅ Done | `Cargo.toml`, `build.rs` |
| Slint UI shell (sidebar + view switching) | ✅ Done | `ui/app.slint` |
| Android Auto video view | ✅ Done | H.264 → RGB → `Image` |
| Settings view | 🟡 Placeholder | Static "coming soon" content |
| Background protocol runtime | ✅ Done | Tokio thread + `AndroidAutoContainer` |
| UI ↔ protocol bridge | ✅ Done | mpsc channels + 16 ms timer |
| Touch input forwarding | ✅ Done | `pointer-event` → `Wifi::TouchEvent` |
| Audio output (media/system/speech) | ✅ Done | `cpal` + `ringbuf` |
| Audio input (microphone) | ✅ Done | 16 kHz mono capture |
| Sensors (driving status, night mode) | ✅ Done | Reported on request |
| Wireless transport (Bluetooth + Wi-Fi) | ✅ Done | NetworkManager hotspot |
| USB transport | ✅ Done | Via `usb` feature |
| Connection status overlay | ✅ Done | `aa-connected` toggles overlay |

Legend: ✅ Done · 🟡 Partial / placeholder · ⬜ Not started

## Build prerequisites

System libraries required on the build host (Fedora package names):

- `dbus-devel`, `pkgconf-pkg-config` — D-Bus / pkg-config (libdbus-sys)
- `clang`, `clang-devel` — bindgen for `aws-lc-sys`
- `protobuf-compiler` — `protoc` for the `android-auto` build script
- `fontconfig-devel` — Slint font rendering (pulls in cairo/harfbuzz/freetype devel)

## Verification performed

- `cargo build` succeeds with no errors (only upstream `android-auto` warnings remain).
- Binary produced at `target/release/a310` / `target/debug/a310`.

## Roadmap / TODO

### Near term
- [ ] Flesh out the Settings view (real, interactive options).
- [ ] Surface connection state / errors in the UI beyond the waiting overlay.
- [ ] Make hotspot SSID/PSK configurable instead of hard-coded.
- [ ] Respect `HeadUnitInfo.left_hand` to mirror the sidebar for RHD.

### Medium term
- [ ] Navigation channel UI (turn-by-turn display).
- [ ] Multi-touch support (currently single pointer id 0).
- [ ] Decode pipeline option to run off the UI thread if needed.
- [ ] Configurable video resolution / DPI from settings.

### Longer term
- [ ] Persisted user preferences.
- [ ] Theming (day/night auto-switching tied to the night-mode sensor).
- [ ] Target validation on real automotive display hardware.

## Known limitations

- Hotspot credentials and network details are hard-coded in `main.rs`.
- Single-pointer touch only.
- Settings view is non-functional placeholder content.
- Video is decoded on the UI thread; very high resolutions may affect UI smoothness.
