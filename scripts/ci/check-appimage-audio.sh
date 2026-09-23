#!/usr/bin/env bash
set -euo pipefail

image="$(find "$1" -maxdepth 1 -name '*.AppImage' -print -quit)"
test -n "$image" || { echo "no AppImage in $1" >&2; exit 1; }
image="$(readlink -f "$image")"

check="$(mktemp -d)"
trap 'rm -rf "$check"' EXIT
(cd "$check" && "$image" --appimage-extract > /dev/null)
root="$check/squashfs-root"

need() { test -f "$root/$1" || { echo "missing from the AppImage: $1" >&2; exit 1; }; }
need apprun-hooks/linuxdeploy-plugin-gstreamer.sh
need usr/lib/gstreamer-1.0/libgstautodetect.so
need usr/lib/gstreamer1.0/gstreamer-1.0/gst-plugin-scanner
test -f "$root/usr/lib/gstreamer-1.0/libgstpulseaudio.so" \
  || test -f "$root/usr/lib/gstreamer-1.0/libgstalsa.so" \
  || { echo "no audio sink plugin in the AppImage" >&2; exit 1; }
grep -qa apprun-hooks "$root/AppRun" || { echo "AppRun skips the bundled hooks" >&2; exit 1; }
