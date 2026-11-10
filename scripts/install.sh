#!/bin/sh
# Usage: curl -fsSL https://raw.githubusercontent.com/methamphetaminelab/gramtty/main/scripts/install.sh | sh
set -eu

repo="methamphetaminelab/gramtty"
install_dir="${GRAMTTY_INSTALL_DIR:-$HOME/.local/bin}"

die() {
    printf 'gramtty install: error: %s\n' "$1" >&2
    exit 1
}

warn() {
    printf 'gramtty install: warning: %s\n' "$1" >&2
}

command -v curl >/dev/null 2>&1 || die "curl is required"
command -v tar >/dev/null 2>&1 || die "tar is required"

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
    Linux) os="linux" ;;
    Darwin) os="macos" ;;
    *) die "unsupported OS: $os (supported: Linux, macOS)" ;;
esac
case "$arch" in
    x86_64 | amd64) arch="x86_64" ;;
    arm64 | aarch64) arch="aarch64" ;;
    *) die "unsupported architecture: $arch" ;;
esac
if [ "$os" = "linux" ] && [ "$arch" = "aarch64" ]; then
    die "no prebuilt binary for aarch64 Linux; build from source: https://github.com/$repo"
fi

if [ -n "${GRAMTTY_VERSION:-}" ]; then
    version="${GRAMTTY_VERSION#v}"
else
    latest="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")" ||
        die "cannot resolve the latest release of $repo"
    version="${latest##*/}"
    version="${version#v}"
    case "$version" in
        "" | latest) die "no published release found for $repo" ;;
    esac
fi

asset="gramtty-$version-$arch-$os.tar.gz"
base="https://github.com/$repo/releases/download/v$version"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
trap 'exit 1' INT TERM

printf 'Downloading %s\n' "$asset"
curl -fsSL -o "$tmp/$asset" "$base/$asset" ||
    die "release v$version has no asset $asset ($base/$asset)"

if curl -fsL -o "$tmp/$asset.sha256" "$base/$asset.sha256"; then
    expected="$(cut -d ' ' -f 1 "$tmp/$asset.sha256")"
    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$tmp/$asset" | cut -d ' ' -f 1)"
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "$tmp/$asset" | cut -d ' ' -f 1)"
    else
        die "neither sha256sum nor shasum found to verify the download"
    fi
    if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
        die "checksum mismatch for $asset (expected $expected, got $actual)"
    fi
    printf 'Checksum OK\n'
elif [ "${GRAMTTY_SKIP_CHECKSUM:-}" = "1" ]; then
    warn "no $asset.sha256 in release v$version; installing WITHOUT checksum verification (GRAMTTY_SKIP_CHECKSUM=1)"
else
    die "release v$version has no $asset.sha256; set GRAMTTY_SKIP_CHECKSUM=1 to install without verification"
fi

tar -xzf "$tmp/$asset" -C "$tmp"
bin="$tmp/${asset%.tar.gz}/gramtty"
[ -f "$bin" ] || die "archive $asset does not contain ${asset%.tar.gz}/gramtty"

mkdir -p "$install_dir"
cp "$bin" "$install_dir/gramtty.tmp"
chmod 755 "$install_dir/gramtty.tmp"
mv -f "$install_dir/gramtty.tmp" "$install_dir/gramtty"
printf 'Installed gramtty %s to %s/gramtty\n' "$version" "$install_dir"

case ":$PATH:" in
    *":$install_dir:"*) ;;
    *) warn "$install_dir is not in PATH; add it, e.g. export PATH=\"$install_dir:\$PATH\"" ;;
esac
