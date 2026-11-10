#!/bin/sh
# usage: scripts/update-aur.sh <version> <dir containing PKGBUILD>
set -eu

[ "$#" -eq 2 ] || { echo "usage: $0 <version> <dir>" >&2; exit 1; }
version="${1#v}"
dir="$2"
printf '%s' "$version" | grep -Eq '^[0-9][0-9A-Za-z._+]*$' || { echo "error: bad version: $version" >&2; exit 1; }
[ -f "$dir/PKGBUILD" ] || { echo "error: PKGBUILD not found in $dir" >&2; exit 1; }
for tool in updpkgsums makepkg; do
    command -v "$tool" >/dev/null 2>&1 || { echo "error: $tool not found (install pacman-contrib and base-devel)" >&2; exit 1; }
done

cd "$dir"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

sed -e "s/^pkgver=.*/pkgver=$version/" -e 's/^pkgrel=.*/pkgrel=1/' PKGBUILD > "$tmp/PKGBUILD"
grep -qx "pkgver=$version" "$tmp/PKGBUILD" || { echo "error: no pkgver= line in PKGBUILD" >&2; exit 1; }
grep -qx "pkgrel=1" "$tmp/PKGBUILD" || { echo "error: no pkgrel= line in PKGBUILD" >&2; exit 1; }
cat "$tmp/PKGBUILD" > PKGBUILD

mkdir "$tmp/src"
SRCDEST="$tmp/src" updpkgsums
makepkg --printsrcinfo > "$tmp/SRCINFO"
cat "$tmp/SRCINFO" > .SRCINFO
echo "updated $dir to $version"
