//! Messages bridging the async android-auto worker and the Slint UI thread.

/// Sent from the async worker to the UI thread.
pub(crate) enum MessageFromAsync {
    VideoData {
        data: Vec<u8>,
        _timestamp: Option<u64>,
    },
    Connected,
    Disconnected,
    ExitContainer,
}

/// Sent from the UI thread to the async worker.
pub(crate) enum MessageToAsync {
    AndroidAutoMessage(android_auto::SendableAndroidAutoMessage),
}

/// Commands to the dedicated video decoder thread.
pub(crate) enum VideoCommand {
    /// Raw H.264 NAL bytes to decode and display.
    Frame(Vec<u8>),
    /// Flush the decoder (e.g. on disconnect).
    Flush,
}
