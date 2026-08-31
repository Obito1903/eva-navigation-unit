//! Bluetooth media: remembers the last device that connected, reconnects to it
//! on startup and on resume from suspend, and starts playback over AVRCP once
//! it is back.
//!
//! This talks to BlueZ directly over D-Bus rather than going through the
//! `bluetooth-rust` adapter used by [`crate::container`]: that crate exposes
//! profile registration, pairing and raw sockets, but has no device-level
//! `Connect()` and no media control at all.
//!
//! Note that starting playback only tells the phone to stream. Whether that
//! audio reaches the speakers is up to the system's PipeWire/PulseAudio policy
//! routing the `bluez_input.*` source to the default sink.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use zbus::{Connection, proxy};
use zvariant::{OwnedObjectPath, Value};

/// Delays *between* connect attempts. A phone is routinely out of range, asleep
/// or still bringing its radio up for the first few seconds after the head unit
/// boots or resumes, so the first failure means very little.
const RETRY_DELAYS: [Duration; 4] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
];

/// How long to wait for AVRCP to expose a player after the device connects.
const PLAYER_WAIT: Duration = Duration::from_secs(15);
const PLAYER_POLL: Duration = Duration::from_millis(500);

#[proxy(interface = "org.bluez.Device1", default_service = "org.bluez")]
trait Device1 {
    fn connect(&self) -> zbus::Result<()>;

