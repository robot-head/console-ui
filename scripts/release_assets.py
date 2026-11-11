#!/usr/bin/env python3
"""Version, signing-policy, and transport checks for native release assets."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import tomllib
from urllib.parse import urlparse

PLATFORMS = {"windows-x64": "zip", "linux-x86_64": "tar.gz", "macos-arm64": "zip"}
NATIVE_SIGNATURES = {"windows-x64": "authenticode", "linux-x86_64": "sigstore", "macos-arm64": "developer-id-notarized"}
WINDOWS_CONFIG = ("AZURE_CLIENT_ID", "AZURE_TENANT_ID", "AZURE_SUBSCRIPTION_ID", "AZURE_SIGNING_ENDPOINT", "AZURE_SIGNING_ACCOUNT", "AZURE_CERTIFICATE_PROFILE", "WINDOWS_SIGNER_SUBJECT")
MACOS_CONFIG = ("MACOS_TEAM_ID", "MACOS_NOTARY_KEY_ID", "MACOS_NOTARY_ISSUER_ID", "MACOS_CERTIFICATE_P12_BASE64", "MACOS_CERTIFICATE_PASSWORD", "MACOS_NOTARY_KEY_P8_BASE64")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def release_policy(event, ref, distribution):
    require(distribution in ("signed", "preview"), "Unknown distribution mode")
    if event == "pull_request":
        return False
    if ref.startswith("refs/tags/"):
        require(distribution == "signed", "Version tags require signed distribution")
        return True
    require(event == "workflow_dispatch", "Release builds require a tag or manual dispatch")
    if distribution == "signed":
        require(ref == "refs/heads/master", "Signed branch builds must use master")
        return True
    return False


def metadata(args):
    root = Path(args.root)
    version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
    gtk = tomllib.loads((root / "platforms/linux/Cargo.toml").read_text())["package"]["version"]
    with (root / "platforms/macos/Info.plist").open("rb") as stream:
        mac = plistlib.load(stream)
    require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?", version), "Invalid application version")
    require(gtk == mac["CFBundleShortVersionString"] == mac["CFBundleVersion"] == version, "Application versions differ")
    signed = release_policy(args.event, args.ref, args.distribution)
    if args.ref.startswith("refs/tags/"):
        require(args.ref == "refs/tags/v" + version, "Tag must match v<Cargo version>")
    sha = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    require(re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", args.repository), "Invalid repository")
    identity = f"https://github.com/{args.repository}/.github/workflows/release.yml@{args.ref}" if signed else ""
    with open(args.output, "a") as stream:
        stream.write(f"version={version}\nsource_sha={sha}\nsigned={str(signed).lower()}\nsigning_identity={identity}\n")


def preflight(args):
    names = WINDOWS_CONFIG if args.platform == "windows-x64" else MACOS_CONFIG if args.platform == "macos-arm64" else ()
    missing = [name for name in names if not os.environ.get(name, "").strip()]
    require(not missing, "Configure release-signing environment values: " + ", ".join(missing))
    if args.platform == "windows-x64":
        endpoint = urlparse(os.environ["AZURE_SIGNING_ENDPOINT"])
        require(endpoint.scheme == "https" and (endpoint.hostname or "").endswith(".codesigning.azure.net") and not endpoint.username and not endpoint.query, "Use the HTTPS Azure Artifact Signing regional endpoint")
    if args.platform == "macos-arm64":
        require(re.fullmatch(r"[A-Z0-9]{10}", os.environ["MACOS_TEAM_ID"]), "Invalid Apple team ID")


def stage(args):
    root = Path(args.output)
    root.mkdir(parents=True, exist_ok=True)
    stem = f"console-ui-{args.version}-{args.platform}"
    name = f"{stem}.{PLATFORMS[args.platform]}"
    archive = root / name
    shutil.copyfile(args.archive, archive)
    checksum = digest(archive)
    (root / (name + ".sha256")).write_text(f"{checksum}  {name}\n", encoding="ascii")
    signed = args.distribution == "signed"
    require(not signed or args.identity, "Signed assets require an expected workflow identity")
    evidence = None
    if signed and args.platform != "linux-x86_64":
        require(args.native_evidence, "Native signature verification evidence is required")
        evidence = json.loads(Path(args.native_evidence).read_text())
        require(evidence.get("native_signing") == NATIVE_SIGNATURES[args.platform], "Native signature verification evidence differs")
    manifest = {"schema_version": 1, "version": args.version, "platform": args.platform,
                "source_revision": args.source_sha, "archive": name, "sha256": checksum,
                "distribution": args.distribution, "signing_identity": args.identity if signed else None,
                "native_signing": NATIVE_SIGNATURES[args.platform] if signed else None}
    if evidence is not None:
        manifest["native_verification"] = evidence
    (root / (stem + ".json")).write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


def verify(args):
    root = Path(args.root)
    expected = set()
    signed = args.distribution == "signed"
    require(not signed or args.identity, "Signed assets require an expected workflow identity")
    for platform, extension in PLATFORMS.items():
        stem = f"console-ui-{args.version}-{platform}"
        name = f"{stem}.{extension}"
        files = {name, name + ".sha256", stem + ".json"}
        expected.update(files)
        if signed:
            expected.update(f + ".sigstore.json" for f in files)
        manifest = json.loads((root / (stem + ".json")).read_text())
        require(manifest["schema_version"] == 1 and manifest["version"] == args.version and manifest["platform"] == platform, "Manifest version/platform mismatch")
        require(manifest["source_revision"] == args.source_sha and manifest["archive"] == name, "Manifest source/archive mismatch")
        require(manifest.get("distribution") == args.distribution, "Unsigned preview cannot satisfy a signed release")
        require(manifest.get("signing_identity") == (args.identity if signed else None), "Workflow signing identity mismatch")
        require(manifest.get("native_signing") == (NATIVE_SIGNATURES[platform] if signed else None), "Native signing mode mismatch")
        if signed and platform != "linux-x86_64":
            require(manifest.get("native_verification", {}).get("native_signing") == NATIVE_SIGNATURES[platform], "Missing native verification evidence")
        checksum = digest(root / name)
        require(checksum == manifest["sha256"], "Archive checksum mismatch")
        require((root / (name + ".sha256")).read_text().strip() == f"{checksum}  {name}", "Sidecar checksum mismatch")
    require({p.name for p in root.iterdir()} == expected, "Missing or unexpected release assets")
    print(f"Verified {len(expected)} {args.distribution} assets and all source manifests.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    meta = commands.add_parser("metadata")
    meta.add_argument("--root", default=".")
    meta.add_argument("--event", default=os.environ.get("GITHUB_EVENT_NAME"), required=False)
    meta.add_argument("--ref", default=os.environ.get("GITHUB_REF"))
    meta.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY"))
    meta.add_argument("--distribution", choices=("signed", "preview"), default="signed")
    meta.add_argument("--output", default=os.environ.get("GITHUB_OUTPUT"))
    meta.set_defaults(function=metadata)
    config = commands.add_parser("preflight")
    config.add_argument("--platform", choices=PLATFORMS, required=True)
    config.set_defaults(function=preflight)
    for name, function in (("stage", stage), ("verify", verify)):
        command = commands.add_parser(name)
        command.add_argument("--version", required=True)
        command.add_argument("--source-sha", required=True)
        command.add_argument("--distribution", choices=("signed", "preview"), required=True)
        command.add_argument("--identity", default="")
        if name == "stage":
            command.add_argument("--platform", choices=PLATFORMS, required=True)
            command.add_argument("--archive", required=True)
            command.add_argument("--output", default="dist/release")
            command.add_argument("--native-evidence")
        else:
            command.add_argument("--root", default="release-assets")
        command.set_defaults(function=function)
    args = parser.parse_args()
    try:
        args.function(args)
    except (ValueError, KeyError, OSError) as error:
        parser.exit(1, f"Release validation failed: {error}\n")


if __name__ == "__main__":
    main()
