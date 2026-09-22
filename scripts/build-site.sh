#!/usr/bin/env sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
out=${1:-$root/dist/site}

mdbook build "$root/docs"
pnpm --dir "$root/site" build

rm -rf "$out"
mkdir -p "$out"
cp -R "$root/docs/book/." "$out/"
cp -R "$root/site/dist/." "$out/"
printf 'sdrmm.newspicel.dev\n' > "$out/CNAME"

echo "site assembled in $out"
