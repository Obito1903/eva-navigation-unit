//! Background OBD2 polling loop: connects to the configured ELM327, then
//! repeatedly requests each configured PID and evaluates its formula.

use std::time::Duration;

use obd2_core::adapter::elm327::Elm327Adapter;
use obd2_core::error::Obd2Error;
use obd2_core::protocol::service::Target;
use obd2_core::session::Session;
use obd2_core::vehicle::PhysicalAddress;

use crate::config::{Obd2Config, Obd2PidConfig};
use crate::messages::Obd2Update;

use super::transport::BluetoothRfcommTransport;

/// Whether a service ID is an enhanced (manufacturer-specific) read, which
/// is sent via [`Session::raw_physical_request`] against a resolved CAN
/// header rather than the raw Mode 01 broadcast escape hatch. Bypassing
/// `Session::read_enhanced` here is deliberate: that path only resolves a
/// module's CAN header from a loaded `VehicleSpec`'s discovery profile, and
/// we don't load one — `raw_physical_request` lets us supply the resolved
/// [`PhysicalAddress`] directly, while still getting the adapter's real
/// `AT SH` header-switching behaviour (see `resolve_module_header` below).
fn is_enhanced_service(service: u8) -> bool {
    matches!(service, 0x21 | 0x22)
}

/// Known symbolic ECU module names, mapped to their standard SAE J1979-2
/// 11-bit physical CAN addressing (`ecuN` request `0x7E0 + N-1`, response
/// `request + 8`). Only `ecm`/`tcm` are covered by the standard; other
/// modules vary by manufacturer, so anything else is parsed as a raw hex
/// header instead (e.g. `module = "714"`).
fn resolve_module_header(module: &str) -> Option<PhysicalAddress> {
    let request_id = match module.to_ascii_lowercase().as_str() {
        "ecm" => 0x7E0,
        "tcm" => 0x7E1,
        _ => {
            let hex = module.trim().strip_prefix("0x").unwrap_or(module.trim());
            u16::from_str_radix(hex, 16).ok()?
        }
    };
    Some(PhysicalAddress::Can11Bit {
        request_id,
        response_id: request_id + 8,
    })
}

/// A configured PID with its formula pre-parsed at startup, so the polling
/// hot path is just a context bind + eval, not a re-parse every tick.
struct CompiledPid {
    cfg: Obd2PidConfig,
    formula: meval::Expr,
    /// Resolved CAN header for enhanced (service `0x21`/`0x22`) PIDs,
    /// pre-resolved at startup so the polling hot path never re-parses
    /// `cfg.module`. `None` for standard Mode 01 PIDs, which use functional
    /// broadcast addressing instead.
    header: Option<PhysicalAddress>,
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
        .filter_map(|pid_cfg| {
            let header = if is_enhanced_service(pid_cfg.service) {
                if pid_cfg.data.len() != 2 {
                    log::warn!(
                        "obd2: PID '{}' uses enhanced service {:#04x} but its data is {} byte(s) \
                         (expected a 2-byte DID); skipping",
                        pid_cfg.name,
                        pid_cfg.service,
                        pid_cfg.data.len()
                    );
                    return None;
                }
                match resolve_module_header(&pid_cfg.module) {
                    Some(header) => Some(header),
                    None => {
                        log::warn!(
                            "obd2: PID '{}' has module '{}', which is neither 'ecm'/'tcm' nor a \
                             valid hex CAN header (e.g. '7E0'); skipping",
                            pid_cfg.name,
                            pid_cfg.module
                        );
                        return None;
                    }
                }
            } else {
                None
            };
            match pid_cfg.formula.parse::<meval::Expr>() {
                Ok(formula) => Some(CompiledPid {
                    cfg: pid_cfg,
                    formula,
                    header,
                }),
                Err(e) => {
                    log::warn!(
                        "obd2: PID '{}' has an invalid formula '{}' ({e}); skipping",
                        pid_cfg.name,
                        pid_cfg.formula
                    );
                    None
                }
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
    let adapter = Elm327Adapter::new(Box::new(transport));
    let mut session = Session::new(adapter);
    // Initialize through the session (rather than the raw adapter) so
    // `Session`'s own `initialized` flag is set — otherwise its first
    // request would silently re-run the whole ELM327 reset/handshake.
    session
        .initialize()
        .await
        .map_err(|e| format!("ELM327 initialize failed: {e}"))?;

    log::info!("obd2: connected to {device_address}");
    let _ = tx.send(Obd2Update::Connected).await;

    loop {
        let tick_start = std::time::Instant::now();

        for pid in pids {
            let result = if let Some(header) = &pid.header {
                // Enhanced (service 0x21/0x22) PID: routes through the
                // already-resolved CAN header, which makes the adapter
                // actually issue `AT SH` to switch to that module before
                // sending the request (see `resolve_module_header`).
                session
                    .raw_physical_request(pid.cfg.service, &pid.cfg.data, header.clone())
                    .await
            } else {
                session
                    .raw_request(pid.cfg.service, &pid.cfg.data, Target::Broadcast)
                    .await
            };

            match result {
                Ok(bytes) => match evaluate(&pid.formula, &bytes) {
                    Some(value) => {
                        log::debug!(
                            "obd2: PID '{}' raw={bytes:?} -> {value} {}",
                            pid.cfg.name,
                            pid.cfg.unit
                        );
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
                Err(e) if is_recoverable(&e) => {
                    log::warn!(
                        "obd2: PID '{}' request failed: {e}; skipping until next poll",
                        pid.cfg.name
                    );
                }
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

/// Whether a failed PID request indicates the request/response itself was
/// bad (vehicle didn't have the data, doesn't support the PID, rejected it,
/// or was slow to answer) rather than the ELM327 link being dead. These are
/// skipped for this tick only; the polling loop keeps going. Anything else
/// (transport/adapter/IO errors) is treated as a dead link and bubbles up to
/// trigger a reconnect.
fn is_recoverable(e: &Obd2Error) -> bool {
    matches!(
        e,
        Obd2Error::NoData
            | Obd2Error::Timeout
            | Obd2Error::UnsupportedPid { .. }
            | Obd2Error::NegativeResponse { .. }
    )
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
