#!/usr/bin/env sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
out=${1:-$root/dist/site}
mdbook_version=0.5.4

if ! command -v mdbook >/dev/null 2>&1; then
  case "$(uname -sm)" in
    "Linux x86_64") ;;
    *)
      echo "mdbook $mdbook_version is missing: cargo install mdbook --version $mdbook_version" >&2
      exit 1
      ;;
  esac
  tools="$root/target/tools"
  mkdir -p "$tools"
  curl -fsSL "https://github.com/rust-lang/mdBook/releases/download/v$mdbook_version/mdbook-v$mdbook_version-x86_64-unknown-linux-gnu.tar.gz" |
    tar -xz -C "$tools"
  PATH="$tools:$PATH"
fi

pnpm --dir "$root/web" install --frozen-lockfile
pnpm --dir "$root/site" install --frozen-lockfile

mdbook build "$root/docs"
pnpm --dir "$root/site" build

rm -rf "$out"
mkdir -p "$out/docs"
cp -R "$root/docs/book/." "$out/docs/"
node "$root/site/scripts/clean-docs.mjs" "$out/docs"
cp -R "$root/site/dist/." "$out/"

echo "site assembled in $out"
