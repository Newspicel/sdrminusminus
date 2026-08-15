#!/usr/bin/env bash
# Unpacks the SDRplay API SDK into a build-only directory: headers and the vendor library the
# SoapySDRPlay3 module links against. Nothing this script writes is ever staged into an artifact.
set -euo pipefail

destination="${1:?SDK destination}"
script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
# shellcheck source=packaging/sdrplay/api.env
. "$script_directory/api.env"

destination="$(python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "$destination")"
working_directory="$(pwd -P)"
case "$destination" in
  /|"") echo "refusing unsafe SDK destination: $destination" >&2; exit 2 ;;
esac
case "$working_directory/" in
  "$destination/"*) echo "refusing SDK destination at or above the working directory: $destination" >&2; exit 2 ;;
esac

case "$(uname -s)" in
  Darwin) url="$MACOS_URL"; file="$MACOS_FILE"; expected="$MACOS_SHA256" ;;
  Linux) url="$LINUX_URL"; file="$LINUX_FILE"; expected="$LINUX_SHA256" ;;
  *) echo "unsupported platform for the SDRplay SDK: $(uname -s)" >&2; exit 2 ;;
esac

rm -rf -- "$destination"
install -d "$destination/include" "$destination/lib"
work="$destination/.work"
install -d "$work"

echo "Fetching the SDRplay API $API_VERSION SDK ($file)."
echo "Use of it is governed by SDRplay's end user licence agreement; it is a build input only."
curl --fail --location --silent --show-error --retry 5 --retry-delay 5 --retry-all-errors \
  --connect-timeout 30 --output "$work/$file" "$url"

if command -v sha256sum > /dev/null; then
  digest="$(sha256sum "$work/$file" | cut -d' ' -f1)"
else
  digest="$(shasum -a 256 "$work/$file" | cut -d' ' -f1)"
fi
if [ "$digest" != "$expected" ]; then
  echo "$file does not match the pinned digest." >&2
  echo "  expected $expected" >&2
  echo "  received $digest" >&2
  echo "SDRplay has published a new API. Re-pin packaging/sdrplay/api.env after checking the" >&2
  echo "new version against the module requirements in docs/src/hardware.md." >&2
  exit 1
fi

if [ "$(uname -s)" = Darwin ]; then
  (cd "$work" && xar -xf "$file")
  (cd "$work" && gunzip -dc SDRplayAPI.pkg/Payload | cpio -idm 2> /dev/null)
  # The .pkg installs under a versioned prefix and symlinks the result into /usr/local; the
  # payload keeps the prefix, so the library is found by name rather than at a fixed path.
  library="$(find "$work/Library" -name 'libsdrplay_api.so.*' -print -quit)"
  headers="$(find "$work/Library" -type d -name include -print -quit)"
else
  # Makeself archive: --noexec unpacks without running the vendor installer, which would
  # otherwise register a system service on the build machine.
  sh "$work/$file" --noexec --keep --nox11 --target "$work/payload" > /dev/null
  case "$(uname -m)" in
    x86_64 | amd64) architecture=amd64 ;;
    aarch64 | arm64) architecture=arm64 ;;
    armv7l | armhf) architecture=armhf ;;
    *) echo "no SDRplay API build for $(uname -m)" >&2; exit 2 ;;
  esac
  library="$(find "$work/payload/$architecture" -name 'libsdrplay_api.so.*' -print -quit)"
  headers="$work/payload/inc"
fi

test -n "$library" || { echo "no libsdrplay_api in $file" >&2; exit 1; }
test -n "$headers" || { echo "no headers in $file" >&2; exit 1; }
cp -L "$library" "$destination/lib/"
cp -L "$headers"/sdrplay_api*.h "$destination/include/"
test -f "$destination/include/sdrplay_api.h" || { echo "no sdrplay_api.h in $file" >&2; exit 1; }
rm -rf -- "$work"

echo "SDRplay SDK: $(basename "$library") and $(find "$destination/include" -name '*.h' | wc -l | tr -d ' ') headers in $destination"
