//! System power monitoring over the system D-Bus: suspend/resume from
//! systemd-logind, and power-supply state from UPower.
//!
//! Both concerns share one connection and one worker thread, but their setup is
//! independent — a missing UPower daemon must not disable suspend detection,
//! and vice versa.
//!
//! Suspend detection uses logind's `PrepareForSleep(b)` signal, which fires with
//! `true` immediately before the machine suspends and `false` once it has
//! resumed. A *delay* inhibitor lock is held so that `true` arrives with time to
//! spare rather than as a race against the kernel; delay locks need no polkit
//! rule (unlike block locks), and logind caps how long they hold suspend off via
//! `InhibitDelayMaxSec` (5s by default).
//!
//! This module only observes and logs. Nothing reacts to suspend yet — the
//! subsystems that would need tearing down (the hostapd unit, cpal streams, the
//! H.264 decoder, the USB accessory) are still left to fail and recover through
//! the existing `ExitContainer` restart path.

use std::time::Duration;

use futures_util::StreamExt;
use zbus::{Connection, proxy};

#[proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Login1Manager {
    /// Returns a file descriptor representing the lock; suspend is held off
    /// until it is closed (i.e. until the returned value is dropped).
    fn inhibit(
        &self,
        what: &str,
        who: &str,
        why: &str,
        mode: &str,
    ) -> zbus::Result<zvariant::OwnedFd>;

    /// `true` just before suspending, `false` just after resuming.
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;

    /// Suspend the machine. `interactive` controls whether polkit may prompt;
    /// always `false` here, since a head unit has nobody to answer it.
    fn suspend(&self, interactive: bool) -> zbus::Result<()>;
}

#[proxy(
    interface = "org.freedesktop.UPower",
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower"
)]
trait UPower {
    /// The composite device aggregating every power source. Also used here as a
    /// liveness probe, since a proxy can be built even when nothing owns the
    /// well-known name yet.
    fn get_display_device(&self) -> zbus::Result<zvariant::OwnedObjectPath>;

    /// `true` when running on battery, i.e. no mains/USB power.
    #[zbus(property)]
    fn on_battery(&self) -> zbus::Result<bool>;
}

#[proxy(interface = "org.freedesktop.UPower.Device", default_service = "org.freedesktop.UPower")]
trait UPowerDevice {
    #[zbus(property)]
    fn is_present(&self) -> zbus::Result<bool>;

    /// `UP_DEVICE_STATE`; see [`state_name`].
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn percentage(&self) -> zbus::Result<f64>;
}

/// Live UPower proxies for the composite display device.
struct UPowerState {
    manager: UPowerProxy<'static>,
    device: UPowerDeviceProxy<'static>,
}

/// Last values written to the log. UPower re-emits `PropertiesChanged` far more
/// often than the values meaningfully change, and zbus fires every property
/// stream once more when its cache first fills, so every handler filters
/// against this before logging.
#[derive(Default)]
struct LastLogged {
    on_battery: Option<bool>,
    state: Option<u32>,
    percent: Option<i64>,
}

/// How long to wait for the Android Auto session to tear down before letting
/// the machine suspend anyway. Must stay under logind's `InhibitDelayMaxSec`
/// (5s by default) or logind stops waiting for us regardless.
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(4);

/// Sent to the UI thread, which owns the Android Auto container.
pub(crate) enum AaCommand {
    /// Tear the session down; the ack fires once the worker thread has joined.
    Suspend(tokio::sync::oneshot::Sender<()>),
    /// Bring the session back (after the configured delay).
    Resume,
}

/// When to suspend the machine after mains power goes away.
#[derive(Clone, Copy)]
pub(crate) struct BatterySuspend {
    pub(crate) enabled: bool,
    pub(crate) delay: Duration,
}

/// Owns the background thread + tokio runtime watching logind and UPower.
/// Mirrors the shape of [`crate::jamesdsp::JamesDspContainer`].
pub(crate) struct PowerMonitor {
    thread: Option<std::thread::JoinHandle<()>>,
    kill: Option<tokio::sync::oneshot::Sender<()>>,
}

impl PowerMonitor {
    /// `bt` is nudged on resume so the last Bluetooth device is reconnected;
    /// `aa` drives the Android Auto teardown and restart.
    pub(crate) fn new(
        bt: tokio::sync::mpsc::Sender<crate::btmedia::Command>,
        aa: tokio::sync::mpsc::Sender<AaCommand>,
        battery_suspend: BatterySuspend,
    ) -> Self {
        let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build tokio runtime");

        let thread = std::thread::spawn(move || rt.block_on(run(kill_rx, bt, aa, battery_suspend)));

        Self {
            thread: Some(thread),
            kill: Some(kill_tx),
        }
    }
}

