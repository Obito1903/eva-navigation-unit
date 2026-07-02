#!/usr/bin/env bash
#
# Install the built eva-ui release as a per-user desktop application.
#
# Copies the release binary into ~/.local/bin and writes a .desktop launcher
# into ~/.local/share/applications so it shows up in the application menu.
# The binary is built first if it is missing.
#
# Once installed, eva-ui reads its configuration from
# ~/.config/eva-ui/config.toml (it only falls back to a local ./config.toml
# when one exists in the working directory, e.g. during development).
#
# Override the install prefix with PREFIX=/some/path ./install.sh
set -euo pipefail

APP_NAME="eva-ui"
BIN_SRC="target/release/eva-navigation-unit"

PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
DESKTOP_DIR="$PREFIX/share/applications"
INSTALLED_BIN="$BIN_DIR/$APP_NAME"
DESKTOP_FILE="$DESKTOP_DIR/$APP_NAME.desktop"

# Run from the repository root (this script's directory) so relative paths
# resolve regardless of where the script is invoked from.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Build the release binary if it has not been built yet.
if [[ ! -f "$BIN_SRC" ]]; then
    echo "Release binary not found at $BIN_SRC; building with cargo..."
    cargo build --release
fi

mkdir -p "$BIN_DIR" "$DESKTOP_DIR"

install -m 755 "$BIN_SRC" "$INSTALLED_BIN"
echo "Installed binary       -> $INSTALLED_BIN"

cat > "$DESKTOP_FILE" <<EOF
[Desktop Entry]
Type=Application
Name=EVA UI
Comment=Android Auto head unit
Exec=$INSTALLED_BIN
Terminal=false
Categories=AudioVideo;Utility;
StartupNotify=true
EOF
chmod 644 "$DESKTOP_FILE"
echo "Installed desktop entry -> $DESKTOP_FILE"

# Refresh the desktop database so the launcher appears immediately (best effort).
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$DESKTOP_DIR" >/dev/null 2>&1 || true
fi

echo
echo "Done. Make sure $BIN_DIR is on your PATH."
echo "Config will be read from ~/.config/eva-ui/config.toml."
