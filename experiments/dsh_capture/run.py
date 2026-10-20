#!/usr/bin/env python3
"""Isolated DeepSeek Harness capture experiment for issue #56.

The script creates a synthetic Git project, an isolated ``DSH_HOME`` and an
isolated engine state directory under ``/private/tmp/coding-memory-dsh-*``. It
mounts the bridge through a ``--patch`` overlay, runs real DSH sessions through
the installed CLI, and records what the engine captured.

Model calls are opt-in through ``--provider``. Without it the sessions still
persist their turns, ``turn/end`` still fires, and the capture pipeline runs end
to end against those native events; only assistant output is absent. With it,
the overlay routes the agent at that provider/model and the run is a real
model-driven session (external transmission of the synthetic project only).

Usage (from the repository root, with a built engine and a built DSH checkout):

    python3 experiments/dsh_capture/run.py \
        --dsh /path/to/deepseek-harness/apps/cli/lib/bin.js \
        --engine target/debug/coding-memory \
        --provider ollama-cloud --model deepseek-v4.1-flash:1017
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
BRIDGE = REPO / "bridge" / "dsh-capture" / "index.mjs"


def run(argv, cwd, env=None, stdin=None, timeout=180):
    return subprocess.run(
        argv, cwd=cwd, env=env, input=stdin, capture_output=True, text=True, timeout=timeout
    )


def git(cwd, *args):
    result = run(["git", *args], cwd)
    if result.returncode != 0:
        raise SystemExit(f"git {args} failed: {result.stderr}")
    return result.stdout.strip()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dsh", required=True, help="built dsh CLI entry (apps/cli/lib/bin.js)")
    parser.add_argument("--engine", required=True, help="built coding-memory executable")
    parser.add_argument(
        "--task",
        default="Reply with the single word: ok",
        help="synthetic task for each turn; keep it small and free of personal data",
    )
    parser.add_argument("--provider", help="provider route for a real model call, for example ollama-cloud")
    parser.add_argument("--model", help="model id on that route, for example deepseek-v4.1-flash:1017")
    parser.add_argument(
        "--credentials",
        action="append",
        default=[],
        help="credential file mounted from DSH_HOME (for example .credentials.yaml); "
        "a value written as NAME=value is passed only through the environment",
    )
    parser.add_argument(
        "--compact-after-turn",
        type=int,
        help="attach the experiment-only compaction driver and condense after this turn",
    )
    parser.add_argument(
        "--extra-patch",
        action="append",
        default=[],
        help="additional YAML overlay for targeted diagnosis (repeatable)",
    )
    parser.add_argument(
        "--pin-summary",
        action="store_true",
        help="pin the summarization route in the composition, as unattended use requires",
    )
    parser.add_argument(
        "--resume",
        action="store_true",
        help="continue the first session by --session-id in a second process",
    )
    parser.add_argument("--keep", action="store_true", help="keep the synthetic root for inspection")
    args = parser.parse_args()

    # Resolve the engine once: the bridge schema requires an absolute executable,
    # and every invocation runs from the synthetic project rather than the
    # repository root, where a relative path would not resolve.
    engine_binary = Path(args.engine).expanduser().resolve()
    if not engine_binary.is_file():
        raise SystemExit(f"engine executable not found: {engine_binary}")
    root = Path(tempfile.mkdtemp(prefix="coding-memory-dsh-"))
    # Canonicalize: the engine resolves a Git repository through git (realpath),
    # so a project path with a symlinked ancestor would register a different id
    # than the one the session recorded. /var is a symlink to /private/var.
    root = root.resolve()
    project = root / "project"
    home = root / "dsh-home"
    state = root / "engine-state"
    project.mkdir()
    home.mkdir()
    state.mkdir()
    # Credentials passed as NAME=value only reach the child environment.
    env_keys = {}
    for entry in args.credentials:
        if "=" in entry:
            name, value = entry.split("=", 1)
            env_keys[name] = value
            continue
        source = Path(entry).expanduser()
        if not source.is_file():
            raise SystemExit(f"credential file not found: {source}")
        # The credential store has a fixed filename in the harness home; mount
        # the source under that name regardless of its own basename.
        target = home / ".credentials.yaml"
        shutil.copyfile(source, target)
        target.chmod(0o600)
    git(project, "init", "-q")
    git(project, "config", "user.email", "probe@example.invalid")
    git(project, "config", "user.name", "probe")
    (project / "README.md").write_text("synthetic project\n")
    git(project, "add", ".")
    git(project, "commit", "-qm", "init")

    env = {k: v for k, v in os.environ.items() if k not in ("OLLAMA_CLOUD_API_KEY", "DEEPSEEK_API_KEY")}
    env["DSH_HOME"] = str(home)

    route = ""
    if args.provider:
        if not args.model:
            raise SystemExit("--provider needs --model")
        key_env = f"{args.provider.upper().replace('-', '_')}_API_KEY"
        # The shipped headless profile disables the managed credential store and
        # authenticates from the launch environment only. An isolated home has no
        # such environment, so the run re-enables the store it mounts, exactly as
        # the headless README documents for stored credentials.
        route = (
            "- id: credentials\n"
            "  disabled: false\n"
            "- id: llm-pi-ai\n"
            "  config:\n"
            "    providers:\n"
            f"      {args.provider}:\n"
            f"        apiKeyEnv: {key_env}\n"
            "- id: agent-default-model\n"
            "  config:\n"
            f"    provider: {args.provider}\n"
            f"    model: {args.model}\n"
        )
        if args.pin_summary:
            route += (
                "- id: compaction-basic\n"
                "  config:\n"
                f"    summarizationProvider: {args.provider}\n"
                f"    summarizationModel: {args.model}\n"
            )

    probe = ""
    if args.compact_after_turn:
        probe = (
            "- insert:\n"
            "    - id: coding-memory-compaction-probe\n"
            f"      name: '{Path(__file__).with_name('compaction_probe.mjs')}'\n"
            "      config:\n"
            f"        triggerTurn: {args.compact_after_turn}\n"
        )

    # An empty overlay first: `dsh --profile headless` reads options that follow
    # the profile name, so the app never sees `--json`.
    empty = root / "empty-patch.yml"
    empty.write_text("[]\n")
    overlays = ["--patch", str(empty)]

    patch = root / "bridge-patch.yml"
    patch.write_text(
        route + probe +
        "- insert:\n"
        "    - id: coding-memory-capture\n"
        f"      name: '{BRIDGE}'\n"
        "      config:\n"
        f"        executable: '{engine_binary}'\n"
        f"        stateDir: '{state}'\n"
        f"        dshHome: '{home}'\n"
        "        timeoutMs: 30000\n"
    )

    env.update(env_keys)
    dsh = ["node", str(Path(args.dsh).resolve())]

    overlays += ["--patch", str(patch)]
    for extra_patch in args.extra_patch:
        overlays += ["--patch", extra_patch]

    def headless(*extra, task=None):
        return run(
            [*dsh, "--profile", "headless", *overlays, "--json", *extra, task or args.task],
            project,
            env,
        )

    record = {"root": str(root), "runs": []}

    # 1. Registration reads one session projection, which only exists after at
    # least one run. The bridge captures that run; it is the only turn that
    # predates the epoch, so registration's baseline excludes it.
    engine = [str(engine_binary), "--state-dir", str(state), "dsh", "--dsh-home", str(home)]
    record["engine"] = [str(engine_binary), str(state), str(home)]

    first = headless()
    record["runs"].append({"name": "baseline", "exit": first.returncode, "stream": first.stdout.splitlines()})
    record["first_stderr"] = first.stderr.splitlines()[-20:]
    time.sleep(2)
    probe = run([str(engine_binary), "--state-dir", str(state), "status"], project)
    record["status_after_first"] = (probe.stdout or probe.stderr).strip()[:400]
    time.sleep(3)
    # Locate the projection of the session this run actually executed: a stale
    # file from an earlier run would name a different session's turn.
    first_session = next(
        (line["sessionId"] for line in map(json.loads, first.stdout.splitlines()) if line.get("type") == "session"),
        None,
    )
    # The bridge publishes one immutable file per captured turn.
    sessions = sorted((state / "dsh" / "projections").glob(f"{first_session}.*.json")) if first_session else []
    record["projection_names"] = [path.name for path in sessions]
    record["projected_sessions"] = len(sessions)
    if sessions:
        reg = run([*engine, "register", "--session", str(sessions[-1]), "--project", str(project)], project)
        record["register"] = json.loads(reg.stdout) if reg.returncode == 0 else {"error": reg.stderr}
        # Hints recorded before registration are queued by reconciliation.
        rec = run([*engine, "reconcile", "--session", str(sessions[-1]), "--project", str(project)], project)
        record["reconcile"] = json.loads(rec.stdout) if rec.returncode == 0 else {"error": rec.stderr}

    # 2. A session after registration is queued by the bridge hint. The bridge
    # can only hint a registered project, so the session must start in a later
    # second than the registration that enables its project. `--resume` continues
    # the first session in a new process, which is a distinct DSH lifecycle.
    time.sleep(1.1)
    second = headless("--session-id", first_session, task="Reply with: resumed") if args.resume and first_session else headless()
    record["runs"].append({"name": "resume" if args.resume else "after", "exit": second.returncode, "stream": second.stdout.splitlines()})
    record["second_stderr"] = second.stderr.splitlines()[-20:]
    time.sleep(3)
    probe = run([str(engine_binary), "--state-dir", str(state), "status"], project)
    record["status_after_second"] = (probe.stdout or probe.stderr).strip()[:400]
    # The bridge captures right after `turn/end`; allow the child process to
    # finish before reading status.
    time.sleep(3)

    status = run([str(engine_binary), "--state-dir", str(state), "status"], project)
    record["status"] = json.loads(status.stdout) if status.returncode == 0 else status.stderr

    projections = sorted((state / "dsh" / "projections").glob("*.json"))
    record["latest_projection"] = None
    if projections:
        document = json.loads(projections[-1].read_text())
        types = {}
        for row in document["records"]:
            types[row["type"]] = types.get(row["type"], 0) + 1
        record["latest_projection"] = {
            "header": {k: v for k, v in document["header"].items() if k in ("version", "isSeeded", "delegationDepth")},
            "records": len(document["records"]),
            "types": types,
            "turn_ends": [
                {"turn": r["data"]["turn"], "reason": r["data"]["reason"]["kind"]}
                for r in document["records"] if r["type"] == "turn/end"
            ],
            "human_messages": [
                r["seq"] for r in document["records"]
                if r["type"] == "user/message" and r["data"]["source"]["kind"] == "user"
            ],
        }

    print(json.dumps(record, indent=2))
    if not args.keep:
        shutil.rmtree(root, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
