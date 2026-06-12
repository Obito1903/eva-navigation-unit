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

## External `.slint` file compiled via `build.rs`

**Decision:** Put the UI in `ui/app.slint` and compile it with `slint-build`, rather
than the inline `slint::slint!` macro.

**Rationale:** A full UI is easier to maintain in a dedicated file with proper syntax
support, and it scales better as views are added.

## Threading: Slint on main thread, Tokio on a background thread

**Decision:** Run the Slint event loop on the main thread and the `android-auto` Tokio
runtime on a spawned `std::thread`.

**Rationale:** Windowing systems generally require the UI loop on the main thread, while
the protocol is inherently async. Isolating them avoids blocking UI rendering on
protocol/network work.

**Trade-off:** Requires explicit channel-based communication between the two worlds.

## UI-thread polling timer instead of `invoke_from_event_loop`

**Decision:** A `slint::Timer` (16 ms) drains the inbound channel and updates the UI,
rather than pushing each frame from the background thread via `invoke_from_event_loop`.

**Rationale:**

- Video decoding (`openh264`) happens on the UI thread inside the timer, so the decoded
  `SharedPixelBuffer` can be assigned to the window property directly.
- Naturally rate-limits UI updates to ~60 Hz and coalesces bursts of messages.
- Simpler ownership: the decoder and window handle live together in the timer closure.

**Trade-off:** Up to ~16 ms of added latency on frame presentation, which is acceptable
for this use case.

## Decode video on the UI thread

**Decision:** Run the H.264 decode loop inside the UI timer callback.

**Rationale:** Keeps the decoder, RGB conversion, and `Image` assignment colocated and
single-threaded, avoiding cross-thread image transfer. If decode cost becomes a
bottleneck it can be moved to the background thread, sending decoded RGB buffers instead
of raw H.264.

## Audio via `cpal` + `ringbuf`

**Decision:** Reuse the example's `cpal`/`ringbuf` audio approach for the three output
channels (media 48 kHz stereo, system & speech 16 kHz mono) and 16 kHz mono input.

**Rationale:** Proven pattern from the existing example; lock-free ring buffers decouple
the protocol's audio delivery from the audio device callback.

## Layout for in-car use

**Decision:** Left sidebar with large (≥72 px) buttons; dark theme (`#0d0d1a` /
`#1a1a2e`).

**Rationale:**

- Big targets are reachable and tappable while driving.
- Sidebar on the left keeps controls near the driver in a left-hand-drive layout.
- Dark palette reduces glare in the cockpit.

**Future:** The side could be flipped based on `HeadUnitInfo.left_hand`.

## Wireless + USB both enabled

**Decision:** Depend on `android-auto` with both `usb` and `wireless` features.

**Rationale:** Supports both wired and Bluetooth-initiated wireless connections from a
single build, matching a real head unit's flexibility.
