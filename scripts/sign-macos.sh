#!/usr/bin/env bash
set -euo pipefail
umask 077
app="${1:?Usage: sign-macos.sh APP_BUNDLE EVIDENCE_JSON}"
evidence="${2:?Notarization evidence path is required}"
: "${MACOS_TEAM_ID:?Configure MACOS_TEAM_ID}"
: "${MACOS_CERTIFICATE_P12_BASE64:?Configure MACOS_CERTIFICATE_P12_BASE64}"
: "${MACOS_CERTIFICATE_PASSWORD:?Configure MACOS_CERTIFICATE_PASSWORD}"
: "${MACOS_NOTARY_KEY_P8_BASE64:?Configure MACOS_NOTARY_KEY_P8_BASE64}"
: "${MACOS_NOTARY_KEY_ID:?Configure MACOS_NOTARY_KEY_ID}"
: "${MACOS_NOTARY_ISSUER_ID:?Configure MACOS_NOTARY_ISSUER_ID}"
work="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/console-ui-signing.XXXXXX")"
keychain="$work/signing.keychain-db"
cleanup() {
  security delete-keychain "$keychain" >/dev/null 2>&1 || true
  rm -rf -- "$work"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
export SIGNING_PRIVATE_DIRECTORY="$work"
python3 - <<'PY'
import base64, os, pathlib
root = pathlib.Path(os.environ['SIGNING_PRIVATE_DIRECTORY'])
for variable, name in [('MACOS_CERTIFICATE_P12_BASE64', 'identity.p12'), ('MACOS_NOTARY_KEY_P8_BASE64', 'notary.p8')]:
    (root / name).write_bytes(base64.b64decode(''.join(os.environ[variable].split()), validate=True))
PY
keychain_password="$(openssl rand -hex 32)"
if [[ "${GITHUB_ACTIONS:-}" == true ]]; then
  printf '::add-mask::%s\n' "$keychain_password"
fi
security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$work/identity.p12" -k "$keychain" -P "$MACOS_CERTIFICATE_PASSWORD" -T /usr/bin/codesign >/dev/null
security set-key-partition-list -S apple-tool:,apple: -s -k "$keychain_password" "$keychain" >/dev/null
identity="$(SIGNING_KEYCHAIN="$keychain" python3 - <<'PY'
import os, re, subprocess
text = subprocess.check_output(['security', 'find-identity', '-v', '-p', 'codesigning', os.environ['SIGNING_KEYCHAIN']], text=True)
matches = re.findall(r'([A-Fa-f0-9]{40}) "Developer ID Application: [^"\n]+ \(' + re.escape(os.environ['MACOS_TEAM_ID']) + r'\)"', text)
assert len(matches) == 1, 'Import exactly one valid Developer ID Application identity for the configured team'
print(matches[0])
PY
)"
# Sign nested code explicitly, then seal the outer bundle; do not use --deep to sign.
for binary in "$app/Contents/Frameworks/libconsole_ui.dylib" "$app/Contents/MacOS/console-ui" "$app/Contents/MacOS/ConsoleUI" "$app"; do
  codesign --force --sign "$identity" --keychain "$keychain" --options runtime --timestamp "$binary"
done
codesign --verify --deep --strict "$app"
ditto -c -k --sequesterRsrc --keepParent "$app" "$work/notarization.zip"
xcrun notarytool submit "$work/notarization.zip" --key "$work/notary.p8" \
  --key-id "$MACOS_NOTARY_KEY_ID" --issuer "$MACOS_NOTARY_ISSUER_ID" \
  --wait --timeout 20m --output-format json > "$work/notarization.json"
python3 - "$work/notarization.json" "$evidence" <<'PY'
import json, os, pathlib, sys
result = json.loads(pathlib.Path(sys.argv[1]).read_text())
assert result['status'] == 'Accepted', 'Apple notarization was not accepted: ' + result['status']
output = pathlib.Path(sys.argv[2])
output.parent.mkdir(parents=True, exist_ok=True)
output.write_text(json.dumps({'native_signing': 'developer-id-notarized', 'team_id': os.environ['MACOS_TEAM_ID'], 'notarization_id': result['id'], 'notarization_status': result['status']}, indent=2) + '\n')
PY
xcrun stapler staple "$app"
bash "$(dirname "$0")/verify-macos-signature.sh" "$app" "$MACOS_TEAM_ID"
