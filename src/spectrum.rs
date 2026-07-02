//! Audio capture from the PulseAudio/PipeWire monitor source.
//!
//! One background thread fills a ring buffer; `SpectrumProcessor` on the render
//! thread drains it each frame and runs all spectrum analysis there.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Maximum configurable band count (fixes GL VBO pre-allocation in renderers).
pub const BANDS: usize = 64;

/// Ring-buffer consumer — passed to `SpectrumProcessor` on the render thread.
pub type AudioConsumer = ringbuf::HeapCons<f32>;

/// Owns the capture background thread; stops it when dropped.
pub struct SpectrumCapture {
    running: Arc<AtomicBool>,
}

impl Drop for SpectrumCapture {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

/// Spawn the PulseAudio monitor capture thread.
///
/// Returns a [`SpectrumCapture`] guard (keep alive for the visualizer lifetime)
/// and the [`AudioConsumer`] to hand to [`crate::visualizer::SpectrumProcessor`].
pub fn start_capture(viz: &crate::config::VizConfig) -> (SpectrumCapture, AudioConsumer) {
    let running = Arc::new(AtomicBool::new(true));
    // ~2 s of stereo headroom at 48 kHz.
    let rb = ringbuf::HeapRb::<f32>::new(48_000 * 2 * 2);
    let (mut producer, consumer) = ringbuf::traits::Split::split(rb);

    let hop = viz.hop;
    let running_cap = running.clone();
    std::thread::Builder::new()
        .name("spectrum-capture".into())
        .spawn(move || {
            use libpulse_binding as pulse;
            use libpulse_simple_binding as psimple;

            let spec = pulse::sample::Spec {
                format: pulse::sample::Format::FLOAT32NE,
                channels: 2,
                rate: 48_000,
            };
            assert!(spec.is_valid());

            // Request small capture fragments so the server delivers audio as
            // soon as `hop` frames are available rather than buffering a large
            // block. `fragsize` is the key attribute for capture streams:
            // it sets the maximum fragment size the server should accumulate
            // before waking us up. Smaller = lower latency, more CPU wakeups.
            let chunk_bytes = hop * 2 * 4;
            let buf_attr = pulse::def::BufferAttr {
                maxlength: u32::MAX,
                tlength:   u32::MAX, // playback-only
                prebuf:    u32::MAX, // playback-only
                minreq:    u32::MAX, // playback-only
                fragsize:  chunk_bytes as u32,
            };

            let s = match psimple::Simple::new(
                None,
                "eva-navigation-unit",
                pulse::stream::Direction::Record,
                Some("@DEFAULT_MONITOR@"),
                "spectrum-analyzer",
                &spec,
                None,
                Some(&buf_attr),
            ) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!(
                        "spectrum: PulseAudio monitor unavailable ({e}); visualizer silent"
                    );
                    return;
                }
            };

            log::info!("spectrum: PulseAudio capture started");

            // Read `hop` stereo frames per call — latency matches update rate.
            let mut buf = vec![0u8; chunk_bytes];

            while running_cap.load(Ordering::Relaxed) {
                if s.read(&mut buf).is_err() {
                    log::warn!("spectrum: read error; stopping capture");
                    break;
                }
                let samples: Vec<f32> = buf
                    .chunks_exact(4)
                    .map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();
                use ringbuf::traits::Producer;
                let _ = producer.push_slice(&samples);
            }
            log::info!("spectrum: capture thread exiting");
        })
        .expect("spawn spectrum-capture");

    (SpectrumCapture { running }, consumer)
}
