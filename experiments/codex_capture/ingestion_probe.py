"""Exercise the Rust ingestion CLI with isolated Codex and a loopback model."""

import argparse
import json
import shutil
import subprocess
import time
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", type=Path, required=True)
    parser.add_argument("--codex", default=shutil.which("codex"))
    args = parser.parse_args()
    # The probe runtime creates a fresh private directory and synthetic provider.
    import runtime as rt

    subprocess.run(["git", "init", str(rt.WORK)], check=True, capture_output=True)
    state = rt.ROOT / "engine"
    base = [str(args.engine.resolve()), "--state-dir", str(state)]
    options = [
        "--codex-home",
        str(rt.HOME_DIR),
        "--codex-binary",
        args.codex,
        "--project",
        str(rt.WORK),
    ]

    def run(command, runtime=True):
        return subprocess.run(
            base + [command] + (options if runtime else []),
            check=True,
            text=True,
            capture_output=True,
        ).stdout

    run("register")
    # Stored timestamps have second resolution; collection excludes the enablement second.
    time.sleep(2)
    client = rt.Client(args.codex)
    try:
        rt.PLANS.put({"text": "bounded synthetic knowledge"})
        thread = client.call(
            "thread/start",
            {
                "cwd": str(rt.WORK),
                "model": "capture-test",
                "approvalPolicy": "never",
            },
        )["thread"]["id"]
        turn = client.call(
            "turn/start",
            {
                "threadId": thread,
                "input": [{"type": "text", "text": "synthetic ingestion fixture only"}],
            },
        )["turn"]["id"]
        client.terminal(turn)
        assert run("reconcile").strip() == "1"
        assert run("reconcile").strip() == "0"
        run("capture")
        rows = json.loads(run("status", runtime=False))
        assert len(rows) == 1 and rows[0]["state"] == "ready", rows
        assert rows[0]["key"]["turn"] == turn
        print(json.dumps({"state_directory": str(state), "runs": rows}, indent=2))
    finally:
        client.close()


if __name__ == "__main__":
    main()
