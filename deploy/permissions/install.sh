#!/usr/bin/env bash
#
# Installer for device permissions eva-ui needs as a non-root user: USB access
# for wired Android Auto (AOA) and backlight write access for the screen
# brightness control. Run as root:
#
#   sudo ./install.sh [USER]
#
# USER is the (unprivileged) account that runs eva-ui. Defaults to
# $SUDO_USER, then the logname.
#
# Fixes:
#   - "Failed to open android device failed to open device (errno 13)" (EACCES)
#   - Brightness slider not working ("Failed to set screen brightness" warning)

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
echo "Installing device permissions, authorising user: $EVA_USER"

# 1. plugdev/video groups (some distros don't ship them by default).
for grp in plugdev video; do
    if ! getent group "$grp" >/dev/null 2>&1; then
        groupadd --system "$grp"
        echo "Created group: $grp"
    fi
done
usermod -aG plugdev,video "$EVA_USER"
echo "Added $EVA_USER to groups: plugdev, video"

# 2. udev rules.
install -D -m 0644 "$SRC_DIR/51-eva-android-auto.rules" \
    /etc/udev/rules.d/51-eva-android-auto.rules
install -D -m 0644 "$SRC_DIR/52-eva-backlight.rules" \
    /etc/udev/rules.d/52-eva-backlight.rules

# 3. Reload rules and re-trigger so already-present devices pick them up.
udevadm control --reload-rules
udevadm trigger --subsystem-match=usb
udevadm trigger --subsystem-match=backlight

cat <<EOF

Device permissions installed.

  Rules : /etc/udev/rules.d/51-eva-android-auto.rules
          /etc/udev/rules.d/52-eva-backlight.rules
  Groups: plugdev, video (member: $EVA_USER)

$EVA_USER must start a NEW login session for the group membership to take
effect (log out/in, or reconnect if using SSH). Then unplug/replug the phone
so udev re-applies permissions to its device node.
EOF
