#!/usr/bin/env sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
out=${1:-$root/dist/site}

mdbook build "$root/docs"

rm -rf "$out"
mkdir -p "$out/screens"
cp -R "$root/docs/book/." "$out/"
cp "$root/site/index.html" "$out/index.html"
cp "$root/assets/icon.svg" "$out/icon.svg"
cp "$root"/assets/screenshots/*.png "$out/screens/"
printf 'sdrmm.newspicel.dev\n' > "$out/CNAME"

echo "site assembled in $out"
