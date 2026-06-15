//! Dedicated H.264 video decoder thread.
//!
//! The heavy decode and YUV→RGB conversion run off the Slint event loop so the
//! UI never stalls. Finished RGB frames are posted back to the UI thread via
//! [`slint::invoke_from_event_loop`].

use crate::messages::VideoCommand;
use crate::AppWindow;

/// Spawn the video decoder thread.
///
/// It owns the openh264 decoder, receives [`VideoCommand`]s over `video_rx`,
/// decodes NAL units, converts to RGB and assigns the frame to the window's
/// `video-frame` property on the UI thread.
pub(crate) fn spawn_decoder(
    video_rx: std::sync::mpsc::Receiver<VideoCommand>,
    window_weak: slint::Weak<AppWindow>,
) {
    std::thread::spawn(move || {
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        log::debug!("Video decoder thread started");
        while let Ok(cmd) = video_rx.recv() {
            // Coalesce everything already queued so we never fall behind: when
            // decode is slower than the incoming frame rate the backlog (and
            // thus the visible delay) would otherwise grow without bound. We
            // must still feed every NAL to the decoder to keep its reference
            // frames valid, but only the newest decoded frame is converted to
            // RGB and presented; intermediate frames are dropped.
            let mut batch = vec![cmd];
            while let Ok(next) = video_rx.try_recv() {
                batch.push(next);
            }
            let last_cmd = batch.len() - 1;

            for (ci, cmd) in batch.into_iter().enumerate() {
                let present_cmd = ci == last_cmd;
                match cmd {
                    VideoCommand::Flush => {
                        let _ = decoder.flush_remaining();
                    }
                    VideoCommand::Frame(data) => {
                        for nal in openh264::nal_units(&data) {
                            match decoder.decode(nal) {
                                Ok(Some(yuv)) => {
                                    // Only the most recent command's output is
                                    // shown; earlier frames are decoded purely
                                    // to advance decoder state, then discarded.
                                    if !present_cmd {
                                        continue;
                                    }
                                    use openh264::formats::YUVSource;
                                    let (w, h) = yuv.dimensions_uv();
                                    let (w, h) = (w * 2, h * 2);
                                    let mut rgb = vec![0u8; yuv.rgb8_len()];
                                    yuv.write_rgb8(&mut rgb);
                                    // `slint::Image` is not `Send`, so hand the
                                    // raw RGB bytes to the UI thread and wrap
                                    // them there (a cheap copy).
                                    let weak = window_weak.clone();
                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(win) = weak.upgrade() {
                                            let mut buf =
                                                slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(
                                                    w as u32,
                                                    h as u32,
                                                );
                                            buf.make_mut_bytes().copy_from_slice(&rgb);
                                            win.set_video_frame(slint::Image::from_rgb8(buf));
                                            // First real frame is now mounted —
                                            // let the UI play the start
                                            // transition over actual video.
                                            win.set_aa_video_ready(true);
                                        }
                                    });
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    log::debug!("Video decode error: {e:?}");
                                }
                            }
                        }
                    }
                }
            }
        }
    });
}
