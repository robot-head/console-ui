#!/usr/bin/env bash
set -euo pipefail
project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"
mode="${1:-preview}"
[[ "$mode" == preview || "$mode" == signed ]] || { echo 'Usage: package-macos.sh [preview|signed]' >&2; exit 2; }
bash scripts/build-macos.sh release
app_root="$project_root/dist/Console UI.app"
if [[ "$mode" == signed ]]; then
  bash scripts/sign-macos.sh "$app_root" dist/signing-evidence/macos.json
fi
ditto -c -k --sequesterRsrc --keepParent "$app_root" "$project_root/dist/console-ui-macos-$(uname -m).zip"
(cd dist && shasum -a 256 "console-ui-macos-$(uname -m).zip" > "console-ui-macos-$(uname -m).zip.sha256")
