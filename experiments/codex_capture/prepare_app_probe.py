"""Prepare a disposable App project; do not edit the user's Codex configuration."""

import json
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile


root = Path(tempfile.mkdtemp(prefix="coding-memory-app-probe-", dir="/private/tmp"))
project = root / "project"
config = project / ".codex"
config.mkdir(parents=True)
hook = root / "record.py"
hook.write_text("""import json, pathlib, sys, time
root = pathlib.Path(sys.argv[1])
event = json.loads(sys.argv[2]) if len(sys.argv) > 2 else json.load(sys.stdin)
cwd = pathlib.Path(event.get("cwd", "")).resolve()
if cwd == (root / "project").resolve():
    fields = ("type", "hook_event_name", "session_id", "turn_id", "thread-id",
              "turn-id", "cwd", "transcript_path", "model", "permission_mode", "stop_hook_active")
    record = {key: event[key] for key in fields if key in event}
    record["observed_at_ns"] = time.time_ns()
    with (root / "events.jsonl").open("a") as stream:
        stream.write(json.dumps(record) + "\\n")
""")
argv = [sys.executable, str(hook), str(root)]
(config / "config.toml").write_text(
    "notify = " + json.dumps(argv) + "\n[features]\nhooks = true\n"
)
(config / "hooks.json").write_text(
    json.dumps(
        {
            "hooks": {
                event: [
                    {
                        "hooks": [
                            {
                                "type": "command",
                                "command": shlex.join(argv),
                                "timeout": 3,
                            }
                        ]
                    }
                ]
                for event in (
                    "SessionStart",
                    "UserPromptSubmit",
                    "Stop",
                    "Interrupt",
                    "PostToolUse",
                )
            }
        },
        indent=2,
    )
    + "\n"
)
(project / "README.md").write_text(
    "# Synthetic App Capture Probe\n\nNo personal project files belong here.\n"
)
subprocess.run(["git", "init", "--quiet", str(project)], check=True)
print(
    json.dumps(
        {"project": str(project), "events": str(root / "events.jsonl")}, indent=2
    )
)
