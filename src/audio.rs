//! cpal audio stream construction for android-auto output channels.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait};

/// Ring-buffer producer feeding a cpal output stream.
pub(crate) type AudioProducer = ringbuf::HeapProd<i16>;

/// Build a cpal error callback that rate-limits logging. ALSA can raise
/// `POLLERR` on every poll once an output device misbehaves (e.g. an HDMI sink
/// that went away), which would otherwise spam the log many times per second.
/// We log the first occurrence at error level and then at most once every
/// 5 seconds, so the condition stays visible without flooding the log.
fn throttled_error_fn(stream: &'static str) -> impl FnMut(cpal::StreamError) {
    let base = Instant::now();
    // Last time we logged, in millis since `base`. `u64::MAX` means "never".
    let last_logged = Arc::new(AtomicU64::new(u64::MAX));
    move |err| {
        const THROTTLE_MS: u64 = 5_000;
        let now = base.elapsed().as_millis() as u64;
        let prev = last_logged.load(Ordering::Relaxed);
        if prev == u64::MAX || now.saturating_sub(prev) >= THROTTLE_MS {
            last_logged.store(now, Ordering::Relaxed);
            log::error!("{stream} audio output error: {err:?}");
        } else {
            log::debug!("{stream} audio output error (throttled): {err:?}");
        }
    }
}

/// Build a cpal output stream fed by a ring buffer at the given rate/channels.
pub(crate) fn build_output_stream_for(
    device: &cpal::Device,
    rate: u32,
    channels: u16,
    buf_size: usize,
    label: &'static str,
) -> Option<(AudioProducer, cpal::Stream)> {
    let configs = device.supported_output_configs().ok()?;
    for c in configs {
        if c.min_sample_rate() <= rate
            && c.max_sample_rate() >= rate
            && c.channels() == channels
            && c.sample_format() == cpal::SampleFormat::I16
        {
            let sc = c.try_with_sample_rate(rate)?;
            let rb = ringbuf::HeapRb::new(buf_size);
            let (producer, mut consumer) = ringbuf::traits::Split::split(rb);
            let stream = device
                .build_output_stream(
                    &sc.config(),
                    move |data: &mut [i16], _| {
                        // Pull as much real audio as the ring buffer holds, then
                        // fill any remainder with silence. Always handing ALSA a
                        // full period prevents playback underruns (xruns) during
                        // gaps in the android-auto audio stream — those xruns are
                        // what surface as a continuous `POLLERR` error spam.
                        let mut idx = 0;
                        while idx < data.len() {
                            let n = ringbuf::traits::Consumer::pop_slice(
                                &mut consumer,
                                &mut data[idx..],
                            );
                            if n == 0 {
                                break;
                            }
                            idx += n;
                        }
                        // Silence-fill whatever the ring buffer could not supply.
                        data[idx..].fill(0);
                    },
                    throttled_error_fn(label),
                    None,
                )
                .ok()?;
            return Some((producer, stream));
        }
    }
    None
}

/// Return type of [`build_audio_streams`]: default input device plus the
/// media/system/speech output streams used by android-auto.
pub(crate) type AudioStreams = (
    Option<cpal::Device>,
    Option<(AudioProducer, cpal::Stream)>,
    Option<(AudioProducer, cpal::Stream)>,
    Option<(AudioProducer, cpal::Stream)>,
);

/// Build the default input device handle plus the media/system/speech output
/// streams used by android-auto.
pub(crate) fn build_audio_streams() -> AudioStreams {
    let host = cpal::default_host();
    let ai = host.default_input_device();
    let ao = host.default_output_device();

    if ai.is_none() {
        log::debug!("No default audio input device found; microphone unavailable");
    }

    if let Some(ao) = &ao {
        let media = build_output_stream_for(ao, 48000, 2, 48000, "media");
        let sys = build_output_stream_for(ao, 16000, 1, 16000, "system");
        let speech = build_output_stream_for(ao, 16000, 1, 16000, "speech");
        (ai, media, sys, speech)
    } else {
        log::warn!("No default audio output device found; Android Auto audio disabled");
        (ai, None, None, None)
    }
}
