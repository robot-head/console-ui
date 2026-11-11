import base64
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("release_assets", ROOT / "scripts/release_assets.py")
assets = importlib.util.module_from_spec(spec)
spec.loader.exec_module(assets)
SHA = "a" * 40
IDENTITY = "https://github.com/robot-head/console-ui/.github/workflows/release.yml@refs/tags/v0.1.1"


class ReleasePolicyTests(unittest.TestCase):
    def test_tags_cannot_bypass_signing(self):
        for event in ("push", "workflow_dispatch"):
            with self.subTest(event=event), self.assertRaises(ValueError):
                assets.release_policy(event, "refs/tags/v0.1.1", "preview")

    def test_signed_manual_builds_require_master(self):
        self.assertTrue(assets.release_policy("workflow_dispatch", "refs/heads/master", "signed"))
        with self.assertRaises(ValueError):
            assets.release_policy("workflow_dispatch", "refs/heads/feature", "signed")

    def test_prs_are_always_unprivileged_previews(self):
        self.assertFalse(assets.release_policy("pull_request", "refs/pull/2/merge", "signed"))

    def test_missing_credentials_and_untrusted_azure_endpoint_fail(self):
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(ValueError, "AZURE_CLIENT_ID"):
                assets.preflight(SimpleNamespace(platform="windows-x64"))
            with self.assertRaisesRegex(ValueError, "MACOS_CERTIFICATE"):
                assets.preflight(SimpleNamespace(platform="macos-arm64"))
        with patch.dict(os.environ, {key: "configured" for key in assets.WINDOWS_CONFIG}, clear=True):
            os.environ["AZURE_SIGNING_ENDPOINT"] = "https://untrusted.example"
            with self.assertRaisesRegex(ValueError, "regional endpoint"):
                assets.preflight(SimpleNamespace(platform="windows-x64"))


class AssetTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.output = self.root / "assets"

    def stage(self, signed=False):
        for platform in assets.PLATFORMS:
            source = self.root / (platform + ".archive")
            source.write_bytes((platform + " payload").encode())
            evidence = self.root / (platform + ".json")
            evidence.write_text(json.dumps({"native_signing": assets.NATIVE_SIGNATURES[platform]}))
            assets.stage(SimpleNamespace(output=self.output, version="0.1.1", platform=platform,
                         archive=source, distribution="signed" if signed else "preview", identity=IDENTITY if signed else "",
                         source_sha=SHA, native_evidence=str(evidence)))
        if signed:
            # Inventory fixtures only; real bundle cryptography is checked by the OIDC CI probe.
            for path in list(self.output.iterdir()):
                Path(str(path) + ".sigstore.json").write_text("{}")

    def verify(self, signed=False):
        assets.verify(SimpleNamespace(root=self.output, version="0.1.1", source_sha=SHA,
                      distribution="signed" if signed else "preview", identity=IDENTITY if signed else ""))

    def test_preview_and_signed_inventories(self):
        self.stage()
        self.verify()
        for path in self.output.iterdir():
            path.unlink()
        self.stage(True)
        self.verify(True)

    def test_tampered_archive_and_missing_bundle_fail(self):
        self.stage(True)
        archive = self.output / "console-ui-0.1.1-windows-x64.zip"
        original = archive.read_bytes()
        archive.write_bytes(b"tampered")
        with self.assertRaisesRegex(ValueError, "checksum"):
            self.verify(True)
        archive.write_bytes(original)
        (self.output / "console-ui-0.1.1-windows-x64.zip.sigstore.json").unlink()
        with self.assertRaisesRegex(ValueError, "Missing or unexpected"):
            self.verify(True)

    def test_source_and_identity_mismatches_fail(self):
        self.stage(True)
        path = self.output / "console-ui-0.1.1-linux-x86_64.json"
        original = json.loads(path.read_text())
        for key, value in (("source_revision", "b" * 40), ("signing_identity", "https://untrusted.example")):
            changed = dict(original, **{key: value})
            path.write_text(json.dumps(changed))
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.verify(True)
        path.write_text(json.dumps(original))

    def test_unsigned_manifest_cannot_satisfy_signed_distribution(self):
        self.stage()
        with self.assertRaisesRegex(ValueError, "Unsigned preview"):
            self.verify(True)


