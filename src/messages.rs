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
    /// Android Auto video focus changed. `true` means Android Auto requested
    /// the screen (show the Android Auto view); `false` is the "Exit" intent —
    /// the user asked to return to the head unit GUI while the Android Auto
    /// session stays connected.
    FocusChanged(bool),
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

/// Sent from the OBD2 worker ([`crate::obd2`]) to whoever consumes live
/// vehicle telemetry. No UI wiring yet — for now the receiving end is a
/// temporary logger in `main.rs`.
#[cfg(feature = "obd2")]
pub(crate) enum Obd2Update {
    /// The ELM327 connection was established.
    Connected,
    /// The connection was lost; a reconnect attempt is in progress.
    Disconnected,
    /// A configured PID's formula evaluated successfully.
    Reading {
        /// The PID's configured name (e.g. "engine_rpm").
        name: String,
        /// The formula's evaluated result.
        value: f64,
        /// The PID's configured physical unit label (e.g. "rpm").
        unit: String,
    },
}

