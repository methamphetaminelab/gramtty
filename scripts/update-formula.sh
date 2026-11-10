#!/bin/sh
set -eu

repo="methamphetaminelab/gramtty"
[ "$#" -ge 1 ] || { echo "usage: $0 <version> [formula]" >&2; exit 1; }
version="${1#v}"
formula="${2:-$(dirname "$0")/../Formula/gramtty.rb}"
[ -f "$formula" ] || { echo "error: formula not found: $formula" >&2; exit 1; }

sums=""
for asset in aarch64-macos x86_64-macos x86_64-linux; do
    url="https://github.com/$repo/releases/download/v$version/gramtty-$version-$asset.tar.gz.sha256"
    body="$(curl -fsSL "$url")" || { echo "error: cannot fetch $url" >&2; exit 1; }
    sum="${body%% *}"
    printf '%s' "$sum" | grep -Eq '^[0-9a-f]{64}$' || { echo "error: bad checksum in $url" >&2; exit 1; }
    sums="$sums $asset=$sum"
done

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
awk -v version="$version" -v sums="$sums" '
    BEGIN { n = split(sums, pairs, " "); for (i = 1; i <= n; i++) { split(pairs[i], kv, "="); sum[kv[1]] = kv[2] } }
    /^  version "/ { sub(/"[^"]*"/, "\"" version "\"") }
    /url "/ { for (a in sum) if (index($0, "-" a ".tar.gz")) cur = a }
    /sha256 "/ && cur != "" { sub(/"[^"]*"/, "\"" sum[cur] "\""); done[cur] = 1; cur = "" }
    { print }
    END { for (a in sum) if (!(a in done)) { print "error: no sha256 line for " a > "/dev/stderr"; exit 1 } }
' "$formula" > "$tmp"
cat "$tmp" > "$formula"
echo "updated $formula to $version"