class MacSigningTests(unittest.TestCase):
    def run_signer(self, status):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / "bin"
            binary.mkdir()
            mock = binary / "mock"
            mock.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
name=pathlib.Path(sys.argv[0]).name
args=sys.argv[1:]
with open(os.environ['SIGNING_TEST_TRACE'],'a') as stream: stream.write(json.dumps([name]+args)+'\\n')
if name=='security':
    if args[0]=='create-keychain': pathlib.Path(args[-1]).touch()
    if args[0]=='find-identity': print('1) '+ 'A'*40 +' "Developer ID Application: Fixture (TEAM123456)"')
    if args[0]=='import': assert pathlib.Path(args[1]).stat().st_mode & 0o777 == 0o600
elif name=='codesign' and '--display' in args:
    print('TeamIdentifier=TEAM123456\\nAuthority=Developer ID Application: Fixture (TEAM123456)\\nCodeDirectory flags=0x10000(runtime)\\nTimestamp=fixture',file=sys.stderr)
elif name=='ditto': pathlib.Path(args[-1]).touch()
elif name=='xcrun' and args[:2]==['notarytool','submit']:
    assert '--wait' in args and '--timeout' in args
    print(json.dumps({'status':os.environ['SIGNING_TEST_STATUS'],'id':'fixture-submission'}))
''')
            mock.chmod(0o755)
            for command in ("security", "codesign", "ditto", "xcrun", "spctl"):
                (binary / command).symlink_to(mock)
            app = root / "Console UI.app"
            for path in ("Contents/Frameworks/libconsole_ui.dylib", "Contents/MacOS/console-ui", "Contents/MacOS/ConsoleUI"):
                target = app / path
                target.parent.mkdir(parents=True, exist_ok=True)
                target.touch()
            env = dict(os.environ, PATH=str(binary) + os.pathsep + os.environ["PATH"], RUNNER_TEMP=str(root),
                       GITHUB_ACTIONS="", SIGNING_TEST_TRACE=str(root / "trace"), SIGNING_TEST_STATUS=status,
                       MACOS_TEAM_ID="TEAM123456", MACOS_CERTIFICATE_P12_BASE64=base64.b64encode(b"fixture-p12").decode(),
                       MACOS_CERTIFICATE_PASSWORD="fixture-password", MACOS_NOTARY_KEY_P8_BASE64=base64.b64encode(b"fixture-p8").decode(),
                       MACOS_NOTARY_KEY_ID="fixture", MACOS_NOTARY_ISSUER_ID="fixture")
            result = subprocess.run(["bash", str(ROOT / "scripts/sign-macos.sh"), str(app), str(root / "evidence.json")], env=env, capture_output=True, text=True)
            calls = [json.loads(line) for line in (root / "trace").read_text().splitlines()]
            self.assertFalse(list(root.glob("console-ui-signing.*")), "Private signing material was not cleaned")
            self.assertTrue(any(call[:2] == ["security", "delete-keychain"] for call in calls))
            self.assertNotIn("fixture-password", result.stdout + result.stderr)
            return result, calls

    def test_accepted_notarization_signs_inside_out_and_cleans_credentials(self):
        result, calls = self.run_signer("Accepted")
        self.assertEqual(result.returncode, 0, result.stderr)
        signing = [call for call in calls if call[0] == "codesign" and "--sign" in call]
        self.assertEqual(len(signing), 4)
        self.assertTrue(signing[0][-1].endswith("libconsole_ui.dylib"))
        self.assertTrue(signing[-1][-1].endswith("Console UI.app"))
        self.assertTrue(all("--deep" not in call and "--timestamp" in call and "runtime" in call for call in signing))
        self.assertTrue(any(call[:3] == ["xcrun", "stapler", "staple"] for call in calls))

    def test_rejected_notarization_never_staples_or_succeeds(self):
        result, calls = self.run_signer("Invalid")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(call[:3] == ["xcrun", "stapler", "staple"] for call in calls))


if __name__ == "__main__":
    unittest.main()
