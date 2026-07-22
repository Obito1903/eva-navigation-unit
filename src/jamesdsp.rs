//! JamesDSP D-Bus client: controls JamesDSP's audio effects (global bypass,
//! Graphic EQ, Convolver, Equalizer, Bass Boost) over its session D-Bus
//! service, plus the background worker + channels bridging it to the UI.
//!
//! JamesDSP exposes a generic key/value config interface (mirroring its
//! `audio.conf`) rather than per-effect typed methods — see
//! `me.timschneeberger.jdsp4linux.Service` at `/jdsp4linux/service` on the
//! session bus (https://github.com/Audio4Linux/JDSP4Linux#scripting--ipc-apis).
//! Keys used here: `master_enable` (global bypass, inverted), `graphiceq_enable`,
//! `convolver_enable`, `tone_enable` (the "Equalizer" effect), `bass_enable`,
//! and `tone_eq` (the Equalizer's frequencies+gains, formatted as
//! `"f1;..;fN;g1;..;gN"`). The band count `N` is always derived at runtime by
//! parsing this string — never hardcoded — so the UI tracks whatever band
//! layout the running JamesDSP instance reports.

use std::time::Duration;

use zbus::{Connection, proxy};
use zvariant::Value;

/// How often the worker polls JamesDSP for external changes while active.
const POLL_INTERVAL: Duration = Duration::from_millis(1500);

#[proxy(
    interface = "me.timschneeberger.jdsp4linux.Service",
    default_service = "me.timschneeberger.jdsp4linux",
    default_path = "/jdsp4linux/service"
)]
trait JamesDspService {
    /// List available audio configuration keys. Used here purely as a cheap
    /// liveness probe (a `Proxy` can be constructed even when nothing owns
    /// the well-known name yet, so an actual call is needed to confirm the
    /// service is really up).
    #[zbus(name = "getKeys")]
    fn get_keys(&self) -> zbus::Result<Vec<String>>;

    /// Get a single audio configuration value as a string.
    #[zbus(name = "get")]
    fn get(&self, key: &str) -> zbus::Result<String>;

    /// Set a value without persisting it to disk (`commit()` does that).
    #[zbus(name = "set")]
    fn set(&self, key: &str, value: &Value<'_>) -> zbus::Result<()>;

    /// Set a value and persist it to disk immediately.
    #[zbus(name = "setAndCommit")]
    fn set_and_commit(&self, key: &str, value: &Value<'_>) -> zbus::Result<()>;
}

/// A single Equalizer band as reported live by JamesDSP.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct EqBandValue {
    pub(crate) freq: f32,
    pub(crate) gain: f32,
}

/// Effects toggled from the DSP/EQ settings tab (phase 2 scope only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Effect {
    GraphicEq,
    Convolver,
    Equalizer,
    BassBoost,
}

impl Effect {
    fn key(self) -> &'static str {
        match self {
            Effect::GraphicEq => "graphiceq_enable",
            Effect::Convolver => "convolver_enable",
            Effect::Equalizer => "tone_enable",
            Effect::BassBoost => "bass_enable",
        }
    }
}

/// A live snapshot of the JamesDSP state relevant to the DSP/EQ tab.
#[derive(Debug, Clone, Default)]
pub(crate) struct Snapshot {
    /// Whether the JamesDSP D-Bus service is currently reachable. All other
    /// fields are meaningless (left at their default) when this is `false`.
    pub(crate) connected: bool,
    pub(crate) master_enable: bool,
    pub(crate) graphiceq_enable: bool,
    pub(crate) convolver_enable: bool,
    pub(crate) tone_enable: bool,
    pub(crate) bass_enable: bool,
    /// Equalizer bands in ascending frequency order, straight from `tone_eq`.
    pub(crate) bands: Vec<EqBandValue>,
}

/// Commands sent from the UI thread to the worker.
pub(crate) enum Command {
    /// Enable/disable the whole DSP chain (`master_enable`). `enabled` is the
    /// non-inverted value — callers translate a "bypass" toggle themselves.
    MasterEnable(bool),
    /// Enable/disable one of the phase-2 effect toggles.
    Effect(Effect, bool),
    /// Set one Equalizer band's gain (dB). `commit = false` applies it live
    /// without persisting (used while dragging); `commit = true` persists it
    /// (used on release).
    EqBand { index: usize, gain: f32, commit: bool },
    /// Pause/resume polling — the UI pauses this while the DSP/EQ tab isn't
    /// visible to avoid needless D-Bus traffic.
    ///
    /// Not wired up yet: `ui.rs` currently polls continuously as a
    /// simplification (no per-tab visibility plumbing from `settings.slint`
    /// yet). Kept for when that's added.
    #[allow(dead_code)]
    PollingActive(bool),
}