impl Drop for PowerMonitor {
    fn drop(&mut self) {
        let _ = self.kill.take().map(|s| s.send(()));
        // Join off the current thread so a UI-thread drop never blocks the
        // event loop — same rationale as `AndroidAutoContainer::drop`.
        if let Some(thread) = self.thread.take() {
            std::thread::spawn(move || {
                if let Err(e) = thread.join() {
                    log::warn!("Power monitor thread panicked on shutdown: {e:?}");
                }
            });
        }
    }
}

async fn run(
    mut kill_rx: tokio::sync::oneshot::Receiver<()>,
    bt: tokio::sync::mpsc::Sender<crate::btmedia::Command>,
    aa: tokio::sync::mpsc::Sender<AaCommand>,
    battery_suspend: BatterySuspend,
) {
    let conn = match Connection::system().await {
        Ok(c) => c,
        Err(e) => {
            log::warn!("Power monitoring disabled — no system D-Bus connection: {e}");
            return;
        }
    };

    // ── systemd-logind: suspend/resume ────────────────────────────────────
    let logind = match Login1ManagerProxy::new(&conn).await {
        Ok(p) => Some(p),
        Err(e) => {
            log::warn!("Suspend/resume detection disabled — logind unreachable: {e}");
            None
        }
    };
    // Subscribe before taking the lock, or a fast suspend can race past the
    // match rule being registered on the bus.
    let mut sleep_signals = match &logind {
        Some(p) => match p.receive_prepare_for_sleep().await {
            Ok(s) => Some(s),
            Err(e) => {
                log::warn!("Suspend/resume detection disabled — cannot subscribe: {e}");
                None
            }
        },
        None => None,
    };
    // Cloned so the handler never contends with the borrow held by the stream.
    let inhibit_proxy = logind.clone();
    // Held only for its `Drop`: closing the fd is what releases the lock.
    let mut inhibitor: Option<zvariant::OwnedFd> = match &inhibit_proxy {
        Some(p) => take_delay_lock(p).await,
        None => None,
    };

    // ── UPower: charge state, mains presence, percentage ──────────────────
    let upower = connect_upower(&conn).await;
    let mut on_battery_changes = match &upower {
        Some(u) => Some(u.manager.receive_on_battery_changed().await),
        None => None,
    };
    let mut state_changes = match &upower {
        Some(u) => Some(u.device.receive_state_changed().await),
        None => None,
    };
    let mut percentage_changes = match &upower {
        Some(u) => Some(u.device.receive_percentage_changed().await),
        None => None,
    };

    // Read the opening state explicitly so it lands as one combined line.
    let mut last = LastLogged::default();
    if let Some(u) = &upower {
        log_snapshot(u, &mut last).await;
    }

    // Armed whenever mains is absent. Evaluated from the current value rather
    // than only on transitions, so starting up (or resuming) already on battery
    // still counts down.
    let mut suspend_at = arm_suspend(battery_suspend, last.on_battery.unwrap_or(false), None);

    loop {
        tokio::select! {
            _ = &mut kill_rx => {
                log::debug!("Power monitor killed");
                break;
            }

            Some(signal) = next_or_pending(&mut sleep_signals) => {
                let Ok(args) = signal.args() else { continue };
                if *args.start() {
                    log::info!("System is suspending");
                    terminate_android_auto(&aa).await;
                    // Release the delay lock so logind can actually suspend.
                    drop(inhibitor.take());
                } else {
                    log::info!("System resumed from suspend");
                    if let Some(p) = &inhibit_proxy {
                        inhibitor = take_delay_lock(p).await;
                    }
                    let _ = bt.send(crate::btmedia::Command::Reconnect).await;
                    let _ = aa.send(AaCommand::Resume).await;
                    // Power state almost certainly moved while we were asleep.
                    if let Some(u) = &upower {
                        log_snapshot(u, &mut last).await;
                    }
                    // No `OnBattery` change fires if we resumed still on
                    // battery, so re-arm from the value we just read.
                    suspend_at =
                        arm_suspend(battery_suspend, last.on_battery.unwrap_or(false), None);
                }
            }

            Some(changed) = next_or_pending(&mut on_battery_changes) => {
                if let Ok(on_battery) = changed.get().await {
                    if last.on_battery.replace(on_battery) != Some(on_battery) {
                        if on_battery {
                            log::info!("Mains power disconnected — running on battery");
                        } else {
                            log::info!("Mains power connected");
                        }
                    }
                    suspend_at = arm_suspend(battery_suspend, on_battery, suspend_at);
                }
            }

            () = wait_until(suspend_at) => {
                suspend_at = None;
                log::info!("On battery for {:?} — suspending", battery_suspend.delay);
                if let Some(p) = &inhibit_proxy
                    && let Err(e) = p.suspend(false).await
                {
                    log::warn!("Could not suspend: {e}");
                }
            }

            Some(changed) = next_or_pending(&mut state_changes) => {
                if let Ok(state) = changed.get().await
                    && last.state.replace(state) != Some(state)
                {
                    log::info!("Battery state: {}", state_name(state));
                }
            }

            Some(changed) = next_or_pending(&mut percentage_changes) => {
                if let Ok(percent) = changed.get().await {
                    let rounded = percent.round() as i64;
                    if last.percent.replace(rounded) != Some(rounded) {
                        log::debug!("Battery at {rounded}%");
                    }
                }
            }
        }
    }
}

