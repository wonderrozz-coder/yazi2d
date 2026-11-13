#!/bin/bash
#
# Install Yazi2D on Linux: the release binary into ~/.local/bin, plus an icon
# entry in the desktop application menu (that is the Linux equivalent of the
# macOS .app bundle).
#
#   ./packaging/linux/build-desktop.sh              # build + install for $USER
#   ./packaging/linux/build-desktop.sh --no-install  # build only, leave dist/
#
# Unlike macOS there is no launcher: the desktop entry sets `Terminal=true`, so
# the desktop opens the user's own terminal emulator, which is already
# configured for truecolor (Ghostty, kitty, WezTerm, foot).

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
APP_NAME="Yazi2D"
BIN_NAME="yazi2d"
ICON_SRC="$ROOT/packaging/icon-source.png"
DIST="$ROOT/dist/linux"
INSTALL=1

for arg in "$@"; do
  case "$arg" in
    --no-install) INSTALL=0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

echo "==> Building release binary"
cd "$ROOT"
source "$HOME/.cargo/env" 2>/dev/null || true
cargo build --release

# The binary may live outside the repo when CARGO_TARGET_DIR is set.
TARGET_DIR="$(cargo metadata --no-deps --format-version 1 2>/dev/null \
  | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
if [ -z "$TARGET_DIR" ]; then
  TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
fi
BINARY="$TARGET_DIR/release/$BIN_NAME"
if [ ! -x "$BINARY" ]; then
  echo "release binary not found at $BINARY" >&2
  exit 1
fi

echo "==> Staging $DIST"
rm -rf "$DIST"
mkdir -p "$DIST"
cp "$BINARY" "$DIST/$BIN_NAME"

# --- Icon -------------------------------------------------------------------
# Icon themes want one image per size directory. `convert` (ImageMagick) is not
# always installed, so a missing tool degrades to copying the source into the
# largest size and into pixmaps, which every desktop environment also searches.
ICON_DIR="$DIST/icons"
if [ -f "$ICON_SRC" ]; then
  for size in 128 256 512; do
    mkdir -p "$ICON_DIR/hicolor/${size}x${size}/apps"
    if command -v magick >/dev/null 2>&1; then
      magick "$ICON_SRC" -resize "${size}x${size}" \
        "$ICON_DIR/hicolor/${size}x${size}/apps/$BIN_NAME.png"
    elif command -v convert >/dev/null 2>&1; then
      convert "$ICON_SRC" -resize "${size}x${size}" \
        "$ICON_DIR/hicolor/${size}x${size}/apps/$BIN_NAME.png"
    fi
  done
  mkdir -p "$ICON_DIR/hicolor/1024x1024/apps" "$ICON_DIR/pixmaps"
  cp "$ICON_SRC" "$ICON_DIR/hicolor/1024x1024/apps/$BIN_NAME.png"
  cp "$ICON_SRC" "$ICON_DIR/pixmaps/$BIN_NAME.png"
else
  echo "!! $ICON_SRC not found — installing without an icon"
fi

# --- Desktop entry ----------------------------------------------------------
# `Terminal=true` is what makes this the Linux counterpart of the macOS
# launcher: the desktop opens the terminal emulator of the user's choice.
mkdir -p "$DIST/applications"
cat > "$DIST/applications/$BIN_NAME.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=$APP_NAME
GenericName=File Manager
Comment=Dual-pane terminal file manager
Exec=$BIN_NAME %F
Icon=$BIN_NAME
Terminal=true
Categories=Utility;FileManager;System;
Keywords=file;manager;tui;terminal;dual;panel;commander;
StartupNotify=false
DESKTOP

if [ "$INSTALL" -eq 0 ]; then
  echo
  echo "Built: $DIST"
  exit 0
fi

echo "==> Installing for $USER"
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications"
install -m 755 "$DIST/$BIN_NAME" "$HOME/.local/bin/$BIN_NAME"

DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
mkdir -p "$DATA/icons" "$DATA/applications"
if [ -d "$ICON_DIR" ]; then
  cp -R "$ICON_DIR/." "$DATA/icons/"
fi
install -m 644 "$DIST/applications/$BIN_NAME.desktop" \
  "$DATA/applications/$BIN_NAME.desktop"

# Refresh the desktop database when the tools exist; desktops also pick the
# entry up on their own after a moment.
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$DATA/applications" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t "$DATA/icons/hicolor" >/dev/null 2>&1 || true
fi

echo
echo "Installed: $HOME/.local/bin/$BIN_NAME"
echo "Menu entry: $DATA/applications/$BIN_NAME.desktop"
case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) echo
     echo "!! $HOME/.local/bin is not on PATH. Add this to your shell profile:"
     echo "     export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac
echo
echo "Start it from the application menu, or type: $BIN_NAME"
