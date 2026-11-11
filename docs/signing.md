# Signed distributions

Version tags require trusted Windows and macOS signatures, Apple notarization, and
keyless Sigstore signatures for every downloadable archive, checksum, and source
manifest. Missing credentials, rejected notarization, unexpected publishers, or
signature verification failures stop the release. Only successfully verified assets
reach a draft GitHub release; publication remains an explicit operator action.

The published `v0.1.0` downloads are preview builds. Enable this configuration before
issuing the next version tag; published release assets are never overwritten.

## 1. GitHub environment

Create the **release-signing** environment in this repository. Allow deployments only
from branch `master` and tags matching `v*`. PR previews run in a separate job with
read-only repository access, no OIDC permission, and no signing secrets. Signed
manual builds accept `master` or a matching version tag. A tag cannot request an
unsigned preview.

With an administrator's GitHub CLI login:

```sh
python3 scripts/configure-signing-environment.py --repository robot-head/console-ui
```

The script preserves existing reviewer settings and refuses unexpected branch/tag
policies. GitHub App connections without repository Administration access cannot
create/configure this environment. Secret and variable management also require the
corresponding repository permissions; configure them in GitHub Settings if the
integration returns HTTP 403.

Copy [signing-variables.example.json](signing-variables.example.json) outside the
checkout, replace its placeholders, then load the ten **non-secret variables**:

```sh
python3 scripts/configure-signing-environment.py \
  --repository robot-head/console-ui --variables /private/path/signing-variables.json
```

The Apple certificate, certificate password, and API private key belong in the
environment's three encrypted **secrets**, described below. Do not commit them or
paste them into chat, issue bodies, workflow inputs, or release notes.

## 2. Windows: Azure Artifact Signing

Create an Azure Artifact Signing account, complete Microsoft's identity validation,
and create a **PublicTrust** certificate profile. A PublicTrustTest profile does not
satisfy Windows trust verification. Account provisioning, billing, and legal identity
validation belong to the account owner.

Create a dedicated Microsoft Entra application/service principal. Assign it the
**Artifact Signing Certificate Profile Signer** role at the signing account (or a
narrower supported certificate-profile scope). Contributor or subscription-wide
permissions are unnecessary.

Add a federated credential to that application:

```json
{
  "name": "github-console-ui-release",
  "issuer": "https://token.actions.githubusercontent.com",
  "subject": "repo:robot-head/console-ui:environment:release-signing",
  "audiences": ["api://AzureADTokenExchange"]
}
```

For example, save that JSON to a local file and run:

```sh
az ad app federated-credential create \
  --id "$AZURE_CLIENT_ID" --parameters @/private/path/github-federation.json
az role assignment create --assignee-object-id "$AZURE_PRINCIPAL_ID" \
  --assignee-principal-type ServicePrincipal \
  --role 'Artifact Signing Certificate Profile Signer' --scope "$SIGNING_ACCOUNT_RESOURCE_ID"
```

Set the seven `AZURE_*`/`WINDOWS_*` variables from the example file. The regional
HTTPS endpoint must match the signing account. `WINDOWS_SIGNER_SUBJECT` is the exact
X.509 Subject from the issued PublicTrust signing certificate, including its normal
X.509 formatting; confirm it from the certificate/profile before releasing.

The workflow obtains Azure credentials through GitHub OIDC. It does not store an
Azure client secret or export the signing private key. Only `console-ui.exe` and
the project's `console_ui.dll`, when present, are submitted for signing. Microsoft
runtime/VC++ DLLs keep their vendor signatures. SHA-256 Authenticode signatures and
RFC 3161 timestamps are required. Verification checks Windows trust, the expected
publisher, code-signing usage, and timestamp before creating the ZIP and again
after extracting it.

