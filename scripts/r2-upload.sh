#!/usr/bin/env sh
set -eu

if [ "$#" -lt 2 ]; then
  echo "usage: $0 <prefix> <file>..." >&2
  echo "  uploads to https://downloads.sdrmm.com/<prefix>/<file name>" >&2
  exit 2
fi

bucket=${R2_BUCKET:-sdrmm}
cache=${CACHE_CONTROL:-public, max-age=31536000, immutable}
wrangler=${WRANGLER:-wrangler}
prefix=${1%/}
shift

for file in "$@"; do
  name=$(basename -- "$file")
  case "$name" in
    *.json) type=application/json ;;
    *.txt | *.asc | *.repo | SHA256SUMS | latest | Packages | Release | InRelease) type="text/plain; charset=utf-8" ;;
    *.xml) type=application/xml ;;
    *) type=application/octet-stream ;;
  esac
  echo "$prefix/$name"
  $wrangler r2 object put "$bucket/$prefix/$name" \
    --remote \
    --file "$file" \
    --content-type "$type" \
    --cache-control "$cache"
done
