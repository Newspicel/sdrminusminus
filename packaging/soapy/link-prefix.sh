#!/usr/bin/env bash
# A prefix to *link* against, holding SoapySDR and nothing else.
#
# `soapysdr-sys` hands whatever prefix it is given to rustc as a link search path, and the
# linker then searches that directory for every `-l` on the line — not only `-lSoapySDR`.
# Pointed at the whole pinned environment it shadows the platform's own libraries with conda's:
# `libc` declares `#[link(name = "iconv")]` on Apple targets, so 0.1.2 bound to conda's
# libiconv, whose install name is `@rpath/libiconv.2.dylib`, instead of the SDK's absolute
# `/usr/lib/libiconv.2.dylib`. Nothing carries that library into the bundle, and every macOS
# install died at launch with "Library not loaded". libz, libc++, libedit and libcurl are all
# in the same environment under names the toolchain asks for.
#
# So the environment is used for what it is pinned for and for nothing else. Both probes
# `soapysdr-sys` runs — `SOAPY_SDR_ROOT` first, pkg-config second — land here, which is why the
# `.pc` file is rewritten rather than left pointing back at the environment it came from.
set -euo pipefail

prefix="${1:?radioconda prefix}"
destination="${2:?link prefix destination}"
destination="$(python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "$destination")"
working_directory="$(pwd -P)"
case "$destination" in
  /|"") echo "refusing unsafe link prefix destination: $destination" >&2; exit 2 ;;
esac
case "$working_directory/" in
  "$destination/") echo "refusing link prefix destination at the working directory" >&2; exit 2 ;;
esac

if [ "$(uname -s)" = Darwin ]; then suffix=dylib; else suffix=so; fi

rm -rf -- "$destination"
install -d "$destination/lib/pkgconfig"
ln -s "$prefix/include" "$destination/include"

cores="$(find "$prefix/lib" -maxdepth 1 \( -type f -o -type l \) \
  \( -name "libSoapySDR.$suffix*" -o -name "libSoapySDR.*.$suffix" \) | sort)"
test -n "$cores" || { echo "SoapySDR core not found under $prefix/lib" >&2; exit 1; }
while IFS= read -r core; do cp -a "$core" "$destination/lib/"; done <<< "$cores"

# `probe_env_var` looks for the unversioned name and nothing else, and a runtime-only conda
# package need not ship that development symlink. Linking it to the newest version it did ship
# keeps the first probe from falling through to pkg-config for a reason that has nothing to do
# with SoapySDR being absent.
bare="$destination/lib/libSoapySDR.$suffix"
if [ ! -e "$bare" ]; then
  newest="$(find "$destination/lib" -maxdepth 1 -name "libSoapySDR*" | sort | tail -1)"
  ln -s "$(basename "$newest")" "$bare"
fi

# Every path in the file, not just `prefix=`: a `.pc` is free to spell its `-L` out in full, and
# one absolute path left in it puts the whole environment back on the link line.
pc="$prefix/lib/pkgconfig/SoapySDR.pc"
if [ -f "$pc" ]; then
  sed "s|$prefix|$destination|g" "$pc" > "$destination/lib/pkgconfig/SoapySDR.pc"
fi

test -e "$bare" || { echo "no libSoapySDR.$suffix in $destination/lib" >&2; exit 1; }
echo "link prefix: $(find "$destination/lib" -maxdepth 1 -mindepth 1 | wc -l | tr -d ' ') entries in $destination/lib"