/// Events sent from the worker to the UI thread.
pub(crate) enum Event {
    Snapshot(Snapshot),
}

/// Thin async wrapper around a live JamesDSP D-Bus connection.
struct JamesDsp {
    proxy: JamesDspServiceProxy<'static>,
}

impl JamesDsp {
    /// Connect to JamesDSP's session-bus service, if it's currently running.
    /// Never fails loudly: any error (no session bus, service not running,
    /// unexpected D-Bus error) simply yields `None`, mirroring the
    /// `discover() -> Option<Self>` pattern used by `controls::Volume`/
    /// `controls::Backlight` for other optionally-present hardware/services.
    async fn connect() -> Option<Self> {
        let conn = Connection::session().await.ok()?;
        let proxy = JamesDspServiceProxy::new(&conn).await.ok()?;
        // Constructing a `Proxy` succeeds even if nothing owns the
        // well-known name yet, so probe with a real call to confirm the
        // service is actually alive before handing it out.
        proxy.get_keys().await.ok()?;
        Some(Self { proxy })
    }

    async fn get_bool(&self, key: &str) -> Option<bool> {
        match self.proxy.get(key).await.ok()?.trim() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        }
    }

    async fn set_bool(&self, key: &str, value: bool, commit: bool) -> Result<(), String> {
        let variant = Value::from(value);
        if commit {
            self.proxy
                .set_and_commit(key, &variant)
                .await
                .map_err(|e| e.to_string())
        } else {
            self.proxy.set(key, &variant).await.map_err(|e| e.to_string())
        }
    }

    async fn get_eq_bands(&self) -> Option<Vec<EqBandValue>> {
        let raw = self.proxy.get("tone_eq").await.ok()?;
        parse_tone_eq(&raw)
    }

    /// Set the gain of a single Equalizer band, re-reading the current
    /// `tone_eq` value first so every other band/frequency is preserved.
    /// `index`/band count are never assumed — they come from whatever
    /// `tone_eq` reports right now.
    async fn set_eq_band(&self, index: usize, gain: f32, commit: bool) -> Result<(), String> {
        let raw = self.proxy.get("tone_eq").await.map_err(|e| e.to_string())?;
        let mut bands = parse_tone_eq(&raw).ok_or_else(|| "invalid tone_eq value".to_string())?;
        let band_count = bands.len();
        let band = bands
            .get_mut(index)
            .ok_or_else(|| format!("EQ band index {index} out of range (have {band_count})"))?;
        band.gain = gain;
        let new_raw = serialize_tone_eq(&bands);
        let variant = Value::from(new_raw);
        if commit {
            self.proxy
                .set_and_commit("tone_eq", &variant)
                .await
                .map_err(|e| e.to_string())
        } else {
            self.proxy
                .set("tone_eq", &variant)
                .await
                .map_err(|e| e.to_string())
        }
    }

    /// Gather a full snapshot, using `getKeys()` as a liveness probe first so
    /// a service that has disappeared mid-session is reliably detected
    /// (individual `get()` calls could otherwise be mistaken for "false").
    async fn try_snapshot(&self) -> Option<Snapshot> {
        self.proxy.get_keys().await.ok()?;
        Some(Snapshot {
            connected: true,
            master_enable: self.get_bool("master_enable").await.unwrap_or(false),
            graphiceq_enable: self.get_bool("graphiceq_enable").await.unwrap_or(false),
            convolver_enable: self.get_bool("convolver_enable").await.unwrap_or(false),
            tone_enable: self.get_bool("tone_enable").await.unwrap_or(false),
            bass_enable: self.get_bool("bass_enable").await.unwrap_or(false),
            bands: self.get_eq_bands().await.unwrap_or_default(),
        })
    }
}

/// Parse a `tone_eq` value (`"f1;..;fN;g1;..;gN"`) into `N` bands. Returns
/// `None` if the string doesn't have an even, non-zero number of
/// semicolon-separated numeric fields.
fn parse_tone_eq(raw: &str) -> Option<Vec<EqBandValue>> {
    let fields: Vec<&str> = raw
        .trim()
        .trim_matches('"')
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if fields.is_empty() || !fields.len().is_multiple_of(2) {
        return None;
    }
    let n = fields.len() / 2;
    let mut bands = Vec::with_capacity(n);
    for i in 0..n {
        let freq: f32 = fields[i].parse().ok()?;
        let gain: f32 = fields[n + i].parse().ok()?;
        bands.push(EqBandValue { freq, gain });
    }
    Some(bands)
}

