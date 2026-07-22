//! OBD2 telemetry over a Bluetooth ELM327 adapter.
//!
//! Plumbing only for now: connects to a paired ELM327 over Bluetooth RFCOMM
//! (via [`transport::BluetoothRfcommTransport`], built on the `bluetooth-rust`
//! dependency already used for Android Auto pairing rather than obd2-core's
//! own serial/BLE transports), polls the PIDs configured in `[[obd2.pids]]`,
//! and evaluates each one's user-supplied formula with `meval`. There is no
//! UI wiring yet — see [`crate::messages::Obd2Update`] for the message type a
//! future UI layer will consume.
//!
//! Runs on its own dedicated thread + tokio runtime (mirroring
//! [`crate::container::AndroidAutoContainer`]), deliberately decoupled from
//! the Android Auto worker's lifecycle since OBD2 telemetry has no reason to
//! depend on an active AA session — the ELM327 is a logically separate
//! Bluetooth peer from the phone.

mod transport;
mod worker;

use crate::config::Obd2Config;
use crate::messages::Obd2Update;

/// Owns the OBD2 worker thread. The channel receiving its readings is
/// returned separately from [`Obd2Container::new`] (rather than stored as a
/// field) since this type implements [`Drop`], which would otherwise forbid
/// moving the receiver back out later.
pub(crate) struct Obd2Container {
    thread: Option<std::thread::JoinHandle<()>>,
    kill: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Obd2Container {
    /// Spawn the OBD2 worker. Returns immediately; the worker connects and
    /// starts polling in the background. If `cfg.enabled` is `false` the
    /// worker exits right away and the returned channel just stays empty.
    pub(crate) fn new(cfg: Obd2Config) -> (Self, tokio::sync::mpsc::Receiver<Obd2Update>) {
        let (tx, rx) = tokio::sync::mpsc::channel(50);
        let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build OBD2 tokio runtime");

        let thread = std::thread::spawn(move || {
            rt.block_on(async {
                tokio::select! {
                    _ = worker::run(cfg, tx) => {}
                    _ = kill_rx => {
                        log::debug!("obd2: worker killed");
                    }
                }
            });
        });

        (
            Self {
                thread: Some(thread),
                kill: Some(kill_tx),
            },
            rx,
        )
    }
}

impl Drop for Obd2Container {
    fn drop(&mut self) {
        // Signal the worker to stop.
        let _ = self.kill.take().map(|s| s.send(()));

        // Reclaim the thread in the background so dropping the container
        // never blocks the caller on the OBD2 runtime's teardown (mirrors
        // `AndroidAutoContainer`'s shutdown in `crate::container`).
        if let Some(thread) = self.thread.take() {
            std::thread::spawn(move || {
                if let Err(e) = thread.join() {
                    log::warn!("obd2 worker thread panicked on shutdown: {e:?}");
                }
            });
        }
    }
}
