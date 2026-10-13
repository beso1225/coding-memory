"""Cold-read an explicitly selected synthetic App thread without resuming it."""

import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import threading


def read_thread(binary, project, codex_home, thread_id, output):
    messages = queue.Queue()
    notifications = []
    with output.with_suffix(".stderr").open("w") as stderr:
        process = subprocess.Popen(
            [
                binary,
                "-c",
                "notify=[]",
                "--disable",
                "hooks",
                "--disable",
                "plugins",
                "app-server",
            ],
            cwd=project,
            env={**os.environ, "CODEX_HOME": str(codex_home)},
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=stderr,
            text=True,
        )

        def receive():
            for line in process.stdout:
                messages.put(json.loads(line))

        threading.Thread(target=receive, daemon=True).start()

        def call(ident, method, params):
            process.stdin.write(
                json.dumps({"id": ident, "method": method, "params": params}) + "\n"
            )
            process.stdin.flush()
            while True:
                message = messages.get(timeout=30)
                if message.get("id") == ident:
                    if "error" in message:
                        raise RuntimeError(message["error"])
                    return message["result"]
                notifications.append(message)

        try:
            call(
                1,
                "initialize",
                {
                    "clientInfo": {
                        "name": "coding_memory_app_capture_reader",
                        "version": "0.1",
                    },
                    "capabilities": {"experimentalApi": True},
                },
            )
            process.stdin.write(
                json.dumps({"method": "initialized", "params": {}}) + "\n"
            )
            process.stdin.flush()
            history = call(
                2, "thread/read", {"threadId": thread_id, "includeTurns": True}
            )
            if Path(history["thread"]["cwd"]).resolve() != project.resolve():
                raise ValueError(
                    "Selected thread does not belong to the synthetic project"
                )
            if any(n.get("method") == "thread/started" for n in notifications):
                raise ValueError("Reader unexpectedly loaded a thread")
            last_turn = history["thread"]["turns"][-1]
            items = call(
                3,
                "thread/items/list",
                {
                    "threadId": thread_id,
                    "turnId": last_turn["id"],
                    "limit": 100,
                },
            )
            result = {
                "read": history,
                "items": items,
                "thread_started_observed": False,
                "requested_methods": ["initialize", "thread/read", "thread/items/list"],
            }
            output.write_text(json.dumps(result, indent=2) + "\n")
            print(
                json.dumps(
                    {
                        "thread_id": thread_id,
                        "state": history["thread"]["status"],
                        "turn_id": last_turn["id"],
                        "status": last_turn["status"],
                        "completedAt": last_turn["completedAt"],
                        "output": str(output),
                    }
                )
            )
        finally:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary")
    parser.add_argument("thread_id")
    parser.add_argument("--project", type=Path, required=True)
    parser.add_argument("--codex-home", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    read_thread(args.binary, args.project, args.codex_home, args.thread_id, args.output)