Reference: [Microsoft's OIDC configuration](https://github.com/Azure/artifact-signing-action/blob/main/docs/OIDC.md).

## 3. macOS: Developer ID and notarization

An Apple Developer Program team must issue a **Developer ID Application** certificate.
Export that identity **with its private key** as a password-protected `.p12` from
Keychain Access. Apple Development, iOS distribution, and ad-hoc identities do not
satisfy this release policy. Create a team App Store Connect API key with access to
the notary service and download its `.p8` private key.

Set `MACOS_TEAM_ID`, `MACOS_NOTARY_KEY_ID`, and `MACOS_NOTARY_ISSUER_ID` as environment
variables. Add the three secrets securely; the password command prompts for input:

```sh
base64 < /private/path/DeveloperID.p12 | gh secret set MACOS_CERTIFICATE_P12_BASE64 \
  --repo robot-head/console-ui --env release-signing
gh secret set MACOS_CERTIFICATE_PASSWORD --repo robot-head/console-ui --env release-signing
base64 < /private/path/AuthKey.p8 | gh secret set MACOS_NOTARY_KEY_P8_BASE64 \
  --repo robot-head/console-ui --env release-signing
```

The workflow imports exactly one matching Developer ID identity into a temporary
keychain. It signs the Rust library, CLI, Swift executable, and app bundle inside
out with hardened runtime and trusted timestamps. It waits for Apple's accepted
notarization result, staples the ticket, and verifies the team, Developer ID chain,
runtime flags, timestamps, stapled ticket, and Gatekeeper assessment. The final ZIP
and its checksum are created **after stapling**. Extraction repeats native signature
and Gatekeeper checks before the UI smoke test. The temporary keychain, `.p12`, API
key, and submission ZIP are removed on success or failure.

## 4. Linux and archive authenticity: Sigstore

No Linux signing secret is required. GitHub OIDC supplies a short-lived Fulcio
certificate, and Cosign records signatures in the public transparency log. All
three platforms sign their final archive, checksum, and JSON source manifest. Each
file gets a `.sigstore.json` verification bundle: **18 assets** per signed release.
Linux retains the Ubuntu 24.04 GTK/GLib system-library baseline; these signatures
authenticate the tarball rather than creating a new apt/rpm repository.

Consumers must pin this repository, the release workflow identity, and the source
commit they expect. For example, after obtaining the tag's source commit through a
trusted checkout, verify a Linux archive with Cosign 3.1.4+:

```sh
tag=v0.1.1
source_sha="$(git rev-parse "$tag^{commit}")"
cosign verify-blob "console-ui-0.1.1-linux-x86_64.tar.gz" \
  --bundle "console-ui-0.1.1-linux-x86_64.tar.gz.sigstore.json" \
  --certificate-identity "https://github.com/robot-head/console-ui/.github/workflows/release.yml@refs/tags/$tag" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate-github-workflow-repository robot-head/console-ui \
  --certificate-github-workflow-sha "$source_sha"
```

To verify an entire downloaded release, run the same checksum/manifest and signature
checks used by CI:

```sh
identity="https://github.com/robot-head/console-ui/.github/workflows/release.yml@refs/tags/$tag"
python3 scripts/release_assets.py verify --root release-assets --version 0.1.1 \
  --source-sha "$source_sha" --distribution signed --identity "$identity"
bash scripts/verify-release-signatures.sh release-assets "$identity" "$source_sha" robot-head/console-ui
```

Do not take the expected identity or commit solely from an unverified manifest.
Signature verification checks the OIDC issuer, exact workflow/ref identity, repository,
source commit, certificate trust, and transparency-log evidence. It does not disable
certificate or transparency-log verification.

## Build and release

After merging the workflows and configuring the environment, first validate the
signing services without publishing:

```sh
gh workflow run release.yml --ref master -f distribution=signed
```

Inspect the native signing evidence and verify all downloaded artifacts. Update the
root/GTK Cargo versions, lockfile, and both macOS bundle versions together for the
next release, then push its matching version tag. Tags run signed builds and stage
a draft; verify distribution on a clean Windows 11 machine before public production
publication. The existing `v0.1.0` tag and published assets stay immutable.

An explicit manual `-f distribution=preview` builds untrusted-development previews
without signing credentials and produces no GitHub release. PR packaging checks use
that same path. [Signing checks](../.github/workflows/signing-checks.yml) also test
release policy and notarization failure cleanup, real Authenticode tamper detection,
and GitHub keyless signing/verification of a public test fixture. Fixture signatures
use the signing-checks workflow identity and cannot satisfy the release verifier.
