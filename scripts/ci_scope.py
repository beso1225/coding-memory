"""Select native CI from the complete Git diff, without API file-count limits."""
import json
import os
from pathlib import Path
import re
import subprocess


SHARED = {"Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "build.rs", "build_inputs.rs",
          "flake.nix", "flake.lock", "Taskfile.pkl", "scripts/ci_scope.py",
          "scripts/tests/test_ci_scope.py"}
MACOS = {f"src/{name}.rs" for name in ["main", "lib", "codex", "compatibility", "runtime_probe",
                                     "generation", "service", "worker", "ingestion"]}
MACOS |= {f"tests/{name}.rs" for name in ["generation", "worker", "reader", "compatibility", "ingestion"]}


def select(paths):
    def shared(path):
        return path in SHARED or path.startswith((".github/", ".cargo/"))
    return {
        "macos": any(shared(p) or p in MACOS or p.startswith((
            "experiments/codex_capture/", "experiments/codex_generation/", "experiments/worker/"
        )) for p in paths),
        "archive": any(shared(p) or p in {"CHANGELOG.md", "dist-workspace.toml"}
                       or p.startswith(("LICENSE", "licenses/", "third-party/", "scripts/release", "scripts/build_release",
                                        "scripts/install_dist", "scripts/tests/test_release")) for p in paths),
    }


def changed_paths(root, base, head, *, pull_request=False):
    if not all(re.fullmatch(r"[0-9a-f]{40}", sha) for sha in [base, head]):
        raise ValueError("CI comparisons require full commit SHAs")
    def git(*args):
        return subprocess.check_output(["git", "-C", str(root), *args], stderr=subprocess.PIPE)
    if base == "0" * 40:
        data = git("ls-tree", "-r", "--name-only", "-z", head)
    else:
        if pull_request:
            base = git("merge-base", base, head).decode().strip()
        data = git("diff", "--name-only", "--no-renames", "-z", base, head, "--")
    return [p.decode("utf-8", errors="surrogateescape") for p in data.split(b"\0") if p]


def main():
    event = os.environ["GITHUB_EVENT_NAME"]
    payload = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    if event == "workflow_dispatch":
        result = {"macos": True, "archive": True}
    elif event in {"push", "pull_request"}:
        pr = event == "pull_request"
        base = payload["pull_request"]["base"]["sha"] if pr else payload["before"]
        head = payload["pull_request"]["head"]["sha"] if pr else os.environ["GITHUB_SHA"]
        result = select(changed_paths(Path.cwd(), base, head, pull_request=pr))
    else:
        raise ValueError(f"Unsupported CI event: {event}")
    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as stream:
        for key, value in result.items():
            stream.write(f"{key}={str(value).lower()}\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
