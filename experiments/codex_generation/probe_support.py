"""Shared policy, credential lifecycle, and subprocess results for opt-in probes."""

import hashlib
import json
import os
import shlex
import shutil
import subprocess
import sys
from contextlib import contextmanager

from contract import classify


def install_policy(root, home):
    guard = root / "deny_tools.py"
    guard.write_text("""import json, pathlib, sys
record = json.load(sys.stdin)
with pathlib.Path(sys.argv[1]).open('a') as stream:
    stream.write(json.dumps(record) + '\\n')
print('Generation tool execution denied', file=sys.stderr)
sys.exit(2)
""")
    (home / "hooks.json").write_text(
        json.dumps(
            {
                "hooks": {
                    "PreToolUse": [
                        {
                            "matcher": ".*",
                            "hooks": [
                                {
                                    "type": "command",
                                    "command": shlex.join(
                                        [
                                            sys.executable,
                                            str(guard),
                                            str(root / "policy.jsonl"),
                                        ]
                                    ),
                                    "timeout": 3,
                                }
                            ],
                        }
                    ]
                }
            }
        )
    )


@contextmanager
def credential_copy(source, destination, refreshed):
    created = False
    original = None
    try:
        with source.open("rb") as incoming:
            fd = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            created = True
            with os.fdopen(fd, "wb") as outgoing:
                shutil.copyfileobj(incoming, outgoing)
        original = hashlib.sha256(destination.read_bytes()).digest()
        yield
    finally:
        if created and destination.exists():
            changed = (
                original is not None
                and hashlib.sha256(destination.read_bytes()).digest() != original
            )
            if changed:
                # Preserve refreshed tokens privately; never overwrite the user's login.
                destination.chmod(0o600)
                os.replace(destination, refreshed)
            else:
                destination.unlink()


def execute_role(args, role, model, home, work, output, timeout=90):
    if output.exists():
        raise ValueError("Each execution requires a fresh output path")
    try:
        process = subprocess.run(
            args,
            cwd=work,
            env={**os.environ, "CODEX_HOME": str(home)},
            capture_output=True,
            check=False,
            text=True,
            timeout=timeout,
        )
    except OSError as error:
        return {
            "role": role,
            "model": model,
            "exit": None,
            "outcome": "process_failure",
            "thread_id": None,
            "output": None,
            "tool_items": [],
            "error_kind": type(error).__name__,
        }
    except subprocess.TimeoutExpired:
        return {
            "role": role,
            "model": model,
            "exit": None,
            "outcome": "timeout",
            "thread_id": None,
            "output": None,
            "tool_items": [],
        }
    events = []
    malformed = False
    for line in process.stdout.splitlines():
        try:
            event = json.loads(line)
            if not isinstance(event, dict):
                malformed = True
                continue
            if "item" in event and not isinstance(event["item"], dict):
                malformed = True
                continue
            events.append(event)
        except ValueError:
            malformed = True
    final = output.read_text() if output.exists() else ""
    outcome = classify(role, process.returncode, final)
    if malformed and process.returncode == 0:
        outcome = "invalid_output"
    try:
        value = json.loads(final)
    except ValueError:
        value = None
    return {
        "role": role,
        "model": model,
        "exit": process.returncode,
        "outcome": outcome,
        "thread_id": next(
            (e.get("thread_id") for e in events if e.get("type") == "thread.started"),
            None,
        ),
        "output": value,
        "tool_items": [
            e
            for e in events
            if e.get("item", {}).get("type")
            in [
                "command_execution",
                "mcp_tool_call",
                "web_search",
                "file_change",
                "collab_tool_call",
            ]
        ],
    }
