# EVA UI (A310)

## Build Prerequisites (Fedora)

Install required system libraries:

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

| Group | Packages | Required by |
|-------|----------|-------------|
| Build tools | gcc, gcc-c++, make, pkgconf-pkg-config, perl | C/C++ compilation, pkg-config |
| Crypto | clang, clang-devel | aws-lc-rs bindgen |
| Protobuf | protobuf-compiler | android-auto build script |
| UI | fontconfig-devel, libxcb-devel, libxkbcommon-devel, libxkbcommon-x11-devel, wayland-devel, mesa-libGL-devel, mesa-libEGL-devel, openssl-devel | Slint (windowing, fonts, OpenGL) |
| Audio | alsa-lib-devel | cpal (ALSA) |
| D-Bus | dbus-devel | zbus, NetworkManager client |
| Video | nasm | OpenH264 asm optimizations |

### Runtime

sudo dnf install bluez NetworkManager

- BlueZ for Bluetooth (wireless transport)
- NetworkManager for Wi-Fi hotspot

## Build & Run

cargo build --release
./target/release/a310