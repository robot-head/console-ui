#!/usr/bin/env bash
set -euo pipefail
root="${1:?Usage: sign-release-assets.sh ASSET_DIRECTORY}"
found=false
for file in "$root"/*; do
  [[ "$file" == *.sigstore.json ]] && continue
  test -f "$file"
  cosign sign-blob --yes --bundle "$file.sigstore.json" "$file" >/dev/null
  found=true
done
test "$found" = true