    #[zbus(property)]
    fn address(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn alias(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn connected(&self) -> zbus::Result<bool>;
}

#[proxy(interface = "org.bluez.MediaPlayer1", default_service = "org.bluez")]
trait MediaPlayer1 {
    fn play(&self) -> zbus::Result<()>;

    /// One of `playing`, `stopped`, `paused`, `forward-seek`, `reverse-seek`,
    /// `error`.
    #[zbus(property)]
    fn status(&self) -> zbus::Result<String>;
}

/// Sent from the UI/power monitor to the worker.
pub(crate) enum Command {
    /// Reconnect to the remembered device and resume playback. Applies the
    /// configured settling delay first, since the only sender is a resume.
    Reconnect,
}

/// Sent from the worker to the UI thread.
pub(crate) enum Event {
    /// A device connected; persist its address as the one to reconnect to.
    /// `Config` is not `Send`, so the UI thread has to do the actual saving.
    LastDeviceChanged(String),
}

/// Owns the background thread + tokio runtime talking to BlueZ, and the
/// channels bridging it to the UI thread.
pub(crate) struct BtMediaContainer {
    thread: Option<std::thread::JoinHandle<()>>,
    pub(crate) recv: tokio::sync::mpsc::Receiver<Event>,
    pub(crate) send: tokio::sync::mpsc::Sender<Command>,
    kill: Option<tokio::sync::oneshot::Sender<()>>,
}

impl BtMediaContainer {
    /// `last_device` is the address remembered from a previous run, if any.
    /// `resume_delay` lets the Bluetooth stack settle before a post-resume
    /// reconnect; it is not applied to the startup attempt.
    pub(crate) fn new(last_device: Option<String>, resume_delay: Duration) -> Self {
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<Command>(8);
        let (evt_tx, evt_rx) = tokio::sync::mpsc::channel::<Event>(8);
        let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build tokio runtime");

        let thread = std::thread::spawn(move || {
            rt.block_on(run(kill_rx, cmd_rx, evt_tx, last_device, resume_delay))
        });

        Self {
            thread: Some(thread),
            recv: evt_rx,
            send: cmd_tx,
            kill: Some(kill_tx),
        }
    }
}

impl Drop for BtMediaContainer {
    fn drop(&mut self) {
        let _ = self.kill.take().map(|s| s.send(()));
        // Join off the current thread so a UI-thread drop never blocks the
        // event loop — same rationale as `AndroidAutoContainer::drop`.
        if let Some(thread) = self.thread.take() {
            std::thread::spawn(move || {
                if let Err(e) = thread.join() {
                    log::warn!("Bluetooth media thread panicked on shutdown: {e:?}");
                }
            });
        }
    }
}

async fn run(
    mut kill_rx: tokio::sync::oneshot::Receiver<()>,
    mut cmd_rx: tokio::sync::mpsc::Receiver<Command>,
    evt_tx: tokio::sync::mpsc::Sender<Event>,
    last_device: Option<String>,
    resume_delay: Duration,
) {
    let conn = match Connection::system().await {
        Ok(c) => c,
        Err(e) => {
            log::warn!("Bluetooth media disabled — no system D-Bus connection: {e}");
            return;
        }
    };

    let mut connect_signals = match connected_signals(&conn).await {
        Ok(s) => Some(s),
        Err(e) => {
            log::warn!("Not tracking Bluetooth connections — cannot watch BlueZ: {e}");
            None
        }
    };

    let mut last_device = last_device;
    // Reconnects run detached so a phone that is out of range cannot stall the
    // connection tracking for the length of the whole backoff.
    let busy = Arc::new(AtomicBool::new(false));

    match &last_device {
        Some(address) => spawn_reconnect(&conn, address.clone(), &busy, Duration::ZERO),
        None => log::debug!("No remembered Bluetooth device to reconnect to"),
    }

    loop {
        tokio::select! {
            _ = &mut kill_rx => {
                log::debug!("Bluetooth media worker killed");
                break;
            }

            Some(Command::Reconnect) = cmd_rx.recv() => {
                match &last_device {
                    Some(address) => {
                        spawn_reconnect(&conn, address.clone(), &busy, resume_delay)
                    }
                    None => log::debug!("No remembered Bluetooth device to reconnect to"),
                }
            }

            Some(Ok(msg)) = next_or_pending(&mut connect_signals) => {
                let Some(path) = newly_connected_device(&msg) else { continue };
                let Some(address) = device_address(&conn, &path).await else { continue };
                if last_device.as_deref() == Some(address.as_str()) {
                    continue;
                }
                log::info!("Bluetooth device {address} connected — remembering it");
                last_device = Some(address.clone());
                let _ = evt_tx.send(Event::LastDeviceChanged(address)).await;
            }
        }
    }
}

/// Run a reconnect in the background, unless one is already in flight. This is
/// also what coalesces an overlapping startup and resume trigger.
fn spawn_reconnect(conn: &Connection, address: String, busy: &Arc<AtomicBool>, delay: Duration) {
    if busy.swap(true, Ordering::SeqCst) {
        log::debug!("Bluetooth reconnect already in progress — ignoring trigger");
        return;
    }
    let conn = conn.clone();
    let busy = busy.clone();
    tokio::spawn(async move {
        if !delay.is_zero() {
            log::debug!("Waiting {}ms before reconnecting Bluetooth", delay.as_millis());
            tokio::time::sleep(delay).await;
        }
        reconnect_and_play(&conn, &address).await;
        busy.store(false, Ordering::SeqCst);
    });
}

async fn reconnect_and_play(conn: &Connection, address: &str) {
    let Some(path) = find_device_path(conn, address).await else {
        log::warn!("Bluetooth device {address} is not known to BlueZ — cannot reconnect");
        return;
    };

    let device = match Device1Proxy::builder(conn).path(&path) {
        Ok(builder) => match builder.build().await {
            Ok(d) => d,
            Err(e) => {
                log::warn!("Cannot talk to Bluetooth device {address}: {e}");
                return;
            }
        },
        Err(e) => {
            log::warn!("Bad object path for Bluetooth device {address}: {e}");
            return;
        }
    };

    if device.connected().await.unwrap_or(false) {
        log::debug!("Bluetooth device {address} is already connected");
    } else if !connect_with_backoff(&device, address).await {
        return;
    }

    start_playback(conn, &path, address).await;
}

async fn connect_with_backoff(device: &Device1Proxy<'_>, address: &str) -> bool {
    let mut delays = RETRY_DELAYS.iter();
    loop {
        match device.connect().await {
            Ok(()) => {
                log::info!("Reconnected to Bluetooth device {address}");
                return true;
            }
            Err(e) => match delays.next() {
                Some(delay) => {
                    log::debug!("Bluetooth reconnect to {address} failed ({e}); retrying");
                    tokio::time::sleep(*delay).await;
                }
                None => {
                    log::info!("Gave up reconnecting to Bluetooth device {address}: {e}");
                    return false;
                }
            },
        }
    }
}

async fn start_playback(conn: &Connection, device: &OwnedObjectPath, address: &str) {
    // AVRCP exposes the player well after `Connect()` returns, so it has to be
    // waited for rather than looked up once.
    let deadline = Instant::now() + PLAYER_WAIT;
    let player = loop {
        if let Some(p) = find_player(conn, device).await {
            break Some(p);
        }
        if Instant::now() >= deadline {
            break None;
        }
        tokio::time::sleep(PLAYER_POLL).await;
    };

    let Some(player) = player else {
        log::info!("No AVRCP player appeared for {address} — not starting playback");
        return;
    };

    if matches!(player.status().await.as_deref(), Ok("playing")) {
        log::debug!("Bluetooth device {address} is already playing");
        return;
    }

    match player.play().await {
        Ok(()) => log::info!("Started playback on Bluetooth device {address}"),
        Err(e) => log::warn!("Could not start playback on {address}: {e}"),
    }
}

/// Find a device's object path by address. The adapter index is not knowable up
/// front, so the path cannot simply be built from the address.
async fn find_device_path(conn: &Connection, address: &str) -> Option<OwnedObjectPath> {
    let objects = managed_objects(conn).await?;
    for (path, interfaces) in objects {
        for (interface, props) in interfaces {
            if interface.as_str() != "org.bluez.Device1" {
                continue;
            }
            let Some(value) = props.get("Address") else {
                continue;
            };
            let Ok(found) = String::try_from(value.clone()) else {
                continue;
            };
            if found.eq_ignore_ascii_case(address) {
                return Some(path);
            }
        }
    }
    None
}

/// Find the `MediaPlayer1` object BlueZ exposes underneath a connected device.
async fn find_player(
    conn: &Connection,
    device: &OwnedObjectPath,
) -> Option<MediaPlayer1Proxy<'static>> {
    let objects = managed_objects(conn).await?;
    let prefix = format!("{}/", device.as_str());
    for (path, interfaces) in objects {
        if !path.as_str().starts_with(&prefix) {
            continue;
        }
        if !interfaces
            .keys()
            .any(|i| i.as_str() == "org.bluez.MediaPlayer1")
        {
            continue;
        }
        return MediaPlayer1Proxy::builder(conn)
            .path(path)
            .ok()?
            .build()
            .await
            .ok();
    }
    None
}

type ManagedObjects = HashMap<OwnedObjectPath, HashMap<zbus::names::OwnedInterfaceName, HashMap<String, zvariant::OwnedValue>>>;

async fn managed_objects(conn: &Connection) -> Option<ManagedObjects> {
    let proxy = zbus::fdo::ObjectManagerProxy::builder(conn)
        .destination("org.bluez")
        .ok()?
        .path("/")
        .ok()?
        .build()
        .await
        .ok()?;
    match proxy.get_managed_objects().await {
        Ok(o) => Some(o),
        Err(e) => {
            log::warn!("Could not enumerate BlueZ objects: {e}");
            None
        }
    }
}

/// A `PropertiesChanged` stream covering every BlueZ object, so devices that
/// only appear later are still tracked.
async fn connected_signals(conn: &Connection) -> zbus::Result<zbus::MessageStream> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.bluez")?
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .build();
    zbus::MessageStream::for_match_rule(rule, conn, None).await
}

/// The device path from a `PropertiesChanged` reporting `Connected` = true, if
/// that is what this message is.
fn newly_connected_device(msg: &zbus::Message) -> Option<OwnedObjectPath> {
    let body = msg.body();
    let (interface, changed, _invalidated) =
        body.deserialize::<(String, HashMap<String, Value>, Vec<String>)>()
            .ok()?;
    if interface != "org.bluez.Device1" {
        return None;
    }
    if !matches!(changed.get("Connected"), Some(Value::Bool(true))) {
        return None;
    }
    msg.header().path().map(|p| p.to_owned().into())
}

async fn device_address(conn: &Connection, path: &OwnedObjectPath) -> Option<String> {
    Device1Proxy::builder(conn)
        .path(path)
        .ok()?
        .build()
        .await
        .ok()?
        .address()
        .await
        .ok()
}

/// Yield the next item, or wait forever when the source was never set up, so an
/// unavailable subsystem simply never fires its `select!` branch.
async fn next_or_pending<S>(stream: &mut Option<S>) -> Option<S::Item>
where
    S: futures_util::Stream + Unpin,
{
    match stream {
        Some(s) => s.next().await,
        None => std::future::pending().await,
    }
}
