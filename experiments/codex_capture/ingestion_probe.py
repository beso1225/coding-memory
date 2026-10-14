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
    parser.add_argument("--turns", type=int, choices=range(1, 5), default=1)
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
        expected = {}
        for sequence in range(args.turns):
            if sequence:
                rt.PLANS.put({"text": "bounded synthetic knowledge " + str(sequence)})
            turn = client.call(
                "turn/start",
                {
                    "threadId": thread,
                    "input": [{"type": "text", "text": "synthetic ingestion fixture " + str(sequence)}],
                },
            )["turn"]["id"]
            client.terminal(turn)
            assert run("reconcile").strip() == "1"
            assert run("reconcile").strip() == "0"
            run("capture")
            rows = json.loads(run("status", runtime=False))
            expected[turn] = "synthetic ingestion fixture " + str(sequence)
            assert len(rows) == sequence + 1 and all(r["state"] == "ready" for r in rows), rows
            assert {r["key"]["turn"] for r in rows} == set(expected)
            for row in rows:
                evidence = json.loads((state / "evidence" / (row["evidence"] + ".json")).read_text())
                assert evidence["run"]["turn"] == row["key"]["turn"]
                users = [i for i in evidence["items"] if i["type"] == "userMessage"]
                assert len(users) == 1
                assert users[0]["content"][0]["text"] == expected[row["key"]["turn"]]
        print(json.dumps({"state_directory": str(state), "runs": rows}, indent=2))
    finally:
        client.close()


if __name__ == "__main__":
    main()
