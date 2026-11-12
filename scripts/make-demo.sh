#!/bin/sh
# usage: scripts/make-demo.sh [out.gif]
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-"$root/assets/demo.gif"}
frames=$(mktemp -d)
trap 'rm -rf "$frames"' EXIT

cd "$root"
GRAMTTY_DEMO_OUT="$frames" cargo test demo::demo_frames -- --ignored --exact
python3 scripts/demo-gif.py "$frames" "$out"
