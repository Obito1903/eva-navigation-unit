# Eva-navigation-unit

A DIY car head-unit interface for SBCs and Linux tablets. It aims to be a
fully-featured head-unit replacement for older cars that either never had one
or have a factory unit worth ripping out. The theme and aesthetic are inspired
by 90s cyberpunk anime and the Evangelion universe.

![Main screen](./docs/assets/main.png)

<details>
<summary>More screenshots</summary>

| | |
|---|---|
| ![Main screen](./docs/assets/main.png) | ![Android Auto](./docs/assets/aa.png) |
| ![Settings](./docs/assets/settings.png) | ![Spectrum visualizer](./docs/assets/viz.png) |

</details>



The current target hardware is a jailbroken Nintendo Switch OLED running
L4T Fedora, but the goal is to not gatekeep the project to a single hardware
configuration — hence the growing number of configuration options available
to tailor the experience to whatever screen/SBC you're running it on.

> [!WARNING]
> This project is mostly **vibe-coded** and is still in **early development
> and testing**. Expect rough edges, half-finished features, and breaking
> changes. It is not yet ready to be relied on as your car's only head unit.

## Features

- [x] Android Auto
  - [x] USB
  - [x] Wireless (WIP)
    - Automatically sets up the access point using the selected backend
      (`hostapd` or NetworkManager)
- [x] Live spectrum analyzer for audio visualization
  - Selectable analyzer theme and shape
- [ ] Bluetooth & media control
- [ ] Embeded media player
  - [ ] Spotify connect
  - [ ] Subsonic
  - [ ] mpc/local file
- [ ] Audio Equilizer & effects
  - [X] EQ
  - [X] Effect toggles
  - [ ] Complete JamesDSP controls
- [X] Nice 90s wireframe-style interface
  - [x] Multiple color themes
- [ ] OBD2
  - Display car telemetry in retro-style gauges and segment displays
  - Send back car telemetry to AA (rpm, speed, fuel tank...)
  - Show OBD2 engine faults
- [ ] Controller/GPIO input for integration with native car headunit buttons
- [ ] Multi-point touch input for AA
- [X] System power awareness (opt-in `power` cargo feature)
  - [x] Detect suspend/resume via systemd-logind
  - [x] Detect charging/discharging, mains presence and battery level via UPower
  - [x] Reconnect the last Bluetooth device on startup and on resume, and start
        playback over AVRCP once it is back
  - [x] End the Android Auto session before suspending, and restart it after
        resume
  - [x] Suspend after a configurable time on battery (opt-in via
        `suspend_on_battery`), i.e. once the car is switched off
  - Power state is currently logged only — nothing else reacts to it yet

## Build Prerequisites (Fedora)

Install required system libraries:

```sh
sudo dnf install \
  gcc gcc-c++ make pkgconf-pkg-config perl \
  clang clang-devel \
  protobuf-compiler \
  fontconfig-devel \
  libxcb-devel libxkbcommon-devel libxkbcommon-x11-devel \
  wayland-devel mesa-libGL-devel mesa-libEGL-devel \
  openssl-devel \
  alsa-lib-devel \
  dbus-devel \
  nasm

# Runtime dependencies
sudo dnf install bluez NetworkManager pipewire-pulseaudio

# Only needed for the optional `power` feature
sudo dnf install upower
```

| Group | Packages | Required by |
|-------|----------|-------------|
| Build tools | gcc, gcc-c++, make, pkgconf-pkg-config, perl | C/C++ compilation, pkg-config |
| Crypto | clang, clang-devel | aws-lc-rs bindgen |
| Protobuf | protobuf-compiler | android-auto build script |
| UI | fontconfig-devel, libxcb-devel, libxkbcommon-devel, libxkbcommon-x11-devel, wayland-devel, mesa-libGL-devel, mesa-libEGL-devel, openssl-devel | Slint (windowing, fonts, OpenGL) |
| Audio | alsa-lib-devel | cpal (ALSA) |
| D-Bus | dbus-devel | zbus, NetworkManager client |
| Video | nasm | OpenH264 asm optimizations |
| Runtime | bluez | Bluetooth (wireless transport) |
| Runtime | NetworkManager | Wi-Fi hotspot |
| Runtime | pipewire-pulseaudio (or pulseaudio) | Audio capture for the spectrum analyzer/visualizer |
| Runtime (optional) | upower | Power-supply state for the `power` feature |

## Build

```sh
cargo build --release
```

System power monitoring is off by default; enable it with:

```sh
cargo build --release --features power
```

The remembered Bluetooth device is stored as `last_bt_device` in the config
file and rewritten whenever a different device connects.

## Installing the Wi-Fi hotspot service (for Android Auto wireless)

> This step is only required when using the `hostapd` hotspot backend. If
> you're using the NetworkManager backend instead, you can skip it.

Android Auto wireless needs a privileged Wi-Fi access point. This is handled
by a small systemd service + polkit rule so the head-unit app itself never
needs to run as root:

```sh
cd deploy/eva-hotspot
sudo ./install.sh <username>             # one-time, needs root; <username> is
                                          # the account that runs eva-ui

# verify polkit works WITHOUT sudo:
systemctl start eva-hotspot.service && systemctl is-active eva-hotspot.service
systemctl stop  eva-hotspot.service
```

Then set `hotspot_backend = 1` (or the desired backend) in eva-ui's
`config.toml`. See [deploy/eva-hotspot/install.sh](deploy/eva-hotspot/install.sh)
and [deploy/eva-hotspot/hotspot.env](deploy/eva-hotspot/hotspot.env) for the
available options (SSID/PSK, channel, country code, DHCP range).

## Device permissions (USB + screen brightness)

USB device nodes and the backlight sysfs interface are root-only by default,
so running eva-ui as a normal user fails to open the phone (`Failed to open
android device failed to open device (errno 13)`, EACCES) and the brightness
slider silently does nothing. Install the udev rules once to grant access:

```sh
cd deploy/permissions
sudo ./install.sh <username>             # one-time, needs root
```

The target user must start a new login session (log out/in, or reconnect over
SSH) for the added group memberships to apply, then unplug/replug the phone.

## Run

```sh
cargo build --release
DISPLAY=:0 ./target/release/eva-navigation-unit &> eva-ui.log   # NOTE: no sudo
```

## Configuration

`eva-navigation-unit` is configured via a TOML file, environment variables,
and CLI flags. See [docs/configuration.md](docs/configuration.md) for the
config file location, precedence rules, and the full list of options.

## Thanks

This project would not be possible without
[uglyoldbob/android-auto](https://github.com/uglyoldbob/android-auto), which
provides the Android Auto server implementation.
