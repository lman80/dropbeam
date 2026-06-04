#!/usr/bin/env bash
# Download the current croc release for every bundled target and place the
# binaries in src-tauri/binaries/ with the Rust-target-triple names Tauri expects.
set -euo pipefail

DIR="$(cd "$(dirname "$0")/.." && pwd)/src-tauri/binaries"
mkdir -p "$DIR"

TAG=$(curl -s https://api.github.com/repos/schollz/croc/releases/latest \
  | sed -nE 's/.*"tag_name": *"([^"]+)".*/\1/p' | head -1)
echo "Fetching croc $TAG"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

dl() { curl -sL -o "$tmp/$2" "https://github.com/schollz/croc/releases/download/$TAG/croc_${TAG}_$1"; }

dl "macOS-ARM64.tar.gz" m1.tgz
tar xzf "$tmp/m1.tgz" -C "$tmp" croc && mv "$tmp/croc" "$DIR/croc-aarch64-apple-darwin"

dl "macOS-64bit.tar.gz" mi.tgz
tar xzf "$tmp/mi.tgz" -C "$tmp" croc && mv "$tmp/croc" "$DIR/croc-x86_64-apple-darwin"

dl "Windows-64bit.zip" w.zip
unzip -o "$tmp/w.zip" croc.exe -d "$tmp" >/dev/null && mv "$tmp/croc.exe" "$DIR/croc-x86_64-pc-windows-msvc.exe"

chmod +x "$DIR/croc-aarch64-apple-darwin" "$DIR/croc-x86_64-apple-darwin"
echo "Placed croc sidecars in $DIR"
