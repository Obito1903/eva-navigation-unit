//! Wires the Slint window to the android-auto worker: forwards touch events,
//! spawns the video decoder, and pumps worker → UI messages on a timer.

use crate::container::AndroidAutoContainer;
use crate::messages::{MessageFromAsync, MessageToAsync, VideoCommand};
use crate::video;
use crate::AppWindow;
use slint::ComponentHandle;

/// How often the UI thread drains messages coming from the worker.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);

/// Connect the window's callbacks and the worker container, and start the
/// polling timer. The returned timer is leaked into the event loop for the
/// lifetime of the program.
pub(crate) fn wire(window: &AppWindow, setup: android_auto::AndroidAutoSetup) {
    let window_weak = window.as_weak();

    // ── Start android-auto in background ──────────────────────────────────
    let mut container = AndroidAutoContainer::new(setup);
    let send_touch = container.send.clone();

    // ── Touch events: Slint UI → android-auto ─────────────────────────────
    window.on_touch_event(move |x, y, kind| {
        let msg = build_touch_message(x, y, kind);
        let _ = send_touch.blocking_send(MessageToAsync::AndroidAutoMessage(msg));
    });

    // ── Video decoder thread ──────────────────────────────────────────────
    let (video_tx, video_rx) = std::sync::mpsc::channel::<VideoCommand>();
    video::spawn_decoder(video_rx, window_weak.clone());

    // ── Poll worker → UI messages ─────────────────────────────────────────
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, POLL_INTERVAL, move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };

        while let Ok(msg) = container.recv.try_recv() {
            match msg {
                MessageFromAsync::ExitContainer => {
                    log::info!("Container exited — restarting");
                    container = AndroidAutoContainer::new(setup);
                }
                MessageFromAsync::Connected => {
                    log::info!("Android Auto connected");
                    // Clear any stale frame so the stream fades in from black
                    // rather than briefly showing the previous session's frame.
                    win.set_video_frame(slint::Image::default());
                    win.set_aa_connected(true);
                }
                MessageFromAsync::Disconnected => {
                    log::info!("Android Auto disconnected");
                    win.set_aa_connected(false);
                    let _ = video_tx.send(VideoCommand::Flush);
                    // Keep the last frame mounted so the locked overlay can
                    // crossfade over it; it is cleared on the next connect.
                }
                MessageFromAsync::VideoData { data, .. } => {
                    // Hand the raw H.264 off to the decoder thread; do not
                    // decode on the UI thread or the event loop will stall.
                    let _ = video_tx.send(VideoCommand::Frame(data));
                }
            }
        }
    });

    // Keep the timer alive for the lifetime of the program.
    std::mem::forget(timer);
}

/// Build an android-auto touch input message from UI-space coordinates.
/// `kind`: 0 = down, 2 = up, anything else = drag.
fn build_touch_message(x: f32, y: f32, kind: i32) -> android_auto::SendableAndroidAutoMessage {
    let mut i_event = android_auto::Wifi::InputEventIndication::new();
    let timestamp: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64;
    i_event.set_timestamp(timestamp);

    let mut te = android_auto::Wifi::TouchEvent::new();
    let mut tl = android_auto::Wifi::TouchLocation::new();
    tl.set_x(x as u32);
    tl.set_y(y as u32);
    tl.set_pointer_id(0);
    te.touch_location = vec![tl];

    let action = match kind {
        0 => android_auto::Wifi::touch_action::Enum::POINTER_DOWN,
        2 => android_auto::Wifi::touch_action::Enum::POINTER_UP,
        _ => android_auto::Wifi::touch_action::Enum::DRAG,
    };
    te.set_touch_action(action);
    i_event.touch_event = android_auto::protobuf::MessageField::some(te);

    android_auto::AndroidAutoMessage::Input(i_event).sendable()
}
