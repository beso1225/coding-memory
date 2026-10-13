"""Opt-in installed CLI probe with a synthetic loopback provider."""

import json
import os
import pathlib
import subprocess
import sys


def main(binary):
    sys.path.insert(
        0, str(pathlib.Path(__file__).resolve().parents[1] / "codex_capture")
    )
    import runtime as p
    from contract import SCHEMA, classify, is_internal, permitted_call
    from probe_support import install_policy

    schema = p.ROOT / "schema.json"
    schema.write_text(json.dumps(SCHEMA))
    (p.HOME_DIR / "config.toml").write_text(
        (p.HOME_DIR / "config.toml").read_text()
        + f'\n[mcp_servers.probe]\ncommand = {json.dumps(sys.executable)}\nargs = ["-c", "from pathlib import Path; Path({str(p.ROOT / "mcp-started")!r}).touch()"]\n'
    )
    results = []
    for name, role, answer in [
        (
            "extract",
            "extraction",
            {"role": "extraction", "items": ["synthetic knowledge"], "approved": True},
        ),
        ("empty", "extraction", {"role": "extraction", "items": [], "approved": True}),
        (
            "consolidate",
            "consolidation",
            {
                "role": "consolidation",
                "items": ["synthetic proposal"],
                "approved": True,
            },
        ),
        ("review", "review", {"role": "review", "items": [], "approved": False}),
        ("malformed", "review", "not JSON"),
        ("failure", "review", None),
        ("tools", "review", {"role": "review", "items": [], "approved": True}),
        ("minimized", "review", {"role": "review", "items": [], "approved": True}),
        ("guarded", "review", {"role": "review", "items": [], "approved": True}),
    ]:
        model = "capture-" + role
        if name == "failure":
            p.PLANS.put("fail")
        elif name in ("tools", "minimized", "guarded"):
            p.PLANS.put(
                {
                    "items": [
                        {
                            "type": "function_call",
                            "name": "exec_command",
                            "call_id": "probe-tool",
                            "arguments": json.dumps(
                                {
                                    "cmd": "printf synthetic-generation-tool",
                                    "yield_time_ms": 1000,
                                }
                            ),
                        }
                    ]
                }
            )
            p.PLANS.put({"text": json.dumps(answer)})
        else:
            p.PLANS.put(
                {"text": json.dumps(answer) if isinstance(answer, dict) else answer}
            )
        output = p.ROOT / (name + ".json")
        start = len(p.EVENTS)
        args = [
            binary,
            "exec",
            "--skip-git-repo-check",
            "--ignore-user-config",
            "--ignore-rules",
            "--json",
            "--sandbox",
            "read-only",
            "--disable",
            "hooks",
            "--disable",
            "plugins",
            "-c",
            "notify=[]",
            "-c",
            'model_provider="mock"',
            "-c",
            f'model_providers.mock={{name="Loopback",base_url="http://127.0.0.1:{p.SERVER.server_port}/v1",wire_api="responses",requires_openai_auth=false,request_max_retries=0,stream_max_retries=0}}',
            "--model",
            model,
            "--output-schema",
            str(schema),
            "-o",
            str(output),
            f"Synthetic {role} role. Return only the specified schema.",
        ]
        if name == "minimized":
            args[1:1] = ["--disable", "shell_tool", "--disable", "unified_exec"]
        if name == "guarded":
            install_policy(p.ROOT, p.HOME_DIR)
            args[1:1] = ["--enable", "hooks", "--dangerously-bypass-hook-trust"]
            # Remove the later conflicting disable flag for this dedicated policy home.
            hook_disable = args.index("hooks", args.index("exec"))
            del args[hook_disable - 1 : hook_disable + 1]
        process = permitted_call(
            True,
            lambda args=args: subprocess.run(
                args,
                cwd=p.WORK,
                env={**os.environ, "CODEX_HOME": str(p.HOME_DIR)},
                capture_output=True,
                check=False,
                text=True,
                timeout=20,
            ),
        )
        events = [json.loads(line) for line in process.stdout.splitlines()]
        tid = next(e["thread_id"] for e in events if e["type"] == "thread.started")
        final = output.read_text() if output.exists() else ""
        results.append(
            {
                "case": name,
                "role": role,
                "model": model,
                "thread_id": tid,
                "exit": process.returncode,
                "outcome": classify(role, process.returncode, final),
                "final": final,
                "internal_live": is_internal(
                    {"store": "generation", "thread": tid}, {"generation"}, set()
                ),
                "internal_reconciled": is_internal(
                    {"store": "generation", "thread": tid}, {"generation"}, set()
                ),
                "requests": p.EVENTS[start:],
                "tool_items": [
                    e
                    for e in events
                    if e.get("item", {}).get("type") == "command_execution"
                ],
            }
        )
    p.PLANS.put({"delay": 3, "text": "late answer"})
    timeout_args = args.copy()
    timeout_args[timeout_args.index("-o") + 1] = str(p.ROOT / "timeout.json")
    try:
        subprocess.run(
            timeout_args,
            cwd=p.WORK,
            env={**os.environ, "CODEX_HOME": str(p.HOME_DIR)},
            capture_output=True,
            check=False,
            text=True,
            timeout=1,
        )
        timeout_outcome = "unexpected_success"
    except subprocess.TimeoutExpired:
        timeout_outcome = "timeout"
    fixture = {
        "timeout_outcome": timeout_outcome,
        "version": subprocess.check_output([binary, "--version"], text=True).strip(),
        "results": results,
        "hook_log_exists": (p.ROOT / "hooks.jsonl").exists(),
        "mcp_started": (p.ROOT / "mcp-started").exists(),
        "policy_called": (p.ROOT / "policy.jsonl").exists(),
        "policy_events": [
            json.loads(line)
            for line in (p.ROOT / "policy.jsonl").read_text().splitlines()
        ]
        if (p.ROOT / "policy.jsonl").exists()
        else [],
        "persisted_native_ids": [
            json.loads(path.read_text().splitlines()[0])["payload"]["id"]
            for path in p.HOME_DIR.rglob("rollout*.jsonl")
        ],
        "persisted_rollouts": len(list(p.HOME_DIR.rglob("rollout*.jsonl"))),
    }
    (p.ROOT / "generation-result.json").write_text(json.dumps(fixture, indent=2))
    print(p.ROOT)
    print(json.dumps({r["case"]: r["outcome"] for r in results}))
    print("hooks", fixture["hook_log_exists"], "mcp", fixture["mcp_started"])


if __name__ == "__main__":
    main(sys.argv[1])
