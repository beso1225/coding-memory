import json, os, pathlib, shutil, subprocess, tempfile, argparse, sys

parser = argparse.ArgumentParser()
parser.add_argument("--allow-external", action="store_true")
args = parser.parse_args()
if not args.allow_external:
    print("held_external_policy")
    sys.exit(0)
root = pathlib.Path(
    tempfile.mkdtemp(prefix="coding-memory-generation-live-", dir="/private/tmp")
)
home = root / "state"
home.mkdir(mode=0o700)
work = root / "project"
work.mkdir()
guard = root / "guard.py"
guard.write_text(
    "import json,sys,pathlib\nevent=json.load(sys.stdin)\nwith pathlib.Path("
    + repr(str(root / "policy.jsonl"))
    + ").open('a') as f:f.write(json.dumps(event)+'\\n')\nprint('Generation tools are disabled',file=sys.stderr)\nsys.exit(2)\n"
)
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
                                "command": f"/opt/homebrew/bin/python3 {guard}",
                                "timeout": 3,
                            }
                        ],
                    }
                ]
            }
        }
    )
)
schema = {
    "type": "object",
    "properties": {
        "role": {"type": "string", "enum": ["extraction", "consolidation", "review"]},
        "items": {"type": "array", "items": {"type": "string"}},
        "approved": {"type": "boolean"},
    },
    "required": ["role", "items", "approved"],
    "additionalProperties": False,
}
(root / "schema.json").write_text(json.dumps(schema))
shutil.copyfile(str(pathlib.Path.home() / ".codex/auth.json"), home / "auth.json")
os.chmod(home / "auth.json", 0o600)
results = []
try:
    for role in ["extraction", "consolidation", "review"]:
        out = root / (role + ".json")
        prompt = f"You are the {role} role in a synthetic protocol test. Use no tools. Evidence: a synthetic test reported PASS for fixture E1. Return role={role}, items=[] and approved=false. Do not infer any reusable knowledge beyond that evidence."
        args = [
            "/opt/homebrew/bin/codex",
            "exec",
            "--skip-git-repo-check",
            "--ignore-user-config",
            "--ignore-rules",
            "--json",
            "--sandbox",
            "read-only",
            "--enable",
            "hooks",
            "--disable",
            "plugins",
            "--dangerously-bypass-hook-trust",
            "-c",
            "notify=[]",
            "-c",
            "project_doc_max_bytes=0",
            "-c",
            'web_search="disabled"',
            "-c",
            'cli_auth_credentials_store="file"',
            "--model",
            "gpt-6.1-sol",
            "--output-schema",
            str(root / "schema.json"),
            "-o",
            str(out),
            prompt,
        ]
        r = subprocess.run(
            args,
            cwd=work,
            env={**os.environ, "CODEX_HOME": str(home)},
            capture_output=True,
            text=True,
            timeout=90,
        )
        events = [json.loads(x) for x in r.stdout.splitlines()]
        results.append(
            {
                "role": role,
                "model": "gpt-6.1-sol",
                "exit": r.returncode,
                "thread_id": next(
                    (e["thread_id"] for e in events if e["type"] == "thread.started"),
                    None,
                ),
                "output": json.loads(out.read_text()) if out.exists() else None,
                "tool_items": [
                    e
                    for e in events
                    if e.get("item", {}).get("type")
                    in ["command_execution", "mcp_tool_call", "web_search"]
                ],
            }
        )
        print(role, r.returncode, flush=True)
finally:
    (home / "auth.json").unlink(missing_ok=True)
(root / "result.json").write_text(
    json.dumps(
        {
            "roles": results,
            "credential_copy_removed": not (home / "auth.json").exists(),
            "policy_calls": (root / "policy.jsonl").exists(),
        },
        indent=2,
    )
    + "\n"
)
print(root)