/// Re-serialize bands back into the `tone_eq` string format.
fn serialize_tone_eq(bands: &[EqBandValue]) -> String {
    let freqs: Vec<String> = bands.iter().map(|b| format!("{:.7}", b.freq)).collect();
    let gains: Vec<String> = bands.iter().map(|b| format!("{:.7}", b.gain)).collect();
    format!("{};{}", freqs.join(";"), gains.join(";"))
}

/// Owns the background thread + tokio runtime that talks to JamesDSP, and
/// the channels bridging it to the UI thread. Mirrors the shape of
/// `container::AndroidAutoContainer`, minus the AA-specific protocol
/// machinery.
pub(crate) struct JamesDspContainer {
    thread: Option<std::thread::JoinHandle<()>>,
    pub(crate) recv: tokio::sync::mpsc::Receiver<Event>,
    pub(crate) send: tokio::sync::mpsc::Sender<Command>,
    kill: Option<tokio::sync::oneshot::Sender<()>>,
}

impl JamesDspContainer {
    pub(crate) fn new() -> Self {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Command>(32);
        let (evt_tx, evt_rx) = tokio::sync::mpsc::channel::<Event>(8);
        let (kill_tx, mut kill_rx) = tokio::sync::oneshot::channel::<()>();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build tokio runtime");

        let thread = std::thread::spawn(move || {
            rt.block_on(async move {
                let mut dsp: Option<JamesDsp> = None;
                // Polling starts active; the UI pauses it once wired (see
                // ui.rs) whenever the DSP/EQ tab isn't the visible one.
                let mut polling_active = true;
                let mut ticker = tokio::time::interval(POLL_INTERVAL);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

                loop {
                    tokio::select! {
                        _ = &mut kill_rx => {
                            log::debug!("JamesDSP worker killed");
                            break;
                        }
                        cmd = cmd_rx.recv() => {
                            let Some(cmd) = cmd else { break; };
                            let mut push_snapshot = true;
                            match cmd {
                                Command::PollingActive(active) => {
                                    polling_active = active;
                                    push_snapshot = false;
                                }
                                Command::MasterEnable(on) => {
                                    if let Some(d) = &dsp
                                        && let Err(e) = d.set_bool("master_enable", on, true).await {
                                            log::warn!("JamesDSP: failed to set master_enable: {e}");
                                        }
                                }
                                Command::Effect(effect, on) => {
                                    if let Some(d) = &dsp
                                        && let Err(e) = d.set_bool(effect.key(), on, true).await {
                                            log::warn!(
                                                "JamesDSP: failed to set {}: {e}",
                                                effect.key()
                                            );
                                        }
                                }
                                Command::EqBand { index, gain, commit } => {
                                    if let Some(d) = &dsp
                                        && let Err(e) = d.set_eq_band(index, gain, commit).await {
                                            log::warn!(
                                                "JamesDSP: failed to set EQ band {index}: {e}"
                                            );
                                        }
                                    // Skip the round-trip snapshot while the
                                    // user is still dragging a slider — it
                                    // would just fight the UI's own live
                                    // value and add needless D-Bus traffic.
                                    push_snapshot = commit;
                                }
                            }
                            if push_snapshot {
                                let snapshot = match &dsp {
                                    Some(d) => match d.try_snapshot().await {
                                        Some(snap) => snap,
                                        None => {
                                            dsp = None;
                                            Snapshot::default()
                                        }
                                    },
                                    None => Snapshot::default(),
                                };
                                let _ = evt_tx.send(Event::Snapshot(snapshot)).await;
                            }
                        }
                        _ = ticker.tick(), if polling_active => {
                            if dsp.is_none() {
                                dsp = JamesDsp::connect().await;
                            }
                            let snapshot = match &dsp {
                                Some(d) => match d.try_snapshot().await {
                                    Some(snap) => snap,
                                    None => {
                                        dsp = None;
                                        Snapshot::default()
                                    }
                                },
                                None => Snapshot::default(),
                            };
                            let _ = evt_tx.send(Event::Snapshot(snapshot)).await;
                        }
                    }
                }
            });
        });

        Self {
            thread: Some(thread),
            recv: evt_rx,
            send: cmd_tx,
            kill: Some(kill_tx),
        }
    }
}

impl Drop for JamesDspContainer {
    fn drop(&mut self) {
        let _ = self.kill.take().map(|s| s.send(()));
        // Join off the current thread in the background so a UI-thread drop
        // never blocks the event loop — same rationale as
        // `AndroidAutoContainer::drop`.
        if let Some(thread) = self.thread.take() {
            std::thread::spawn(move || {
                if let Err(e) = thread.join() {
                    log::warn!("JamesDSP worker thread panicked on shutdown: {e:?}");
                }
            });
        }
    }
}
