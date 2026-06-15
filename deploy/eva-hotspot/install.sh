#!/usr/bin/env bash
#
# Installer for the EVA Android Auto Wi-Fi hotspot (Option A: privileged
# systemd service + polkit rule). Run as root:
#
#   sudo ./install.sh [USER]
#
# USER is the (unprivileged) account that runs eva-ui and should be allowed to
# start/stop the hotspot. Defaults to $SUDO_USER, then the logname.
#
# After installation, set `hotspot_backend = 1` in eva-ui's config.toml and run
# eva-ui as a normal user — no sudo needed.

set -euo pipefail

if [[ "${EUID}" -ne 0 ]]; then
    echo "error: must be run as root (use sudo)" >&2
    exit 1
fi

SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

EVA_USER="${1:-${SUDO_USER:-$(logname 2>/dev/null || true)}}"
if [[ -z "$EVA_USER" ]]; then
    echo "error: could not determine the eva-ui user; pass it explicitly:" >&2
    echo "       sudo ./install.sh <username>" >&2
    exit 1
fi
if ! id "$EVA_USER" >/dev/null 2>&1; then
    echo "error: user '$EVA_USER' does not exist" >&2
    exit 1
fi
echo "Installing eva-hotspot, authorising user: $EVA_USER"

# 1. Privileged helper script.
install -D -m 0755 "$SRC_DIR/eva-hotspot" /usr/local/sbin/eva-hotspot

# 2. systemd unit.
install -D -m 0644 "$SRC_DIR/eva-hotspot.service" \
    /etc/systemd/system/eva-hotspot.service

# 3. Default config — installed only if absent so local edits are preserved.
if [[ ! -e /etc/eva-hotspot/hotspot.env ]]; then
    install -D -m 0644 "$SRC_DIR/hotspot.env" /etc/eva-hotspot/hotspot.env
    echo "Installed default /etc/eva-hotspot/hotspot.env"
else
    echo "Kept existing /etc/eva-hotspot/hotspot.env"
fi

# 4. polkit rule with the user substituted in.
install -d -m 0755 /etc/polkit-1/rules.d
sed "s/@EVA_USER@/$EVA_USER/g" "$SRC_DIR/49-eva-hotspot.rules" \
    > /etc/polkit-1/rules.d/49-eva-hotspot.rules
chmod 0644 /etc/polkit-1/rules.d/49-eva-hotspot.rules

# 5. Reload systemd so the new unit is visible. (No need to enable it: eva-ui
#    starts it on demand. Enable it manually for an always-on hotspot.)
systemctl daemon-reload

cat <<EOF

eva-hotspot installed.

  Helper : /usr/local/sbin/eva-hotspot
  Unit   : /etc/systemd/system/eva-hotspot.service
  Config : /etc/eva-hotspot/hotspot.env
  Polkit : /etc/polkit-1/rules.d/49-eva-hotspot.rules (user: $EVA_USER)

Next steps:
  1. Set 'hotspot_backend = 1' in eva-ui's config.toml.
  2. (Optional) Edit /etc/eva-hotspot/hotspot.env (channel, country, SSID/PSK).
  3. Test as the $EVA_USER user, without sudo:
       systemctl start eva-hotspot.service && systemctl is-active eva-hotspot.service
       systemctl stop  eva-hotspot.service
  4. Run eva-ui normally (no sudo). It will start/stop the hotspot on demand.
EOF
