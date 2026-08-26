# OBD2 Telemetry

`eva-navigation-unit` can poll vehicle telemetry from a Bluetooth ELM327
adapter and evaluate user-defined formulas on the raw response bytes. This is
**early plumbing**: it connects, polls, and logs readings, but there is no UI
integration yet — readings aren't shown anywhere in the app.

Requires building with the `obd2` cargo feature (off by default):

```sh
cargo run --features obd2
```

## How it connects

- Uses a Bluetooth **RFCOMM (Serial Port Profile)** connection to the ELM327,
  built directly on the `bluetooth-rust` dependency already used for Android
  Auto — not `obd2-core`'s own serial/BLE transports, and not a kernel
  SocketCAN/`can327` bridge.
- The adapter must already be **paired** at the OS level (`bluetoothctl` or
  your desktop's Bluetooth settings) — there is no in-app pairing/discovery
  yet, just a configured MAC address.
- The RFCOMM channel is discovered via SDP, falling back to channel 1 (the
  conventional SPP channel for most ELM327 adapters) if that fails.
- Runs on its own dedicated background thread, independent of the Android
  Auto worker's lifecycle — it connects and polls regardless of whether an AA
  session is active.
- On any request failure the worker tears down and reconnects with
  exponential backoff (1s up to 30s).

## Options (`[obd2]` table)

| Config key (TOML) | CLI flag | Env var | Default | Description |
|---|---|---|---|---|
| `obd2.enabled` | `--obd2-enabled` | `EVA_OBD2_ENABLED` | `false` | Enable the OBD2 worker. |
| `obd2.device_address` | `--obd2-device-address` | `EVA_OBD2_DEVICE_ADDRESS` | _(unset)_ | Bluetooth MAC address of the paired ELM327, e.g. `"AA:BB:CC:DD:EE:FF"`. Not required when `mock` is `true`. |
| `obd2.mock` | `--obd2-mock` | `EVA_OBD2_MOCK` | `false` | Simulate the configured PIDs instead of connecting to a real ELM327 — see [Mock mode](#mock-mode-no-hardware). |
| `obd2.poll_interval_ms` | `--obd2-poll-interval-ms` | `EVA_OBD2_POLL_INTERVAL_MS` | `250` | Poll interval for all configured PIDs, in milliseconds. |

`[[obd2.pids]]` (an array of tables) is only configurable from the TOML file
— there's no CLI/env equivalent for the PID list itself.

## Mock mode (no hardware)

Setting `obd2.mock = true` (or `--obd2-mock` / `EVA_OBD2_MOCK=true`) skips the
ELM327/Bluetooth connection entirely and instead generates realistic,
continuously-changing values for whichever configured PIDs it recognises by
`name` — useful for testing UI/telemetry consumers without a car or adapter
present. `device_address` is ignored (and not required) in this mode.

Each simulated signal is an independent bounded random walk (or a small
variation — a warm-up curve, a depleting-then-refuelling tank, an
ever-increasing odometer, or an infrequently-changing gear) tuned to stay
within realistic bounds for that value. There is no correlation between
signals (e.g. `gear` does not track `vehicle_speed`).

Recognised PID names (see `profile_for` in `src/obd2/mock.rs` for exact
bounds): `engine_rpm`, `vehicle_speed`, `odometer`, `fuel_level`, `oil_temp`,
`fuel_rate`, `gear`, `boost_pressure_actual`, `boost_pressure_commanded`. Any
other configured PID name has no simulated profile and is skipped (logged at
`debug`) — mock mode only fakes physical values by name, it does not
simulate the underlying request/response/formula pipeline at all.

## Defining PIDs (`[[obd2.pids]]`)

Each entry describes one request/response pair and how to turn the response
bytes into a physical value:

```toml
[[obd2.pids]]
name = "engine_rpm"
service = 1
pid = "0C"
formula = "(A * 256 + B) / 4"
unit = "rpm"
```

| Field | Meaning |
|---|---|
| `name` | Identifier used in logs (and later, the UI). |
| `service` | OBD-II service/mode byte, e.g. `1` (show current data) or `0x22` (VAG/manufacturer-specific read-by-identifier). |
| `pid` | Hex string of the request data that follows the service byte — one byte for standard Mode 01 PIDs (`"0C"`), two bytes for enhanced Mode 22 DIDs (`"100C"`). Always an even number of hex digits; each pair is one byte, so byte count is never ambiguous. |
| `formula` | Expression evaluated with the response bytes bound to `A`, `B`, `C`, `D`, ... (the SAE/Wikipedia [OBD-II PIDs](https://en.wikipedia.org/wiki/OBD-II_PIDs) convention) — formulas from that page can be pasted in directly. Evaluated with [`meval`](https://docs.rs/meval). Invalid formulas are logged and skipped at startup rather than crashing the app. |
| `unit` | Arbitrary physical unit label attached to the reading (e.g. `"rpm"`, `"°C"`, `"km/h"`), not otherwise interpreted. |
| `module` | ECU module targeted by enhanced (service `0x21`/`0x22`) PIDs: `"ecm"`, `"tcm"` (SAE J1979-2 standard 11-bit CAN addressing), or a raw hex request header (e.g. `"714"`) for modules outside that standard. Ignored for standard Mode 01 PIDs. Defaults to `"ecm"` when omitted. |

Standard Mode 01 PIDs go through `obd2-core`'s raw-request escape hatch, so
any service/data combination your adapter and vehicle support can be
expressed this way. Enhanced PIDs (service `0x21`/`0x22`) go through
[`Session::raw_physical_request`](https://docs.rs/obd2-core/latest/obd2_core/session/struct.Session.html#method.raw_physical_request)
against the CAN header resolved from `module` — the adapter issues `AT SH`
to switch to that header before the request (see `obd2::worker::resolve_module_header`
and "Enhanced PID addressing" below) — which requires exactly a 2-byte DID
in `pid`; PIDs with an invalid byte count for their service are logged and
skipped at startup rather than crashing the app.

### Standard Mode 01 PIDs

Common ones, taken directly from the
[Wikipedia OBD-II PID table](https://en.wikipedia.org/wiki/OBD-II_PIDs):

```toml
[[obd2.pids]]
name = "engine_rpm"
service = 1
pid = "0C"
formula = "(A * 256 + B) / 4"
unit = "rpm"

[[obd2.pids]]
name = "vehicle_speed"
service = 1
pid = "0D"
formula = "A"
unit = "km/h"

[[obd2.pids]]
name = "odometer"
service = 1
pid = "A6"
formula = "(A * (2^24) + B * (2^16) + C * (2^8) + D) / 10"
unit = "km"
```

### VAG enhanced PIDs (service 0x22)

VW/Audi/Seat/Škoda ECUs expose additional manufacturer-specific PIDs over
service `0x22` with a 2-byte DID. The following were ported from
[`Obito1903/obd_exporter`](https://github.com/Obito1903/obd_exporter/blob/main/config.yaml)
for an Audi A4 B8 (2.0 TDI, 2009):

```toml
[[obd2.pids]]
name = "fuel_level"
service = 0x22
pid = "100C"
formula = "A * 256 + B"
unit = "L"

[[obd2.pids]]
name = "oil_temp"
service = 0x22
pid = "11BE"
formula = "(A * 256 + B) - 40"
unit = "°C"

[[obd2.pids]]
name = "fuel_rate"
service = 0x22
pid = "111A"
formula = "(A * 256 + B) * 0.05"
unit = "l/h"

[[obd2.pids]]
name = "gear"
service = 0x22
pid = "100D"
formula = "A"
unit = ""

[[obd2.pids]]
name = "boost_pressure_actual"
service = 0x22
pid = "1057"
formula = "A * 256 + B"
unit = "hPa"

[[obd2.pids]]
name = "boost_pressure_commanded"
service = 0x22
pid = "1149"
formula = "A * 256 + B"
unit = "hPa"
```

> The `count`-decoded fields above (`fuel_level`, `gear`, boost pressures)
> apply no additional scaling beyond the raw integer — that matches the
> source tool's behaviour, but hasn't been independently verified against
> real hardware. Sanity-check readings against known values (fuel gauge,
> actual gear, etc.) before trusting them.

### Enhanced PID addressing (`module`)

Enhanced PIDs need the ELM327 pointed at a specific ECU's CAN request ID
before the request goes out (an `AT SH` command), otherwise the functional
broadcast address the adapter auto-negotiated is used, and the vehicle either
doesn't answer or answers from the wrong module. `module` controls this:

- `"ecm"` → `0x7E0` request / `0x7E8` response (SAE J1979-2 standard
  physical addressing, ECU #1) — the default.
- `"tcm"` → `0x7E1` request / `0x7E9` response (ECU #2).
- Anything else is parsed as a raw hex 11-bit request header (e.g.
  `module = "714"`); the response ID is assumed to be the request ID + 8
  (the standard physical-addressing convention — override by editing
  `resolve_module_header` in `src/obd2/worker.rs` if your module doesn't
  follow it).

Only `ecm`/`tcm` are standardized; other VAG modules (ABS, airbag, instrument
cluster, ...) use manufacturer-specific addressing that varies by platform,
so look up the header for your vehicle and pass it directly, e.g.
`module = "714"`.

## Formula variable convention

Response bytes are bound to `A`, `B`, `C`, `D`, ... in order (up to 26,
`A`–`Z`), matching how formulas are documented on the
[Wikipedia OBD-II PID page](https://en.wikipedia.org/wiki/OBD-II_PIDs) — so
you can copy a formula from that table verbatim. If a formula references a
variable beyond the number of bytes actually returned, evaluation fails for
that PID and it's skipped (logged at `debug`) for that poll cycle rather than
crashing the worker.

## Current limitations

- No UI: readings are only visible via logs (`log::debug!`) until the UI is
  wired up.
- No in-app Bluetooth pairing/discovery — pair the adapter with the OS first
  and hardcode its MAC in `device_address`.
- One PID request per poll tick, requested sequentially — there's no batching
  or per-PID interval yet, so a long PID list scales linearly with
  `poll_interval_ms`.
- `obd2-core` is vendored as a local path dependency (a sibling checkout of
  [`trepidity/obd2-core`](https://github.com/trepidity/obd2-core)) rather
  than the published crates.io `0.2` release, since enhanced-PID header
  switching (`Session::raw_physical_request`, real `AT SH` support in the
  ELM327 adapter) isn't in a published release yet.
