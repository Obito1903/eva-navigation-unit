//! Dedicated H.264 video decoder thread.
//!
//! The heavy decode and YUV→RGB conversion run off the Slint event loop so the
//! UI never stalls. Finished RGB frames are posted back to the UI thread via
//! [`slint::invoke_from_event_loop`].

use crate::messages::VideoCommand;
use crate::AppWindow;

/// Crop a decoded RGB8 buffer down to the centered region matching the
/// `target_w`/`target_h` aspect ratio.
///
/// Android Auto only encodes at fixed 16:9 base resolutions
/// (`protocol::build_video_configuration`), so when the actual video
/// viewport isn't 16:9 the phone pads the picture with black
/// letterbox/pillarbox margins baked directly into the decoded buffer rather
/// than changing its pixel dimensions. Displaying that buffer as-is with
/// `image-fit: fill` stretches those margins along with the real picture,
/// visibly distorting it. Cropping to the viewport's own aspect ratio here
/// removes the margins so `fill` only ever scales uniformly.
///
/// The cropped buffer's dimensions match the "active" resolution Android was
/// told about via `TouchConfig` (see `resolution_dimensions` in
/// `protocol.rs`), so touch coordinates mapped against `video-frame` line up
/// with Android's touch coordinate space with no further offset needed.
fn crop_to_aspect(rgb: &[u8], w: usize, h: usize, target_w: i32, target_h: i32) -> (u32, u32, Vec<u8>) {
    if target_w <= 0 || target_h <= 0 || w == 0 || h == 0 {
        return (w as u32, h as u32, rgb.to_vec());
    }
    let target_aspect = target_w as f64 / target_h as f64;
    let candidate_w = (h as f64 * target_aspect).round() as usize;
    let (crop_w, crop_h) = if candidate_w <= w {
        (candidate_w.max(1), h)
    } else {
        let candidate_h = ((w as f64 / target_aspect).round() as usize).max(1);
        (w, candidate_h.min(h))
    };
    if crop_w >= w && crop_h >= h {
        return (w as u32, h as u32, rgb.to_vec());
    }
    let x0 = (w - crop_w) / 2;
    let y0 = (h - crop_h) / 2;
    let stride = w * 3;
    let crop_stride = crop_w * 3;
    let mut out = vec![0u8; crop_w * crop_h * 3];
    for row in 0..crop_h {
        let src_start = (y0 + row) * stride + x0 * 3;
        let dst_start = row * crop_stride;
        out[dst_start..dst_start + crop_stride]
            .copy_from_slice(&rgb[src_start..src_start + crop_stride]);
    }
    (crop_w as u32, crop_h as u32, out)
}

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
                                            let target_w = win.get_aa_viewport_width();
                                            let target_h = win.get_aa_viewport_height();
                                            let (cw, ch, cropped) =
                                                crop_to_aspect(&rgb, w, h, target_w, target_h);
                                            let mut buf =
                                                slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(
                                                    cw, ch,
                                                );
                                            buf.make_mut_bytes().copy_from_slice(&cropped);
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

