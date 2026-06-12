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
        while let Ok(cmd) = video_rx.recv() {
            match cmd {
                VideoCommand::Flush => {
                    let _ = decoder.flush_remaining();
                }
                VideoCommand::Frame(data) => {
                    let mut units = openh264::nal_units(&data).peekable();
                    while let Some(nal) = units.next() {
                        match decoder.decode(nal) {
                            Ok(Some(yuv)) => {
                                use openh264::formats::YUVSource;
                                let (w, h) = yuv.dimensions_uv();
                                let (w, h) = (w * 2, h * 2);
                                let mut rgb = vec![0u8; yuv.rgb8_len()];
                                yuv.write_rgb8(&mut rgb);
                                // `slint::Image` is not `Send`, so hand the raw
                                // RGB bytes to the UI thread and wrap them there
                                // (a cheap copy).
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
                                    }
                                });
                            }
                            Ok(None) => {}
                            Err(e) => {
                                log::error!("Video decode error: {e:?}");
                            }
                        }
                    }
                }
            }
        }
    });
}
