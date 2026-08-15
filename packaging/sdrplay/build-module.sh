#!/usr/bin/env bash
# Compiles SoapySDRPlay3 against a staged SDK and drops the module into a Soapy module directory.
#
# The module is MIT and is the only thing that ships: it names the vendor library by soname and
# finds it at runtime wherever the SDRplay installer put it, so an install without the vendor API
# simply has one module that does not load.
set -euo pipefail

soapy_prefix="${1:?SoapySDR prefix}"
sdk="${2:?SDRplay SDK directory}"
destination="${3:?module destination directory}"
architecture="${4:-}"
script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
# shellcheck source=packaging/sdrplay/api.env
. "$script_directory/api.env"

destination="$(python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "$destination")"
test -d "$destination" || { echo "no module directory at $destination" >&2; exit 2; }
library="$(find "$sdk/lib" -name 'libsdrplay_api.so.*' -print -quit)"
test -n "$library" || { echo "no vendor library under $sdk/lib — run fetch-api.sh first" >&2; exit 2; }
test -f "$sdk/include/sdrplay_api.h" || { echo "no headers under $sdk/include" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf -- "$work"' EXIT

git clone --quiet --branch "$MODULE_TAG" --depth 1 \
  https://github.com/pothosware/SoapySDRPlay3.git "$work/source"
commit="$(git -C "$work/source" rev-parse HEAD)"
if [ "$commit" != "$MODULE_COMMIT" ]; then
  echo "$MODULE_TAG resolves to $commit, not the pinned $MODULE_COMMIT" >&2
  exit 1
fi

configure=(
  cmake -S "$work/source" -B "$work/build"
  -DCMAKE_BUILD_TYPE=Release
  -DCMAKE_PREFIX_PATH="$soapy_prefix"
  -DLIBSDRPLAY_INCLUDE_DIRS="$sdk/include"
  -DLIBSDRPLAY_LIBRARIES="$library"
  # The module's floor is CMake 2.8.12, which CMake 4 refuses to be compatible with.
  -DCMAKE_POLICY_VERSION_MINIMUM=3.5
)
if [ -n "$architecture" ]; then configure+=(-DCMAKE_OSX_ARCHITECTURES="$architecture"); fi
"${configure[@]}" > /dev/null
cmake --build "$work/build" --config Release --parallel > /dev/null

built="$(find "$work/build" -name '*sdrPlaySupport*' \( -name '*.so' -o -name '*.dylib' \) -print -quit)"
test -n "$built" || { echo "SoapySDRPlay3 built no module" >&2; exit 1; }
module="$destination/$(basename "$built")"
cp -L "$built" "$module"

if [ "$(uname -s)" = Darwin ]; then
  install_name_tool -id "@rpath/$(basename "$module")" "$module"
  while IFS= read -r dependency; do
    case "$dependency" in "$soapy_prefix"/*)
      install_name_tool -change "$dependency" "@rpath/$(basename "$dependency")" "$module"
    esac
  done < <(otool -L "$module" | awk 'NR > 1 { print $1 }')
  # CMake records the build machine's library directories — the throwaway SDK path among them —
  # and a leftover rpath resolves on this machine and nowhere else, which would let the linkage
  # check pass here and the module fail on every install.
  while IFS= read -r rpath; do
    install_name_tool -delete_rpath "$rpath" "$module"
  done < <(otool -l "$module" | awk '/LC_RPATH/ { found = 1 } found && $1 == "path" { print $2; found = 0 }')
  # Two rpaths, two owners: the bundle's own lib directory two levels up holds SoapySDR, and
  # /usr/local/lib is where every SDRplay macOS installer links libsdrplay_api.so.3.
  install_name_tool -add_rpath "@loader_path/../.." "$module"
  install_name_tool -add_rpath "/usr/local/lib" "$module"
  remaining="$(otool -L "$module" | awk 'NR > 1 { print $1 }' | grep -c "^$soapy_prefix" || true)"
  test "$remaining" -eq 0 || { echo "$module still names the build prefix" >&2; exit 1; }
else
  patchelf --force-rpath --set-rpath '$ORIGIN:$ORIGIN/..:$ORIGIN/../..' "$module"
fi

test -z "$(find "$destination" -name 'libsdrplay_api*' -print -quit)" \
  || { echo "the vendor library must not be staged: $destination" >&2; exit 1; }
echo "SDRplay module: $(basename "$module") in $destination"
