# Design Decisions

This document records the significant technology and design choices made for `a310`,
along with their rationale and trade-offs.

## Separate crate (`a310`)

**Decision:** Implement the GUI as its own crate, separate from the `android-auto`
example.

**Rationale:** Keeps the reusable library and the head-unit application cleanly
decoupled. The existing eframe/egui example under `android-auto/examples/main` remains
untouched as a reference implementation.

## Slint for the UI toolkit

**Decision:** Use [Slint](https://slint.dev) rather than egui (used by the example) or
GTK/Qt.

**Rationale:**

- Declarative `.slint` markup separates UI structure from application logic.
- Touch-first: `TouchArea` and pointer events map naturally to a touchscreen head unit.
- Lightweight and embeddable, with a path toward embedded/MCU targets later.
- Native rendering with a retained scene graph (no per-frame immediate-mode redraw of
  the whole UI), which suits a mostly-static cockpit layout with one video region.

**Trade-off:** Slint's font rendering pulls in system `fontconfig`/`freetype` on Linux,
adding build-time system dependencies (see Status doc).

## External `.slint` files compiled via `build.rs`

**Decision:** Put the UI under `ui/` (a `theme.slint`, reusable `components/`, per-screen
`views/`, and an `app.slint` composition root) and compile it with `slint-build`, rather
than the inline `slint::slint!` macro.

**Rationale:** A full UI is easier to maintain in dedicated files with proper syntax
support, and splitting by concern scales as views and widgets are added. `build.rs`
compiles `app.slint` and follows its imports automatically.

## Modular Rust layout

**Decision:** Split the application into focused modules — `messages`, `audio`, `video`,
`protocol`, `container`, and `ui` — leaving `main.rs` as a thin entry point.

**Rationale:** The original single-file `main.rs` mixed the trait impls, audio setup,
threading, decoding, and Slint wiring. Separating them keeps each concern independently
readable and makes adding views/features a localized change. The public `AppWindow` API
was preserved so the split required no behavioral changes.

## Threading: Slint on main thread, Tokio on a background thread

**Decision:** Run the Slint event loop on the main thread and the `android-auto` Tokio
runtime on a spawned `std::thread`.

**Rationale:** Windowing systems generally require the UI loop on the main thread, while
the protocol is inherently async. Isolating them avoids blocking UI rendering on
protocol/network work.

**Trade-off:** Requires explicit channel-based communication between the two worlds.

## UI-thread polling timer for message delivery

**Decision:** A `slint::Timer` (16 ms) drains the inbound channel on the UI thread,
forwarding video data to the decoder thread and applying connection-state changes to the
window properties.

**Rationale:**

- Naturally rate-limits UI updates to ~60 Hz and coalesces bursts of messages.
- Keeps property mutation on the Slint thread, where it must happen.

**Trade-off:** Up to ~16 ms of added latency on frame hand-off, which is acceptable for
this use case.

## Decode video on a dedicated thread

**Decision:** Run the H.264 decode loop on its own `std::thread` (`video::spawn_decoder`),
posting finished frames back to the UI via `slint::invoke_from_event_loop`.

**Rationale:** Decoding and YUV→RGB conversion are CPU-heavy; keeping them off the Slint
event loop ensures the UI never stalls on a slow or large frame. An earlier iteration
decoded inside the UI timer, but that risked frame-time spikes blocking rendering.

**Trade-off:** `slint::Image` is not `Send`, so the decoder sends raw RGB bytes and the
UI thread wraps them in an `Image` (a cheap copy).

## Audio via `cpal` + `ringbuf`

**Decision:** Reuse the example's `cpal`/`ringbuf` audio approach for the three output
channels (media 48 kHz stereo, system & speech 16 kHz mono) and 16 kHz mono input.

**Rationale:** Proven pattern from the existing example; lock-free ring buffers decouple
the protocol's audio delivery from the audio device callback.

## Layout and theme for in-car use

**Decision:** Left sidebar with large nav buttons; a red-on-black "Central Dogma"
aesthetic with all colors and animation timings centralized in `ui/theme.slint`.

**Rationale:**

- Big targets are reachable and tappable while driving.
- Sidebar on the left keeps controls near the driver in a left-hand-drive layout.
- A near-black palette reduces glare in the cockpit; the single accent color keeps the
  cockpit calm and legible.
- Centralizing palette + durations in one global makes restyling a one-file change and
  keeps transitions consistent across views.

**Future:** The side could be flipped based on `HeadUnitInfo.left_hand`.

## Animated view and connection transitions

**Decision:** Animate view switching (slide + fade via `ViewSlot`), video connect
(fade-in from black), and disconnect (overlay crossfade over the last frame).

**Rationale:** Smooth transitions read as a polished, intentional head unit rather than
abrupt swaps, and the connect/disconnect fades hide the brief protocol gaps. Clearing the
stale frame on connect (rather than disconnect) lets the disconnect crossfade play over
the last real frame.

## Wireless + USB both enabled

**Decision:** Depend on `android-auto` with both `usb` and `wireless` features.

**Rationale:** Supports both wired and Bluetooth-initiated wireless connections from a
single build, matching a real head unit's flexibility.
