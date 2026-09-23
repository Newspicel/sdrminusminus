#!/usr/bin/env bash
set -euo pipefail

arch="$1"
tools="${XDG_CACHE_HOME:-$HOME/.cache}/tauri"
mkdir -p "$tools"

fetch() {
  if ! curl --fail --location --silent --show-error \
    --retry 5 --retry-delay 5 --retry-all-errors --connect-timeout 30 \
    --output "$tools/$2.part" "$1" || [ ! -s "$tools/$2.part" ]; then
    rm -f "$tools/$2.part"
    return 1
  fi
  chmod +x "$tools/$2.part"
  mv "$tools/$2.part" "$tools/$2"
}

raw=https://raw.githubusercontent.com/tauri-apps
linuxdeploy=https://github.com/linuxdeploy

fetch "https://github.com/tauri-apps/binary-releases/releases/download/apprun-old/AppRun-$arch" "AppRun-$arch"
fetch "$linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-$arch.AppImage" "linuxdeploy-$arch.AppImage"
fetch "$raw/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh" linuxdeploy-plugin-gtk.sh
fetch "$raw/linuxdeploy-plugin-gstreamer/master/linuxdeploy-plugin-gstreamer.sh" linuxdeploy-plugin-gstreamer.sh
fetch "$linuxdeploy/linuxdeploy-plugin-appimage/releases/download/continuous/linuxdeploy-plugin-appimage-$arch.AppImage" \
  linuxdeploy-plugin-appimage.AppImage || echo "AppImage plugin unavailable, using the bundled one"
ls -l "$tools"
