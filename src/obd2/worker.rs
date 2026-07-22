//! Background OBD2 polling loop: connects to the configured ELM327, then
//! repeatedly requests each configured PID and evaluates its formula.

use std::time::Duration;

use obd2_core::adapter::Adapter;
use obd2_core::adapter::elm327::Elm327Adapter;
use obd2_core::protocol::service::Target;
use obd2_core::session::Session;

use crate::config::{Obd2Config, Obd2PidConfig};
use crate::messages::Obd2Update;

use super::transport::BluetoothRfcommTransport;

/// A configured PID with its formula pre-parsed at startup, so the polling
/// hot path is just a context bind + eval, not a re-parse every tick.
struct CompiledPid {
    cfg: Obd2PidConfig,
    formula: meval::Expr,
}

/// Runs until the container is dropped (raced against its kill signal by
/// `Obd2Container::new`). Returns early (and never emits anything) if OBD2
/// is disabled, no device address is configured, or no PID formulas parse
/// successfully.
pub(super) async fn run(cfg: Obd2Config, tx: tokio::sync::mpsc::Sender<Obd2Update>) {
    if !cfg.enabled {
        log::debug!("obd2: disabled, not starting");
        return;
    }
    let Some(device_address) = cfg.device_address.clone() else {
        log::warn!("obd2: enabled but no device_address configured; not starting");
        return;
    };

    let poll_interval = Duration::from_millis(cfg.poll_interval_ms as u64);
    let pids: Vec<CompiledPid> = cfg
        .pids
        .into_iter()
        .filter_map(|pid_cfg| match pid_cfg.formula.parse::<meval::Expr>() {
            Ok(formula) => Some(CompiledPid {
                cfg: pid_cfg,
                formula,
            }),
            Err(e) => {
                log::warn!(
                    "obd2: PID '{}' has an invalid formula '{}' ({e}); skipping",
                    pid_cfg.name,
                    pid_cfg.formula
                );
                None
            }
        })
        .collect();

    if pids.is_empty() {
        log::warn!("obd2: no valid PIDs configured; not starting");
        return;
    }

    let mut backoff = Duration::from_secs(1);
    const MAX_BACKOFF: Duration = Duration::from_secs(30);

    loop {
        match connect_and_poll(&device_address, &pids, poll_interval, &tx).await {
            Ok(()) => backoff = Duration::from_secs(1),
            Err(e) => {
                log::warn!("obd2: {e}; retrying in {backoff:?}");
                let _ = tx.send(Obd2Update::Disconnected).await;
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

/// Connects once, then polls forever until a request fails (treated as a
/// dead link, triggering a reconnect from the caller's retry loop).
async fn connect_and_poll(
    device_address: &str,
    pids: &[CompiledPid],
    poll_interval: Duration,
    tx: &tokio::sync::mpsc::Sender<Obd2Update>,
) -> Result<(), String> {
    let transport = BluetoothRfcommTransport::connect(device_address).await?;
    let mut adapter = Elm327Adapter::new(Box::new(transport));
    adapter
        .initialize()
        .await
        .map_err(|e| format!("ELM327 initialize failed: {e}"))?;
    let mut session = Session::new(adapter);

    log::info!("obd2: connected to {device_address}");
    let _ = tx.send(Obd2Update::Connected).await;

    loop {
        let tick_start = std::time::Instant::now();

        for pid in pids {
            match session
                .raw_request(pid.cfg.service, &pid.cfg.data, Target::Broadcast)
                .await
            {
                Ok(bytes) => match evaluate(&pid.formula, &bytes) {
                    Some(value) => {
                        let _ = tx
                            .send(Obd2Update::Reading {
                                name: pid.cfg.name.clone(),
                                value,
                                unit: pid.cfg.unit.clone(),
                            })
                            .await;
                    }
                    None => log::debug!(
                        "obd2: PID '{}' formula evaluation failed for bytes {bytes:?}",
                        pid.cfg.name
                    ),
                },
                Err(e) => {
                    return Err(format!("PID '{}' request failed: {e}", pid.cfg.name));
                }
            }
        }

        // Sleep only for however long is left in this tick, so the cycle
        // period stays close to `poll_interval` regardless of how long the
        // PIDs themselves took to request (each is a synchronous round trip
        // over the ELM327 link — see docs/obd2.md's batching limitations).
        let elapsed = tick_start.elapsed();
        if let Some(remaining) = poll_interval.checked_sub(elapsed) {
            tokio::time::sleep(remaining).await;
        } else {
            log::warn!(
                "obd2: polling {} PIDs took {elapsed:?}, longer than poll_interval_ms \
                 ({poll_interval:?}); running back-to-back with no sleep this tick",
                pids.len()
            );
        }
    }
}

/// Binds response bytes to `A`, `B`, `C`, ... (the SAE/Wikipedia OBD-II PID
/// convention: <https://en.wikipedia.org/wiki/OBD-II_PIDs>) and evaluates
/// the formula.
fn evaluate(formula: &meval::Expr, bytes: &[u8]) -> Option<f64> {
    let mut ctx = meval::Context::new();
    for (i, byte) in bytes.iter().enumerate().take(26) {
        let name = (b'A' + i as u8) as char;
        ctx.var(name.to_string(), *byte as f64);
    }
    formula.eval_with_context(ctx).ok()
}
