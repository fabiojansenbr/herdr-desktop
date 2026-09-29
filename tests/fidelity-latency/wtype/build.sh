#!/usr/bin/env bash
# Private instrumented wtype 0.4 for the spec 007 latency measurement.
# Output only under $OUT (default <worktree>/.local/native-latency); never touches /usr/bin/wtype.
# Usage: tests/fidelity-latency/wtype/build.sh [--offline]   (--offline reuses the pinned tarball)
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
OUT="${OUT:-$ROOT/.local/native-latency}"
# shellcheck disable=SC1091
source <(grep -E '^(version|commit|url|tarball_sha256)=' "$HERE/SOURCE")
TARBALL="$OUT/wtype-$version.tar.gz"
BUILD="$OUT/build/wtype-$version"
BIN="$OUT/bin/wtype-hd-latency"
mkdir -p "$OUT/bin" "$OUT/build"
if [[ ! -f "$TARBALL" ]]; then
  [[ "${1:-}" == "--offline" ]] && { echo "missing $TARBALL" >&2; exit 2; }
  curl -fsSL -o "$TARBALL.part" "$url"
  mv "$TARBALL.part" "$TARBALL"
fi
echo "$tarball_sha256  $TARBALL" | sha256sum -c --strict -
rm -rf "$BUILD"
tar xzf "$TARBALL" -C "$OUT/build"
cp "$HERE/hd_latency.h" "$BUILD/hd_latency.h"
patch --forward --fuzz=0 -d "$BUILD" -p1 < "$HERE/instrumentation.patch"
P="$BUILD/protocol/virtual-keyboard-unstable-v1.xml"
wayland-scanner client-header "$P" "$BUILD/virtual-keyboard-unstable-v1-client-protocol.h"
wayland-scanner private-code "$P" "$BUILD/virtual-keyboard-unstable-v1-protocol.c"
# shellcheck disable=SC2046
cc -std=gnu11 -O2 -Wall -DVERSION="\"$version (hd-latency $commit)\"" -I"$BUILD" \
  "$BUILD/main.c" "$BUILD/virtual-keyboard-unstable-v1-protocol.c" \
  $(pkg-config --cflags --libs wayland-client xkbcommon) -lrt -o "$BIN.part"
mv "$BIN.part" "$BIN"
{
  echo "commit=$commit"
  echo "tarball_sha256=$tarball_sha256"
  echo "patch_sha256=$(sha256sum "$HERE/instrumentation.patch" | cut -d' ' -f1)"
  echo "header_sha256=$(sha256sum "$HERE/hd_latency.h" | cut -d' ' -f1)"
  echo "binary_sha256=$(sha256sum "$BIN" | cut -d' ' -f1)"
  echo "cc=$(cc --version | head -1)"
  echo "wayland_client=$(pkg-config --modversion wayland-client)"
  echo "xkbcommon=$(pkg-config --modversion xkbcommon)"
  echo "wayland_scanner=$(wayland-scanner --version 2>&1)"
} > "$OUT/manifest.txt"
cat "$OUT/manifest.txt"
