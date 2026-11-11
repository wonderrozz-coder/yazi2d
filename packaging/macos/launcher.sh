#!/bin/bash
# Resolve the bundle this script lives in, then start Yazi2D in the best
# terminal available.
#
# Terminal.app does not support truecolor, so images and thumbnails render as
# black blocks there. We therefore prefer terminals that do:
#   Ghostty → kitty → WezTerm → iTerm2 → Terminal.app (last resort).
BUNDLE="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$BUNDLE/Resources/bin/yazi2d"

if [ ! -x "$BIN" ]; then
  osascript -e 'display alert "Yazi2D" message "The application is damaged: the yazi2d binary is missing."'
  exit 1
fi

notify() {
  osascript -e "display notification \"$1\" with title \"Yazi2D\"" >/dev/null 2>&1
}

# --- Ghostty ----------------------------------------------------------------
# `open -na` is the documented way to launch a macOS GUI build with arguments.
if [ -d "/Applications/Ghostty.app" ]; then
  if open -na Ghostty.app --args -e "$BIN" 2>/dev/null; then
    exit 0
  fi
fi

# --- kitty ------------------------------------------------------------------
if [ -d "/Applications/kitty.app" ]; then
  if open -na kitty.app --args "$BIN" 2>/dev/null; then
    exit 0
  fi
fi

# --- WezTerm ----------------------------------------------------------------
if [ -d "/Applications/WezTerm.app" ]; then
  if open -na WezTerm.app --args start -- "$BIN" 2>/dev/null; then
    exit 0
  fi
fi

# --- iTerm2 -----------------------------------------------------------------
if [ -d "/Applications/iTerm.app" ]; then
  osascript <<APPLESCRIPT >/dev/null 2>&1 && exit 0
tell application "iTerm"
  activate
  create window with default profile command "$BIN"
end tell
APPLESCRIPT
fi

# --- Detected truecolor terminal via $TERM_PROGRAM ---------------------------
# When launched from a terminal that already supports truecolor, reuse it by
# asking its app bundle to open a new window.
case "${TERM_PROGRAM:-}" in
  ghostty|Ghostty)
    open -na Ghostty.app --args -e "$BIN" >/dev/null 2>&1 && exit 0 ;;
  WezTerm)
    open -na WezTerm.app --args start -- "$BIN" >/dev/null 2>&1 && exit 0 ;;
esac

# --- Fallback: Terminal.app -------------------------------------------------
# Works everywhere, but images and thumbnails will look flat.
notify "Terminal.app has no truecolor. Install Ghostty for previews."
CMD=$(printf '%q' "$BIN")
ESCAPED=${CMD//\'/\'\\\'\'}
osascript <<APPLESCRIPT >/dev/null 2>&1
tell application "Terminal"
  activate
  do script "$ESCAPED"
end tell
APPLESCRIPT
