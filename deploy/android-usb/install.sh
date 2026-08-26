#!/usr/bin/env bash
#
# Installer for USB device permissions needed by wired Android Auto (AOA).
# Run as root:
#
#   sudo ./install.sh [USER]
#
# USER is the (unprivileged) account that runs eva-ui and needs to open the
# phone's USB device node. Defaults to $SUDO_USER, then the logname.
#
# Fixes: "Failed to open android device failed to open device (errno 13)"
# (EACCES) when eva-ui is run as a normal user.

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
echo "Installing USB udev rule, authorising user: $EVA_USER"

# 1. plugdev group (some distros don't ship it by default).
if ! getent group plugdev >/dev/null 2>&1; then
    groupadd --system plugdev
    echo "Created group: plugdev"
fi
usermod -aG plugdev "$EVA_USER"
echo "Added $EVA_USER to group: plugdev"

# 2. udev rule.
install -D -m 0644 "$SRC_DIR/51-eva-android-auto.rules" \
    /etc/udev/rules.d/51-eva-android-auto.rules

# 3. Reload rules and re-trigger so an already-connected phone picks it up.
udevadm control --reload-rules
udevadm trigger --subsystem-match=usb

cat <<EOF

USB permissions installed.

  Rule  : /etc/udev/rules.d/51-eva-android-auto.rules
  Group : plugdev (member: $EVA_USER)

$EVA_USER must start a NEW login session for the group membership to take
effect (log out/in, or reconnect if using SSH). Then unplug and replug the
phone so udev re-applies permissions to its device node.
EOF
