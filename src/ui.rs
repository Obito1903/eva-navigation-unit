//! Wires the Slint window to the android-auto worker: forwards touch events,
//! spawns the video decoder, and pumps worker → UI messages on a timer.

use crate::container::{AndroidAutoContainer, VideoSettings};
use crate::messages::{MessageFromAsync, MessageToAsync, VideoCommand};
use crate::video;
use crate::AppWindow;
use crate::Theme;
use slint::ComponentHandle;
use slint::Global;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;

/// How often the UI thread drains messages coming from the worker.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);

/// Copy the window's current size into the shared video settings so the next
/// (re)connection negotiates a picture matching the live screen aspect ratio.
/// Falls back to 16:9 (1280×720) before the window has been realised.
fn refresh_screen_size(win: &AppWindow, video: &VideoSettings) {
    let size = win.window().size();
    let (w, h) = if size.width == 0 || size.height == 0 {
        (1280, 720)
    } else {
        (size.width, size.height)
    };
    video.screen_w.store(w, Ordering::Relaxed);
    video.screen_h.store(h, Ordering::Relaxed);
}

/// Connect the window's callbacks and the worker container, and start the
/// polling timer. The returned timer is leaked into the event loop for the
/// lifetime of the program.
pub(crate) fn wire(
    window: &AppWindow,
    setup: android_auto::AndroidAutoSetup,
    cfg: crate::config::Config,
) {
    let window_weak = window.as_weak();

    // Shared, mutable configuration. Settings callbacks update it and persist
    // the change back to the config file on the UI thread.
    let cfg = Rc::new(RefCell::new(cfg));

    // Shared Android Auto video settings, read by the worker on every
    // (re)connection. Seeded from config + a 16:9 fallback screen size.
    let video = Arc::new(VideoSettings {
        resolution: AtomicI32::new(cfg.borrow().resolution),
        fps: AtomicI32::new(cfg.borrow().fps),
        dpi: AtomicI32::new(cfg.borrow().dpi),
        screen_w: AtomicU32::new(1280),
        screen_h: AtomicU32::new(720),
    });
    refresh_screen_size(window, &video);

    // ── Wireless toggle: Settings UI → worker ─────────────────────────────
    let wireless = Arc::new(AtomicBool::new(cfg.borrow().wireless));
    {
        let wireless = wireless.clone();
        let cfg = cfg.clone();
        window.on_aa_wireless_changed(move |enabled| {
            log::info!("Wireless Android Auto {}", if enabled { "enabled" } else { "disabled" });
            wireless.store(enabled, Ordering::Relaxed);
            let mut cfg = cfg.borrow_mut();
            cfg.wireless = enabled;
            cfg.save();
        });
    }

    // ── USB toggle: Settings UI → worker ──────────────────────────────────
    let usb = Arc::new(AtomicBool::new(cfg.borrow().usb));
    {
        let usb = usb.clone();
        let cfg = cfg.clone();
        window.on_aa_usb_changed(move |enabled| {
            log::info!("USB Android Auto {}", if enabled { "enabled" } else { "disabled" });
            usb.store(enabled, Ordering::Relaxed);
            let mut cfg = cfg.borrow_mut();
            cfg.usb = enabled;
            cfg.save();
        });
    }

    // ── Hotspot backend: Settings UI → worker ─────────────────────────────
    // 0 = NetworkManager | 1 = hostapd. Applied on the next (re)connection.
    let hotspot_backend = Arc::new(AtomicI32::new(cfg.borrow().hotspot_backend));
    {
        let hotspot_backend = hotspot_backend.clone();
        let cfg = cfg.clone();
        window.on_hotspot_backend_changed(move |backend| {
            log::info!(
                "Hotspot backend set to {} (applies on next connection)",
                if backend == 1 { "hostapd" } else { "NetworkManager" }
            );
            hotspot_backend.store(backend, Ordering::Relaxed);
            let mut cfg = cfg.borrow_mut();
            cfg.hotspot_backend = backend;
            cfg.save();
        });
    }

    // ── Hotspot channel: Settings UI → worker ─────────────────────────────
    let hotspot_channel = Arc::new(AtomicI32::new(cfg.borrow().hotspot_channel));
    {
        let hotspot_channel = hotspot_channel.clone();
        let cfg = cfg.clone();
        window.on_hotspot_channel_changed(move |channel| {
            log::info!("Hotspot (hostapd) channel set to {channel} (applies on next connection)");
            hotspot_channel.store(channel, Ordering::Relaxed);
            let mut cfg = cfg.borrow_mut();
            cfg.hotspot_channel = channel;
            cfg.save();
        });
    }

    // ── DPI: Settings UI → config ─────────────────────────────────────────
    {
        let video = video.clone();
        let cfg = cfg.clone();
        window.on_aa_dpi_changed(move |dpi| {
            log::info!("Android Auto DPI set to {dpi} (applies on next connection)");
            video.dpi.store(dpi, Ordering::Relaxed);
            let mut cfg = cfg.borrow_mut();
            cfg.dpi = dpi;
            cfg.save();
        });
    }

    // ── Quick controls: system volume ─────────────────────────────────────
    // Drives the real system mixer; seed the slider from the current level.
    {
        let volume = crate::controls::Volume::discover();
        if let Some(level) = volume.as_ref().and_then(|v| v.get_fraction()) {
            window.set_volume_level(level);
        }
        window.on_volume_changed(move |level| {
            if let Some(v) = volume.as_ref() {
                v.set_fraction(level);
            }
        });
    }

    // ── Quick controls: physical screen brightness ────────────────────────
    // Drives the kernel backlight sysfs; seed the slider from the current level.
    {
        let backlight = crate::controls::Backlight::discover();
        if let Some(level) = backlight.as_ref().and_then(|b| b.get_fraction()) {
            window.set_brightness_level(level);
        }
        window.on_brightness_changed(move |level| {
            if let Some(b) = backlight.as_ref() {
                b.set_fraction(level);
            }
        });
    }

    // ── Resolution: Settings UI → config + worker ─────────────────────────
    {
        let video = video.clone();
        let cfg = cfg.clone();
        window.on_aa_resolution_changed(move |resolution| {
            log::info!("Android Auto resolution set to {resolution}p (applies on next connection)");
            video.resolution.store(resolution, Ordering::Relaxed);
            let mut cfg = cfg.borrow_mut();
            cfg.resolution = resolution;
            cfg.save();
        });
    }

    // ── Frame rate: Settings UI → config + worker ─────────────────────────
    {
        let video = video.clone();
        let cfg = cfg.clone();
        window.on_aa_fps_changed(move |fps| {
            log::info!("Android Auto frame rate set to {fps}fps (applies on next connection)");
            video.fps.store(fps, Ordering::Relaxed);
            let mut cfg = cfg.borrow_mut();
            cfg.fps = fps;
            cfg.save();
        });
    }

    // ── View transition: Settings UI → config ─────────────────────────────
    {
        let cfg = cfg.clone();
        window.on_transition_changed(move |mode| {
            log::info!("View transition mode set to {mode}");
            let mut cfg = cfg.borrow_mut();
            cfg.transition_mode = mode;
            cfg.save();
        });
    }

    // ── Android Auto video transition: Settings UI → config ───────────────
    {
        let cfg = cfg.clone();
        window.on_aa_video_transition_changed(move |mode| {
            log::info!("Android Auto video transition mode set to {mode}");
            let mut cfg = cfg.borrow_mut();
            cfg.aa_video_transition_mode = mode;
            cfg.save();
        });
    }

    // ── View transition speed: Settings UI → config ───────────────────────
    {
        let cfg = cfg.clone();
        window.on_transition_speed_changed(move |speed| {
            log::info!("View transition speed set to {speed:.2}×");
            let mut cfg = cfg.borrow_mut();
            cfg.transition_speed = speed;
            cfg.save();
        });
    }

    // ── Android Auto video transition speed: Settings UI → config ─────────
    {
        let cfg = cfg.clone();
        window.on_aa_video_transition_speed_changed(move |speed| {
            log::info!("Android Auto video transition speed set to {speed:.2}×");
            let mut cfg = cfg.borrow_mut();
            cfg.aa_video_transition_speed = speed;
            cfg.save();
        });
    }

    // ── Color theme: Settings UI → config ─────────────────────────────────
    {
        let cfg = cfg.clone();
        let window_weak = window_weak.clone();
        window.on_theme_changed(move |theme| {
            log::info!("Color theme set to {theme}");
            if let Some(win) = window_weak.upgrade() {
                Theme::get(&win).set_theme_id(theme);
            }
            let mut cfg = cfg.borrow_mut();
            cfg.theme = theme;
            cfg.save();
        });
    }

    // ── GL underlay model: Settings UI → config ───────────────────────────
    {
        let cfg = cfg.clone();
        window.on_gfx_model_changed(move |model| {
            log::info!("Background model set to {model}");
            let mut cfg = cfg.borrow_mut();
            cfg.gfx_model = model;
            cfg.save();
        });
    }

    // ── Fullscreen: Settings UI → config ──────────────────────────────────
    {
        let cfg = cfg.clone();
        window.on_fullscreen_changed(move |enabled| {
            log::info!("Fullscreen {}", if enabled { "enabled" } else { "disabled" });
            let mut cfg = cfg.borrow_mut();
            cfg.fullscreen = enabled;
            cfg.save();
        });
    }

    // ── Start android-auto in background ──────────────────────────────────
    let mut container = AndroidAutoContainer::new(
        setup,
        wireless.clone(),
        usb.clone(),
        video.clone(),
        hotspot_backend.clone(),
        hotspot_channel.clone(),
    );

    // The worker is torn down and recreated on every disconnect, which makes a
    // fresh message channel each time. Touch input must always target the
    // *current* worker, so keep the sender in a shared cell and refresh it on
    // restart — otherwise touches would silently go to the previous (dead)
    // channel after the first reconnect.
    let send_touch = Rc::new(RefCell::new(container.send.clone()));

    // ── Touch events: Slint UI → android-auto ─────────────────────────────
    {
        let send_touch = send_touch.clone();
        window.on_touch_event(move |x, y, kind| {
            let msg = build_touch_message(x, y, kind);
            // Never block the event loop: this runs on the UI thread, and a
            // blocking send would freeze the whole UI if the worker isn't
            // draining its channel (e.g. while reconnecting). Dropping the odd
            // touch when the buffer is full is preferable to a frozen UI.
            let _ = send_touch
                .borrow()
                .try_send(MessageToAsync::AndroidAutoMessage(msg));
        });
    }

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
                    // Pick up the latest screen size so the renegotiated stream
                    // matches the current window aspect ratio.
                    refresh_screen_size(&win, &video);
                    container = AndroidAutoContainer::new(
                        setup,
                        wireless.clone(),
                        usb.clone(),
                        video.clone(),
                        hotspot_backend.clone(),
                        hotspot_channel.clone(),
                    );
                    // Point touch input at the new worker's channel.
                    *send_touch.borrow_mut() = container.send.clone();
                }
                MessageFromAsync::Connected => {
                    log::info!("Android Auto connected");
                    // Clear any stale frame and mark the stream "not ready" so
                    // the start transition only plays once the first real frame
                    // arrives (see `set_aa_video_ready` in the decoder thread).
                    win.set_video_frame(slint::Image::default());
                    win.set_aa_video_ready(false);
                    win.set_aa_connected(true);
                }
                MessageFromAsync::Disconnected => {
                    log::info!("Android Auto disconnected");
                    win.set_aa_connected(false);
                    win.set_aa_video_ready(false);
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
