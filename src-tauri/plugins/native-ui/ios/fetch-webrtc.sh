#!/bin/sh
# Calls on iOS (docs/CALLS.md): Google's WebRTC, prebuilt by stasel/WebRTC
# (BSD-3-Clause, https://github.com/stasel/WebRTC, release M155: ~45 MB zip,
# ~97 MB unpacked). Fetched once into ./Frameworks (gitignored) and checked
# against the release's SwiftPM checksum before use. Run by
# plugins/native-ui/build.rs and by the app target's first build phase
# (gen/apple/project.yml), so either build path finds it.
set -eu
DIR="$(cd "$(dirname "$0")" && pwd)/Frameworks"
VERSION=155.0.0
URL="https://github.com/stasel/WebRTC/releases/download/$VERSION/WebRTC-M155.xcframework.zip"
SHA=cb9adacf2d6bbf598d1d9e3e6c16d8fb97ca987b5b697746891828f9dc068ffe
if [ -f "$DIR/WebRTC.xcframework/.version-$VERSION" ]; then exit 0; fi
mkdir -p "$DIR"
TMP="$DIR/webrtc-$$.zip"
trap 'rm -f "$TMP"' EXIT
curl -fsSL --retry 3 -o "$TMP" "$URL"
GOT=$(shasum -a 256 "$TMP" | cut -d' ' -f1)
if [ "$GOT" != "$SHA" ]; then echo "WebRTC download checksum mismatch ($GOT)" >&2; exit 1; fi
rm -rf "$DIR/WebRTC.xcframework"
unzip -q -o "$TMP" -d "$DIR"
touch "$DIR/WebRTC.xcframework/.version-$VERSION"
