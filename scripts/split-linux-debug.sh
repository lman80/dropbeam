#!/usr/bin/env bash
# Linux release builds: move the debug info out of the shipped binary before
# tauri bundles it (.deb/.rpm/.AppImage), keeping it as a separate
# DropBeam-<triple>.debug file for symbolicating crash reports.
#
# Runs as build.beforeBundleCommand (src-tauri/tauri.linux.conf.json), i.e. after
# cargo has built the binary and before the bundler copies it. The release
# profile keeps line tables (debug = 1) so this is where the ~90 MB shrinks.
# CI uploads target/debug-symbols/ as a workflow artifact.
set -euo pipefail

[ "${TAURI_ENV_DEBUG:-false}" = "true" ] && exit 0
command -v objcopy >/dev/null || { echo "split-linux-debug: objcopy not found, binary left unstripped"; exit 0; }

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="${CARGO_TARGET_DIR:-$root/src-tauri/target}"
triple="${TAURI_ENV_TARGET_TRIPLE:-}"
bin="$target/$triple/release/DropBeam"
[ -f "$bin" ] || bin="$target/release/DropBeam"
[ -f "$bin" ] || { echo "split-linux-debug: no release binary found under $target"; exit 0; }

out="$target/debug-symbols"
mkdir -p "$out"
dbg="$out/DropBeam-${triple:-$(uname -m)}.debug"

before=$(stat -c %s "$bin")
objcopy --only-keep-debug "$bin" "$dbg"
objcopy --strip-debug --strip-unneeded "$bin"
objcopy --add-gnu-debuglink="$dbg" "$bin"
echo "split-linux-debug: $(basename "$bin") $before -> $(stat -c %s "$bin") bytes; symbols in $dbg"
