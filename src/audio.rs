//! cpal audio stream construction for android-auto output channels.

use cpal::traits::{DeviceTrait, HostTrait};

/// Ring-buffer producer feeding a cpal output stream.
pub(crate) type AudioProducer = ringbuf::HeapProd<i16>;

/// Build a cpal output stream fed by a ring buffer at the given rate/channels.
pub(crate) fn build_output_stream_for(
    device: &cpal::Device,
    rate: u32,
    channels: u16,
    buf_size: usize,
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
                    },
                    |err| log::error!("Audio output error: {err:?}"),
                    None,
                )
                .ok()?;
            return Some((producer, stream));
        }
    }
    None
}

/// Build the default input device handle plus the media/system/speech output
/// streams used by android-auto.
pub(crate) fn build_audio_streams() -> (
    Option<cpal::Device>,
    Option<(AudioProducer, cpal::Stream)>,
    Option<(AudioProducer, cpal::Stream)>,
    Option<(AudioProducer, cpal::Stream)>,
) {
    let host = cpal::default_host();
    let ai = host.default_input_device();
    let ao = host.default_output_device();

    if let Some(ao) = &ao {
        let media = build_output_stream_for(ao, 48000, 2, 48000);
        let sys = build_output_stream_for(ao, 16000, 1, 16000);
        let speech = build_output_stream_for(ao, 16000, 1, 16000);
        (ai, media, sys, speech)
    } else {
        (ai, None, None, None)
    }
}
