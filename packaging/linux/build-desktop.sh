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
# Icon themes look the picture up by size directory: a menu asking for 32x32 or
# 48x48 does not find a 1024x1024-only icon, and LXQt then shows a blank or
# generic square. ImageMagick is not installed everywhere (it is not on a stock
# Arch + LXQt), so when it is missing the source is copied into *every* size
# directory instead. The file is simply larger than the directory claims; every
# toolkit scales on load, and the menu entry resolves either way.
ICON_DIR="$DIST/icons"
ICON_SIZES="16 24 32 48 64 128 256 512"
if [ -f "$ICON_SRC" ]; then
  resize_icon() {
    local size="$1" out="$2"
    if command -v magick >/dev/null 2>&1; then
      magick "$ICON_SRC" -resize "${size}x${size}" "$out"
    elif command -v convert >/dev/null 2>&1; then
      convert "$ICON_SRC" -resize "${size}x${size}" "$out"
    else
      cp "$ICON_SRC" "$out"
    fi
  }
  for size in $ICON_SIZES; do
    mkdir -p "$ICON_DIR/hicolor/${size}x${size}/apps"
    resize_icon "$size" "$ICON_DIR/hicolor/${size}x${size}/apps/$BIN_NAME.png"
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
# `Exec` is deliberately not bare `yazi2d`: the graphical session's PATH is
# /usr/local/sbin:/usr/local/bin:/usr/bin:..., which does not include
# ~/.local/bin on most distributions, so the menu entry would exit instantly
# with no window and no message. `sh -lc` prepends the install directory, and
# `\$PATH` is escaped so the *target* shell expands it, not this script.
# (This note must stay outside the heredoc: a `#` line inside it becomes part
# of the desktop file, which is how it broke the first time.)
cat > "$DIST/applications/$BIN_NAME.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=$APP_NAME
GenericName=File Manager
Comment=Dual-pane terminal file manager
Exec=sh -lc "PATH=\\"\$HOME/.local/bin:\$PATH\\" exec $BIN_NAME %F"
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

# --- PATH -------------------------------------------------------------------
# `~/.local/bin` is not on PATH on a stock Arch install (or most distributions),
# so typing `yazi2d` in a terminal would fail even though the file is installed.
# A warning is not enough: add it to the profile the login shell actually reads.
# Only ~/.profile is touched, and only once — it is sourced by bash, zsh and
# most display managers.
ensure_path_entry() {
  local profile="$HOME/.profile"
  local marker="# added by Yazi2D setup"
  if [ -f "$profile" ] && grep -qF "$marker" "$profile"; then
    return 0
  fi
  {
    echo
    echo "$marker"
    echo 'case ":$PATH:" in'
    echo '  *":$HOME/.local/bin:"*) ;;'
    echo '  *) export PATH="$HOME/.local/bin:$PATH" ;;'
    echo 'esac'
  } >> "$profile"
  echo "Added ~/.local/bin to PATH in $profile"
}

ensure_path_entry

echo
echo "Installed: $HOME/.local/bin/$BIN_NAME"
echo "Menu entry: $DATA/applications/$BIN_NAME.desktop"
echo
echo "Open a new terminal (or run 'source ~/.profile') and type: $BIN_NAME"
echo "It is also in the application menu as \"$APP_NAME\"."