/// Build the UPower proxies, or `None` if the daemon is absent or reports no
/// battery (e.g. a mains-only SBC).
async fn connect_upower(conn: &Connection) -> Option<UPowerState> {
    let manager = match UPowerProxy::new(conn).await {
        Ok(p) => p,
        Err(e) => {
            log::warn!("Power supply monitoring disabled — UPower unreachable: {e}");
            return None;
        }
    };

    let path = match manager.get_display_device().await {
        Ok(p) => p,
        Err(e) => {
            log::warn!("Power supply monitoring disabled — UPower not responding: {e}");
            return None;
        }
    };

    let device = match async {
        UPowerDeviceProxy::builder(conn)
            .path(path)
            .map_err(|e| e.to_string())?
            .build()
            .await
            .map_err(|e| e.to_string())
    }
    .await
    {
        Ok(d) => d,
        Err(e) => {
            log::warn!("Power supply monitoring disabled — no UPower display device: {e}");
            return None;
        }
    };

    if !device.is_present().await.unwrap_or(false) {
        log::info!("Power supply monitoring disabled — no battery present");
        return None;
    }

    Some(UPowerState { manager, device })
}

/// Ask the UI thread to end the Android Auto session and wait for it, so the
/// phone, USB and the hotspot are released before the machine goes down.
async fn terminate_android_auto(aa: &tokio::sync::mpsc::Sender<AaCommand>) {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    if aa.send(AaCommand::Suspend(ack_tx)).await.is_err() {
        log::warn!("Could not reach the UI thread to end the Android Auto session");
        return;
    }
    match tokio::time::timeout(TEARDOWN_TIMEOUT, ack_rx).await {
        Ok(Ok(())) => log::info!("Android Auto session ended before suspend"),
        Ok(Err(_)) => log::warn!("Android Auto teardown was abandoned"),
        Err(_) => log::warn!("Android Auto teardown timed out; suspending anyway"),
    }
}

/// Compute the suspend deadline for the current mains state. Keeps an existing
/// deadline rather than pushing it back, so repeated `PropertiesChanged` while
/// on battery cannot postpone the suspend indefinitely.
fn arm_suspend(
    policy: BatterySuspend,
    on_battery: bool,
    current: Option<tokio::time::Instant>,
) -> Option<tokio::time::Instant> {
    if !policy.enabled {
        return None;
    }
    match (on_battery, current) {
        (false, Some(_)) => {
            log::info!("Mains power back — cancelling the pending suspend");
            None
        }
        (false, None) => None,
        (true, Some(at)) => Some(at),
        (true, None) => {
            log::info!("On battery — suspending in {:?}", policy.delay);
            Some(tokio::time::Instant::now() + policy.delay)
        }
    }
}

/// Wait for a deadline, or forever when none is set.
async fn wait_until(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Take a logind *delay* inhibitor so `PrepareForSleep(true)` arrives with a
/// usable window before the machine actually goes down.
async fn take_delay_lock(proxy: &Login1ManagerProxy<'_>) -> Option<zvariant::OwnedFd> {
    match proxy
        .inhibit(
            "sleep",
            "eva-navigation-unit",
            "Preparing head unit for sleep",
            "delay",
        )
        .await
    {
        Ok(fd) => {
            log::debug!("Holding logind sleep delay lock");
            Some(fd)
        }
        Err(e) => {
            log::warn!("Could not take logind sleep delay lock: {e}");
            None
        }
    }
}

/// Log the full power-supply state in one line.
async fn log_snapshot(upower: &UPowerState, last: &mut LastLogged) {
    let state = upower.device.state().await.unwrap_or(0);
    let percent = upower.device.percentage().await.unwrap_or(0.0);
    let on_battery = upower.manager.on_battery().await.unwrap_or(false);

    last.state = Some(state);
    last.percent = Some(percent.round() as i64);
    last.on_battery = Some(on_battery);

    log::info!(
        "Power supply: {} at {percent:.0}%, mains {}",
        state_name(state),
        if on_battery { "absent" } else { "present" }
    );
}

/// `UP_DEVICE_STATE` values as reported by UPower.
fn state_name(state: u32) -> &'static str {
    match state {
        1 => "charging",
        2 => "discharging",
        3 => "empty",
        4 => "fully charged",
        5 => "pending charge",
        6 => "pending discharge",
        _ => "unknown",
    }
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
