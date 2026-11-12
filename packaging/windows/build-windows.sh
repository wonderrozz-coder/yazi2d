#!/bin/bash
# Build the Windows binaries for Yazi2D.
#
# Two artefacts come out of one source tree:
#
#   yazi2d-x86_64-windows.exe  x86_64-pc-windows-gnu — Windows 10/11, stable
#                              toolchain, no nightly needed.
#   yazi2d-x86_64-win7.exe     x86_64-win7-windows-gnu — Windows 7/8/8.1.
#                              rustup ships no `std` for the win7 targets, so
#                              this one needs nightly + `-Z build-std`.
#
# Requirements (Arch Linux):
#   sudo pacman -S mingw-w64-gcc          # assembler, linker, import libraries
#   rustup +nightly component add rust-src
#   rustup target add x86_64-pc-windows-gnu
#
# Usage:
#   packaging/windows/build-windows.sh                 # both builds
#   WIN7=0 packaging/windows/build-windows.sh          # modern build only
#
# Environment:
#   CARGO      cargo binary      (default: cargo)
#   TOOLCHAIN  stable toolchain  (default: the default one)
#   OUT        output directory  (default: dist/windows)
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

CARGO="${CARGO:-cargo}"
# Honour an external CARGO_TARGET_DIR: the build artifacts land there, not in
# the source tree, and the copies below must look in the same place.
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
OUT="${OUT:-$ROOT/dist/windows}"
WIN7="${WIN7:-1}"

mkdir -p "$OUT"

say() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[!]\033[0m %s\n' "$*" >&2; }

if command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
  say "MinGW-w64 toolchain: $(command -v x86_64-w64-mingw32-gcc)"
else
  warn "x86_64-w64-mingw32-gcc is missing — install mingw-w64-gcc"
  exit 1
fi

# The win7 target links the `windows` crate's import library
# (`libwindows.0.48.5.a`), which ships inside the crate source rather than in a
# system directory. Every such library found in the cargo registry is offered to
# the linker, so the build survives a dependency bump.
gnu_lib_flags() {
  local dir
  find "${CARGO_HOME:-$HOME/.cargo}/registry/src" -maxdepth 4 \
    -name 'libwindows.*.a' -printf '%h\n' 2>/dev/null |
    sort -u |
    while read -r dir; do
      printf -- "-L\n%s\n" "native=$dir"
    done
}

status=0

say "building Windows 10/11 (x86_64-pc-windows-gnu)"
if "$CARGO" ${TOOLCHAIN:-} build --release --target x86_64-pc-windows-gnu; then
  cp "$TARGET_DIR/x86_64-pc-windows-gnu/release/yazi2d.exe" \
     "$OUT/yazi2d-x86_64-windows.exe"
  say "wrote $OUT/yazi2d-x86_64-windows.exe"
else
  warn "the Windows 10/11 build failed"
  status=1
fi

if [ "$WIN7" = "1" ]; then
  mapfile -t native < <(gnu_lib_flags)
  if [ "${#native[@]}" -eq 0 ]; then
    warn "no libwindows.*.a in the registry; run the Windows 10/11 build first"
    native=("-L" "native=/nonexistent")
  fi
  rustflags="['${native[0]}'"
  for flag in "${native[@]:1}"; do
    rustflags="$rustflags,'$flag'"
  done
  rustflags="$rustflags]"

  say "building Windows 7/8/8.1 (x86_64-win7-windows-gnu)"
  if "$CARGO" +nightly -Z build-std=std,panic_abort \
      build --release --target x86_64-win7-windows-gnu \
      --config "target.x86_64-win7-windows-gnu.rustflags=$rustflags"; then
    cp "$TARGET_DIR/x86_64-win7-windows-gnu/release/yazi2d.exe" \
       "$OUT/yazi2d-x86_64-win7.exe"
    say "wrote $OUT/yazi2d-x86_64-win7.exe"
  else
    warn "the Windows 7 build needs nightly with the rust-src component"
    status=1
  fi
fi

exit "$status"
