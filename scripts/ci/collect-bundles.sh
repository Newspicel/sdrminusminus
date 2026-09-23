#!/usr/bin/env bash
set -euo pipefail
shopt -s nullglob

target="$1"
version="$2"
mkdir -p bundles
find "target/$target/release/bundle" -maxdepth 2 -type f \
  \( -name '*.dmg' -o -name '*.deb' -o -name '*.AppImage' \
     -o -name '*.msi' -o -name '*-setup.exe' \
     -o -name '*.app.tar.gz' -o -name '*.sig' \) \
  -exec cp {} bundles/ \;

arch="${target%%-*}"
for f in bundles/*.app.tar.gz; do
  mv "$f" "${f%.app.tar.gz}_${version}_${arch}.app.tar.gz"
  mv "$f.sig" "${f%.app.tar.gz}_${version}_${arch}.app.tar.gz.sig"
done

test -n "$(find bundles -name '*.sig')" || { echo "no .sig files, is TAURI_SIGNING_PRIVATE_KEY set?" >&2; exit 1; }
ls -l bundles
