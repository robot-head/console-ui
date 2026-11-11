#!/usr/bin/env python3
"""Configure the GitHub signing environment using an administrator's gh login."""
import argparse
import json
import re
import subprocess

VARIABLES = {"AZURE_CLIENT_ID", "AZURE_TENANT_ID", "AZURE_SUBSCRIPTION_ID", "AZURE_SIGNING_ENDPOINT", "AZURE_SIGNING_ACCOUNT", "AZURE_CERTIFICATE_PROFILE", "WINDOWS_SIGNER_SUBJECT", "MACOS_TEAM_ID", "MACOS_NOTARY_KEY_ID", "MACOS_NOTARY_ISSUER_ID"}


def api(method, path, body=None, missing_ok=False):
    command = ["gh", "api", "--method", method, path]
    if body is not None:
        command += ["--input", "-"]
    result = subprocess.run(command, input=json.dumps(body) if body is not None else None, capture_output=True, text=True)
    if result.returncode:
        if missing_ok and "HTTP 404" in result.stderr:
            return None
        raise RuntimeError(result.stderr.strip() + "; environment administration requires a GitHub login with repository Administration access")
    return json.loads(result.stdout) if result.stdout.strip() else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", default="robot-head/console-ui")
    parser.add_argument("--variables", help="JSON containing the ten non-secret signing variables; private keys are never read by this script")
    args = parser.parse_args()
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", args.repository):
        parser.error("Invalid repository")
    variables = None
    if args.variables:
        with open(args.variables) as stream:
            variables = json.load(stream)
        if set(variables) != VARIABLES or any(not isinstance(v, str) or not v.strip() or '<' in v or '>' in v for v in variables.values()):
            parser.error("Supply exactly the ten non-secret variables, with real values replacing every placeholder")
    base = f"repos/{args.repository}/environments/release-signing"
    try:
        environment = api("GET", base, missing_ok=True)
        if environment is None:
            environment = api("PUT", base, {"deployment_branch_policy": {"protected_branches": False, "custom_branch_policies": True}})
        policy = environment.get("deployment_branch_policy") or {}
        if not policy.get("custom_branch_policies") or policy.get("protected_branches"):
            raise RuntimeError("Existing release-signing environment must use selected deployment branches/tags; its protection rules were preserved")
        existing = api("GET", base + "/deployment-branch-policies")["branch_policies"]
        desired = {("master", "branch"), ("v*", "tag")}
        actual = {(p["name"], p.get("type", "branch")) for p in existing}
        if actual - desired:
            raise RuntimeError("Remove unexpected release-signing branch/tag policies before continuing; existing policies were preserved")
        for name, kind in sorted(desired - actual):
            api("POST", base + "/deployment-branch-policies", {"name": name, "type": kind})
        if variables:
            for name, value in variables.items():
                subprocess.run(["gh", "variable", "set", name, "--repo", args.repository, "--env", "release-signing", "--body", value], check=True)
        print("Configured release-signing for master and v* tags. Add the three Apple secrets using gh secret set or GitHub Settings.")
    except (RuntimeError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Signing setup incomplete: {error}\n")


if __name__ == "__main__":
    main()
