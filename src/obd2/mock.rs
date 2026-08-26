//! Simulated OBD2 backend for UI testing without a real ELM327/vehicle.
//!
//! Enabled via [`Obd2Config::mock`] (`--obd2-mock` / `EVA_OBD2_MOCK` /
//! `[obd2] mock = true`). Bypasses the ELM327/`Session`/`Link` stack in
//! [`super::worker`] entirely and generates realistic, continuously-changing
//! values directly for whichever of a small set of known PID names are
//! configured (see [`profile_for`]), sent through the same [`Obd2Update`]
//! channel the real worker uses. Every signal is independent — there is no
//! attempt to correlate e.g. `gear` with `vehicle_speed`.

use std::time::Duration;

use rand::Rng;

use crate::config::{Obd2Config, Obd2PidConfig};
use crate::messages::Obd2Update;

/// Per-tick step/drift sizes below are tuned against this reference poll
/// interval; other `poll_interval_ms` values scale them so the same signal
/// still "moves" at roughly the same real-world rate.
const REFERENCE_POLL_INTERVAL_MS: f64 = 250.0;

/// A single simulated signal's behaviour and current state.
enum MockSignal {
    /// Bounded random walk: nudges `value` by up to `max_step` each tick,
    /// clamped to `[min, max]`.
    RandomWalk {
        value: f64,
        min: f64,
        max: f64,
        max_step: f64,
    },
    /// Always increases by a small random amount each tick; never resets.
    MonotonicIncrease { value: f64, max_step: f64 },
    /// Climbs from its initial value toward `[plateau_min, plateau_max]` over
    /// `ticks_remaining` ticks (simulating an engine warming up), then
    /// behaves like a `RandomWalk` clamped to the plateau range.
    WarmUp {
        value: f64,
        plateau_min: f64,
        plateau_max: f64,
        ticks_remaining: u32,
        step_per_tick: f64,
    },
    /// Random walk with a small constant downward drift; resets to `max`
    /// once it reaches `min` (simulating a refuel), so it keeps cycling
    /// during a long-running test session.
    DepletingWalk {
        value: f64,
        min: f64,
        max: f64,
        drift: f64,
        max_step: f64,
    },
    /// Only changes by ±1 every `change_every_ticks` ticks, clamped to
    /// `[min, max]` and rounded to a whole number on read.
    SteppedDiscrete {
        value: f64,
        min: f64,
        max: f64,
        change_every_ticks: u32,
        ticks_since_change: u32,
    },
}

impl MockSignal {
    /// Advances the signal by one tick and returns its current value.
    fn next(&mut self, rng: &mut impl Rng) -> f64 {
        match self {
            MockSignal::RandomWalk {
                value,
                min,
                max,
                max_step,
            } => {
                *value = (*value + rng.random_range(-*max_step..=*max_step)).clamp(*min, *max);
                *value
            }
            MockSignal::MonotonicIncrease { value, max_step } => {
                *value += rng.random_range(0.0..=*max_step);
                *value
            }
            MockSignal::WarmUp {
                value,
                plateau_min,
                plateau_max,
                ticks_remaining,
                step_per_tick,
            } => {
                if *ticks_remaining > 0 {
                    *ticks_remaining -= 1;
                    *value = (*value + *step_per_tick).min(*plateau_max);
                } else {
                    let noise = (*plateau_max - *plateau_min) * 0.05;
                    *value = (*value + rng.random_range(-noise..=noise))
                        .clamp(*plateau_min, *plateau_max);
                }
                *value
            }
            MockSignal::DepletingWalk {
                value,
                min,
                max,
                drift,
                max_step,
            } => {
                *value += rng.random_range(-*max_step..=*max_step) - *drift;
                if *value <= *min {
                    *value = *max;
                } else {
                    *value = value.clamp(*min, *max);
                }
                *value
            }
            MockSignal::SteppedDiscrete {
                value,
                min,
                max,
                change_every_ticks,
                ticks_since_change,
            } => {
                *ticks_since_change += 1;
                if *ticks_since_change >= *change_every_ticks {
                    *ticks_since_change = 0;
                    let step = if rng.random_bool(0.5) { 1.0 } else { -1.0 };
                    *value = (*value + step).clamp(*min, *max);
                }
                value.round()
            }
        }
    }
}

