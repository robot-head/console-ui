#!/usr/bin/env bash
set -euo pipefail
project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"
bash scripts/build-macos.sh release
app_root="$project_root/dist/Console UI.app"
ditto -c -k --sequesterRsrc --keepParent "$app_root" "$project_root/dist/console-ui-macos-$(uname -m).zip"
(cd dist && shasum -a 256 "console-ui-macos-$(uname -m).zip" > "console-ui-macos-$(uname -m).zip.sha256")
