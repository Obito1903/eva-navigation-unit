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
| `obd2.device_address` | `--obd2-device-address` | `EVA_OBD2_DEVICE_ADDRESS` | _(unset)_ | Bluetooth MAC address of the paired ELM327, e.g. `"AA:BB:CC:DD:EE:FF"`. |
| `obd2.poll_interval_ms` | `--obd2-poll-interval-ms` | `EVA_OBD2_POLL_INTERVAL_MS` | `250` | Poll interval for all configured PIDs, in milliseconds. |

`[[obd2.pids]]` (an array of tables) is only configurable from the TOML file
— there's no CLI/env equivalent for the PID list itself.

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

Requests go through `obd2-core`'s raw-request escape hatch (not its typed
standard-PID API), so any service/data combination your adapter and vehicle
support can be expressed this way — including manufacturer-specific PIDs.

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
