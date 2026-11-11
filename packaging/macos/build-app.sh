#!/bin/bash
#
# Build a macOS .app bundle for Yazi2D and install it into /Applications.
#
#   ./packaging/macos/build-app.sh            # build + install to /Applications
#   ./packaging/macos/build-app.sh --no-copy  # build only, leave the .app in dist/
#
# Yazi2D is a terminal UI, so the bundle does not run the binary directly: the
# executable is a tiny launcher that asks Terminal.app to run it. Double-clicking
# the icon therefore opens a Terminal window with Yazi2D already running.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
APP_NAME="Yazi2D"
BUNDLE_ID="dev.yazi2d.app"
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)"
DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"
ICON_SRC="$ROOT/packaging/icon-source.png"
COPY_TO_APPLICATIONS=1

for arg in "$@"; do
  case "$arg" in
    --no-copy) COPY_TO_APPLICATIONS=0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

echo "==> Building release binary"
cd "$ROOT"
source "$HOME/.cargo/env" 2>/dev/null || true
cargo build --release

BINARY="$ROOT/target/release/yazi2d"
if [ ! -x "$BINARY" ]; then
  echo "release binary not found at $BINARY" >&2
  exit 1
fi

echo "==> Creating bundle at $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/bin"

# The real binary lives in Resources so it survives `codesign --deep` intact.
cp "$BINARY" "$APP/Contents/Resources/bin/yazi2d"

# --- Launcher ---------------------------------------------------------------
# Runs in Terminal.app. The `open` trick gives the new window a PTY, which is
# what a curses-style program needs.
cat > "$APP/Contents/MacOS/$APP_NAME" <<'LAUNCHER'
#!/bin/bash
# Resolve the bundle this script lives in, then start Yazi2D in Terminal.
BUNDLE="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$BUNDLE/Resources/bin/yazi2d"

if [ ! -x "$BIN" ]; then
  osascript -e 'display alert "Yazi2D" message "The application is damaged: the yazi2d binary is missing."'
  exit 1
fi

# Command to run inside the new terminal window. Single quotes are escaped so
# paths with spaces or quotes survive the AppleScript round trip.
CMD=$(printf '%q' "$BIN")
ESCAPED=${CMD//\'/\'\\\'\'}
osascript <<APPLESCRIPT >/dev/null 2>&1
tell application "Terminal"
  activate
  do script "$ESCAPED"
end tell
APPLESCRIPT
LAUNCHER
chmod +x "$APP/Contents/MacOS/$APP_NAME"

# --- Icon -------------------------------------------------------------------
if [ -f "$ICON_SRC" ]; then
  echo "==> Generating .icns from $(basename "$ICON_SRC")"
  ICONSET="$DIST/$APP_NAME.iconset"
  rm -rf "$ICONSET"
  mkdir -p "$ICONSET"

  # macOS wants these exact sizes; the @2x variants serve Retina displays.
  for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$ICON_SRC" \
      --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$ICON_SRC" \
      --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
  done

  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/$APP_NAME.icns"
  rm -rf "$ICONSET"
else
  echo "!! $ICON_SRC not found — bundling without an icon"
fi

# --- Info.plist -------------------------------------------------------------
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>$APP_NAME</string>
  <key>CFBundleDisplayName</key>
  <string>$APP_NAME</string>
  <key>CFBundleIdentifier</key>
  <string>$BUNDLE_ID</string>
  <key>CFBundleVersion</key>
  <string>$VERSION</string>
  <key>CFBundleShortVersionString</key>
  <string>$VERSION</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleExecutable</key>
  <string>$APP_NAME</string>
  <key>CFBundleIconFile</key>
  <string>$APP_NAME</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <!-- The launcher hands off to Terminal, so it needs no window of its own. -->
  <key>LSUIElement</key>
  <true/>
</dict>
</plist>
PLIST

# Ad-hoc sign so Gatekeeper accepts a locally built bundle.
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || \
  echo "!! ad-hoc code signing failed (the app still runs locally)"
xattr -cr "$APP" 2>/dev/null || true

if [ "$COPY_TO_APPLICATIONS" -eq 1 ]; then
  echo "==> Installing to /Applications"
  rm -rf "/Applications/$APP_NAME.app"
  cp -R "$APP" "/Applications/"
  # Nudge LaunchServices so Finder and Launchpad pick up the new icon.
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
    -f "/Applications/$APP_NAME.app" >/dev/null 2>&1 || true
  echo
  echo "Installed: /Applications/$APP_NAME.app"
else
  echo
  echo "Built: $APP"
fi

echo
echo "Double-click the icon to open Yazi2D in a Terminal window."
echo "From a shell, 'yazi2d' works as before."