/// Looks up the simulated profile for a known PID name, scaling all
/// per-tick step/drift amounts (and the oil temperature warm-up duration) so
/// they move at roughly the same real-world rate regardless of
/// `poll_interval_ms`. Returns `None` for any name without a known
/// realistic profile.
fn profile_for(name: &str, poll_interval_ms: u32) -> Option<MockSignal> {
    let scale = poll_interval_ms as f64 / REFERENCE_POLL_INTERVAL_MS;
    Some(match name {
        "engine_rpm" => MockSignal::RandomWalk {
            value: 800.0,
            min: 700.0,
            max: 3000.0,
            max_step: 150.0 * scale,
        },
        "vehicle_speed" => MockSignal::RandomWalk {
            value: 0.0,
            min: 0.0,
            max: 160.0,
            max_step: 6.0 * scale,
        },
        "odometer" => MockSignal::MonotonicIncrease {
            value: 128_473.2,
            max_step: 0.05 * scale,
        },
        "fuel_level" => MockSignal::DepletingWalk {
            value: 55.0,
            min: 0.0,
            max: 65.0,
            drift: 0.02 * scale,
            max_step: 0.3 * scale,
        },
        "oil_temp" => {
            let plateau_min = 85.0;
            let plateau_max = 105.0;
            let ambient = 20.0;
            // ~2 minutes of real time to warm up, regardless of poll rate.
            let ticks_remaining = ((120_000.0 / poll_interval_ms as f64) as u32).max(1);
            MockSignal::WarmUp {
                value: ambient,
                plateau_min,
                plateau_max,
                ticks_remaining,
                step_per_tick: (plateau_min - ambient) / ticks_remaining as f64,
            }
        }
        "fuel_rate" => MockSignal::RandomWalk {
            value: 2.0,
            min: 0.6,
            max: 18.0,
            max_step: 2.0 * scale,
        },
        "gear" => MockSignal::SteppedDiscrete {
            value: 0.0,
            min: 0.0,
            max: 6.0,
            change_every_ticks: 8,
            ticks_since_change: 0,
        },
        "boost_pressure_actual" => MockSignal::RandomWalk {
            value: 1000.0,
            min: 950.0,
            max: 2000.0,
            max_step: 80.0 * scale,
        },
        "boost_pressure_commanded" => MockSignal::RandomWalk {
            value: 1000.0,
            min: 950.0,
            max: 2000.0,
            max_step: 80.0 * scale,
        },
        _ => return None,
    })
}

/// Runs until the container is dropped (raced against its kill signal by
/// `Obd2Container::new`, same as [`super::worker::run`]). Sends
/// [`Obd2Update::Connected`] once, then a [`Obd2Update::Reading`] per
/// simulated PID on every `poll_interval_ms` tick, forever — there is no
/// link to drop, so this never reconnects or emits `Disconnected`.
pub(super) async fn run(cfg: Obd2Config, tx: tokio::sync::mpsc::Sender<Obd2Update>) {
    let poll_interval = Duration::from_millis(cfg.poll_interval_ms as u64);

    let mut signals: Vec<(Obd2PidConfig, MockSignal)> = Vec::new();
    for pid_cfg in cfg.pids {
        match profile_for(&pid_cfg.name, cfg.poll_interval_ms) {
            Some(signal) => signals.push((pid_cfg, signal)),
            None => log::debug!(
                "obd2: mock has no simulated profile for PID '{}'; it will not report readings",
                pid_cfg.name
            ),
        }
    }

    if signals.is_empty() {
        log::warn!(
            "obd2: mock enabled but none of the configured PIDs have a simulated profile; not \
             starting"
        );
        return;
    }

    log::info!("obd2: mock enabled, simulating {} PID(s)", signals.len());
    let _ = tx.send(Obd2Update::Connected).await;

    let mut rng = rand::rng();
    loop {
        for (pid_cfg, signal) in &mut signals {
            let value = signal.next(&mut rng);
            log::debug!(
                "obd2: mock PID '{}' -> {value} {}",
                pid_cfg.name,
                pid_cfg.unit
            );
            let _ = tx
                .send(Obd2Update::Reading {
                    name: pid_cfg.name.clone(),
                    value,
                    unit: pid_cfg.unit.clone(),
                })
                .await;
        }
        tokio::time::sleep(poll_interval).await;
    }
}
