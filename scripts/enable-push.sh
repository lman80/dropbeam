#!/bin/sh
# Turn on Transfer Server push for the iOS app (after the App ID has Push
# Notifications enabled — see docs/PUSH-SETUP.md). Adds the aps-environment
# entitlement and regenerates the Xcode project.
set -e
cd "$(dirname "$0")/../src-tauri/gen/apple"
sed -i '' 's/^        # aps-environment: production$/        aps-environment: production/' project.yml
grep -q '^        aps-environment: production$' project.yml || { echo "aps-environment line not found in project.yml"; exit 1; }
xcodegen generate --spec project.yml
echo "Push entitlement on. Build + upload as usual."
