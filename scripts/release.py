"""Validate release metadata and publish only a tested artifact set."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def release_notes(changelog, version):
    headings = list(re.finditer(r"^## \[([^\]]+)\][^\n]*$", changelog, re.MULTILINE))
    matches = [i for i, heading in enumerate(headings) if heading[1] == version]
    if len(matches) != 1:
        raise ValueError(f"Expected one changelog section for {version}")
    index = matches[0]
    end = headings[index + 1].start() if index + 1 < len(headings) else len(changelog)
    notes = changelog[headings[index].end():end].strip()
    if not notes:
        raise ValueError(f"Empty changelog section for {version}")
    return notes + "\n"


def plan(event, before, publish):
    package = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]
    version = package["version"]
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-rc\.[1-9][0-9]*)?", version):
        raise ValueError("Release versions must be canonical X.Y.Z or X.Y.Z-rc.N")
    if event == "push":
        if not re.fullmatch(r"[0-9a-f]{40}", before) or before == "0" * 40:
            raise ValueError("A push release requires an existing previous commit")
        previous = tomllib.loads(subprocess.check_output(
            ["git", "show", f"{before}:Cargo.toml"], cwd=ROOT, text=True
        ))["package"]["version"]
        publish = previous != version
    elif event == "pull_request":
        publish = False
    elif event != "workflow_dispatch":
        raise ValueError(f"Unsupported release event: {event}")
    if publish and os.environ.get("GITHUB_REF") != "refs/heads/main":
        raise ValueError("Publishing is only allowed from main")
    sha = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    if publish and sha != os.environ.get("GITHUB_SHA"):
        raise ValueError("Checkout does not match the workflow commit")
    notes = release_notes((ROOT / "CHANGELOG.md").read_text(), version)
    output = ROOT / "target/release-plan"
    output.mkdir(parents=True, exist_ok=True)
    (output / "notes.md").write_text(notes)
    result = {"version": version, "tag": f"v{version}", "sha": sha,
              "publish": publish, "prerelease": "-" in version}
    (output / "plan.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as stream:
            for key, value in result.items():
                stream.write(f"{key}={str(value).lower() if isinstance(value, bool) else value}\n")


def gh(*args, check=True):
    return subprocess.run(["gh", *args], text=True, capture_output=True, check=check)


def api_optional(path):
    result = gh("api", path, check=False)
    if result.returncode:
        if "(HTTP 404)" in result.stderr:
            return None
        raise RuntimeError(result.stderr)
    return json.loads(result.stdout)


def publish(directory):
    release = json.loads((directory / "plan.json").read_text())
    if not release["publish"] or os.environ.get("GITHUB_REF") != "refs/heads/main":
        raise ValueError("This artifact set is not approved for publication from main")
    if release["sha"] != os.environ.get("GITHUB_SHA"):
        raise ValueError("Artifact commit differs from the publishing workflow")
    repo, tag = os.environ["GITHUB_REPOSITORY"], release["tag"]
    existing_tag = api_optional(f"repos/{repo}/git/ref/tags/{tag}")
    if existing_tag:
        commit = json.loads(gh("api", f"repos/{repo}/commits/{tag}").stdout)
        if commit["sha"] != release["sha"]:
            raise ValueError("Existing release tag points at a different commit")
    existing = api_optional(f"repos/{repo}/releases/tags/{tag}")
    if existing and not existing["draft"]:
        raise ValueError("Published releases are immutable; create a new version")
    if not existing_tag:
        gh("api", "--method", "POST", f"repos/{repo}/git/refs",
           "-f", f"ref=refs/tags/{tag}", "-f", f"sha={release['sha']}")
    if not existing:
        gh("release", "create", tag, "--repo", repo, "--verify-tag",
           "--draft", "--title", tag, "--notes-file", str(directory / "notes.md"))
    # Verify the tag again before uploading/publishing.
    commit = json.loads(gh("api", f"repos/{repo}/commits/{tag}").stdout)
    if commit["sha"] != release["sha"]:
        raise ValueError("Release tag changed during publication")
    assets = sorted((directory / "artifacts").iterdir())
    gh("release", "upload", tag, "--repo", repo, "--clobber", *map(str, assets))
    expected = {f.name for f in assets}
    uploaded = json.loads(gh("api", f"repos/{repo}/releases/tags/{tag}").stdout)
    if {asset["name"] for asset in uploaded["assets"]} != expected:
        raise ValueError("Draft contains unexpected or missing assets; inspect before retrying")
    gh("release", "edit", tag, "--repo", repo, "--draft=false",
       f"--prerelease={str(release['prerelease']).lower()}",
       f"--latest={str(not release['prerelease']).lower()}",
       "--notes-file", str(directory / "notes.md"))
    print(f"Published https://github.com/{repo}/releases/tag/{tag}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    planning = sub.add_parser("plan")
    planning.add_argument("--event", default="workflow_dispatch")
    planning.add_argument("--before", default="")
    planning.add_argument("--publish", action="store_true")
    publishing = sub.add_parser("publish")
    publishing.add_argument("directory", type=Path)
    args = parser.parse_args()
    if args.command == "plan":
        plan(args.event, args.before, args.publish)
    else:
        publish(args.directory)
