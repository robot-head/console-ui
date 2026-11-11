#!/usr/bin/env bash
set -euo pipefail
root="${1:?Usage: verify-release-signatures.sh DIRECTORY WORKFLOW_IDENTITY SOURCE_SHA REPOSITORY}"
identity="${2:?Expected workflow identity is required}"
source_sha="${3:?Expected source revision is required}"
repository="${4:?Expected repository is required}"
found=false
for file in "$root"/*; do
  [[ "$file" == *.sigstore.json ]] && continue
  test -f "$file"
  test -f "$file.sigstore.json"
  cosign verify-blob --bundle "$file.sigstore.json" \
    --certificate-identity "$identity" \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com \
    --certificate-github-workflow-sha "$source_sha" \
    --certificate-github-workflow-repository "$repository" "$file"
  found=true
done
test "$found" = true
