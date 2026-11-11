#!/usr/bin/env bash
set -euo pipefail
app="${1:?Usage: verify-macos-signature.sh APP_BUNDLE TEAM_ID}"
team="${2:?Expected Apple team ID is required}"
codesign --verify --deep --strict "$app"
for binary in "$app" "$app/Contents/MacOS/console-ui" "$app/Contents/Frameworks/libconsole_ui.dylib"; do
  details="$(codesign --display --verbose=4 "$binary" 2>&1)"
  SIGNATURE_DETAILS="$details" EXPECTED_TEAM="$team" python3 - <<'PY'
import os
details = os.environ['SIGNATURE_DETAILS'].splitlines()
assert 'TeamIdentifier=' + os.environ['EXPECTED_TEAM'] in details, 'Unexpected Apple signing team'
assert any(line.startswith('Authority=Developer ID Application:') for line in details), 'Developer ID Application signature required'
assert any(line.startswith('CodeDirectory ') and 'runtime' in line for line in details), 'Hardened runtime required'
assert any(line.startswith('Timestamp=') for line in details), 'Trusted signing timestamp required'
PY
done
xcrun stapler validate "$app"
spctl --assess --type execute --verbose=2 "$app"
