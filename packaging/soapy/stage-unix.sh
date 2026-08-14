#!/usr/bin/env bash
set -euo pipefail

prefix="${1:?radioconda prefix}"
destination="${2:?staging destination}"
script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
# Linux only, and only because `ldd` resolves a library's dependencies by loading it. The macOS
# walk reads recorded names out of the file with `otool` and needs no loader path — while
# DYLD_LIBRARY_PATH would apply to every tool this script runs, python3 and otool included,
# handing each of them this environment's libc++ and libiconv in place of the system's. That is
# what killed gtar in the release job (559a669); here it segfaulted python3 on the first line.
if [ "$(uname -s)" != Darwin ]; then
  export LD_LIBRARY_PATH="$prefix/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi
destination="$(python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "$destination")"
working_directory="$(pwd -P)"
case "$destination" in
  /|"") echo "refusing unsafe staging destination: $destination" >&2; exit 2 ;;
esac
case "$working_directory/" in
  "$destination/"*) echo "refusing staging destination at or above the working directory: $destination" >&2; exit 2 ;;
esac

rm -rf -- "$destination"
install -d "$destination/bin" "$destination/lib/SoapySDR/modules0.8" "$destination/licenses"

cores="$(find "$prefix/lib" -maxdepth 1 \( -type f -o -type l \) \
  \( -name 'libSoapySDR.so*' -o -name 'libSoapySDR*.dylib' \) | sort)"
test -n "$cores" || { echo "SoapySDR core not found under $prefix/lib" >&2; exit 1; }
while IFS= read -r core; do cp -L "$core" "$destination/lib/$(basename "$core")"; done <<< "$cores"

module_dir="$(find "$prefix/lib" -type d -path '*/SoapySDR/modules0.8' | head -1)"
test -n "$module_dir" || { echo "SoapySDR modules0.8 not found under $prefix/lib" >&2; exit 1; }
find "$module_dir" -maxdepth 1 -type f \
  | grep -Ei '/(lib)?(rtlsdr|hackrf|airspyhf|airspy|bladerf|lms7|plutosdr|remote).*\.(so|dylib)' \
  | while IFS= read -r module; do cp -L "$module" "$destination/lib/SoapySDR/modules0.8/"; done

test -n "$(find "$destination/lib/SoapySDR/modules0.8" -iname '*rtlsdr*')" \
  || { echo "SoapyRTLSDR was not staged" >&2; exit 1; }
test -n "$(find "$destination/lib/SoapySDR/modules0.8" -iname '*hackrf*')" \
  || { echo "SoapyHackRF was not staged" >&2; exit 1; }

copy_dependencies_linux() {
  local changed=1
  while [ "$changed" -eq 1 ]; do
    changed=0
    while IFS= read -r dependency; do
      case "$dependency" in "$prefix"/*)
        target="$destination/lib/$(basename "$dependency")"
        if [ ! -e "$target" ]; then cp -L "$dependency" "$target"; changed=1; fi
      esac
    done < <(find "$destination/lib" -type f -print0 | xargs -0 -r ldd 2>/dev/null \
      | awk '/=> \// { print $3 }' | sort -u)
  done
  find "$destination/lib" -type f -exec patchelf --force-rpath \
    --set-rpath '$ORIGIN:$ORIGIN/..:$ORIGIN/../..' {} +
}

copy_dependencies_macos() {
  local changed=1
  while [ "$changed" -eq 1 ]; do
    changed=0
    while IFS= read -r dependency; do
      source=""
      case "$dependency" in
        "$prefix"/*) source="$dependency" ;;
        @rpath/*|@loader_path/*|@executable_path/*)
          # `-type l` as well as `-type f`: a library's install name is usually its soname, and
          # conda ships that as a symlink onto the fully versioned file — `librtlsdr.0.dylib`
          # onto `librtlsdr.0.6.0.dylib`. Matching files alone silently staged no driver library
          # at all for rtlsdr, hackrf, airspy, LimeSuite or Pluto, and none of them missed a
          # thing until a radio was plugged in. `cp -L` below copies the content under the name
          # the module asks for.
          source="$(find "$prefix/lib" \( -type f -o -type l \) \
            -name "$(basename "$dependency")" -print -quit)"
          ;;
      esac
      if [ -n "$source" ]; then
        target="$destination/lib/$(basename "$source")"
        if [ ! -e "$target" ]; then cp -L "$source" "$target"; changed=1; fi
      fi
    done < <(find "$destination/lib" -type f -print0 | xargs -0 otool -L \
      | awk 'NR > 1 { print $1 }' | sort -u)
  done
  while IFS= read -r binary; do
    install_name_tool -id "@rpath/$(basename "$binary")" "$binary" 2>/dev/null || true
    while IFS= read -r dependency; do
      case "$dependency" in "$prefix"/*)
        install_name_tool -change "$dependency" "@rpath/$(basename "$dependency")" "$binary"
      esac
    done < <(otool -L "$binary" | awk 'NR > 1 { print $1 }')
    case "$binary" in
      "$destination/lib/SoapySDR/modules0.8/"*) relative_rpath="@loader_path/../.." ;;
      *) relative_rpath="@loader_path" ;;
    esac
    install_name_tool -add_rpath "$relative_rpath" "$binary" 2>/dev/null || true
  done < <(find "$destination/lib" -type f)
}

if [ "$(uname -s)" = Darwin ]; then copy_dependencies_macos; else copy_dependencies_linux; fi

if [ -d "$prefix/conda-meta" ]; then
  cp "$prefix/conda-meta/"*.json "$destination/licenses/" 2>/dev/null || true
fi
for license_root in "$prefix/share/licenses" "$prefix/Library/share/licenses"; do
  if [ -d "$license_root" ]; then
    install -d "$destination/licenses/texts"
    cp -R "$license_root/." "$destination/licenses/texts/"
  fi
done
install -d "$destination/licenses/texts"
cp "$script_directory/licenses/"*.txt "$destination/licenses/texts/"
find "$prefix" -maxdepth 4 -type f \( -iname 'license*' -o -iname 'copying*' \) \
  | head -200 | while IFS= read -r license; do
      cp "$license" "$destination/licenses/$(basename "$(dirname "$license")")-$(basename "$license")"
    done
