#!/usr/bin/env bash
# Build the DropBeam Finder Sync extension (.appex) and embed it inside an
# already-built DropBeam.app, then code-sign. Run AFTER `tauri build` (or in CI
# after the Tauri bundle step), pointing APP_PATH at the built .app.
#
#   APP_PATH=/path/to/DropBeam.app \
#   SIGN_ID="Developer ID Application: Your Name (TEAMID)" \
#   macos/finder-sync/build-and-embed.sh
#
# SIGN_ID is REQUIRED for the extension to load on other users' Macs (a Finder
# Sync extension must be properly signed + notarized for distribution). Without a
# real Developer ID it can be built and will load only on THIS machine with
# ad-hoc signing + the extension manually enabled in System Settings.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
APP_PATH="${APP_PATH:?set APP_PATH to the built DropBeam.app}"
SIGN_ID="${SIGN_ID:--}" # default ad-hoc; override with a Developer ID for distribution
SDK="$(xcrun --show-sdk-path)"
APPEX="$APP_PATH/Contents/PlugIns/DropBeamFinderSync.appex"
MACOS_DIR="$APPEX/Contents/MacOS"

echo "==> compiling Swift extension"
mkdir -p "$MACOS_DIR"
xcrun swiftc -sdk "$SDK" -target arm64-apple-macos10.15 -O \
  -framework FinderSync -framework Cocoa \
  -emit-executable -o "$MACOS_DIR/DropBeamFinderSync" \
  "$HERE/DropBeamFinderSync.swift"
# (CI: also build x86_64 and `lipo -create` for a universal binary.)

cp "$HERE/Info.plist" "$APPEX/Contents/Info.plist"

echo "==> signing extension + app with: $SIGN_ID"
codesign --force --sign "$SIGN_ID" --options runtime "$APPEX"
codesign --force --sign "$SIGN_ID" --deep "$APP_PATH"

echo "==> done. Embedded: $APPEX"
echo "    For distribution this app must then be NOTARIZED (xcrun notarytool) —"
echo "    which requires a paid Apple Developer account. Ad-hoc (-) loads only"
echo "    locally with the extension enabled in System Settings > Login Items &"
echo "    Extensions > Finder."
