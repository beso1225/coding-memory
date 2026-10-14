"""Opt-in Rust extraction round trip using synthetic capture and a loopback model."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", type=Path, required=True)
    parser.add_argument("--codex", default=shutil.which("codex"))
    args = parser.parse_args()
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "codex_capture"))
    import runtime as rt

    subprocess.run(["git", "init", str(rt.WORK)], check=True, capture_output=True)
    state = rt.ROOT / "engine"
    base = [str(args.engine.resolve()), "--state-dir", str(state)]
    runtime = ["--codex-home", str(rt.HOME_DIR), "--codex-binary", args.codex, "--project", str(rt.WORK)]

    def run(command, *options):
        return subprocess.run(base + [command, *options], check=True, text=True, capture_output=True, timeout=45).stdout

    project = run("register", *runtime).strip()
    time.sleep(2)
    client = rt.Client(args.codex)
    results = []
    try:
        thread = client.call("thread/start", {"cwd": str(rt.WORK), "model": "capture-test", "approvalPolicy": "never"})["thread"]["id"]
        for case in ["supported", "empty", "guarded"]:
            rt.PLANS.put({"text": "Acknowledged the synthetic constraint."})
            turn = client.call("turn/start", {"threadId": thread, "input": [{"type": "text", "text": "Synthetic project constraint: use token-fenced leases for SQLite jobs."}]})["turn"]["id"]
            client.terminal(turn)
            run("reconcile", *runtime)
            run("capture", *runtime)
            row = next(r for r in json.loads(run("status")) if r["key"]["turn"] == turn)
            assert row["state"] == "ready", row
            bundle = json.loads(run("evidence", row["evidence"]))
            item = next(i["id"] for i in bundle["items"] if i["type"] == "userMessage")
            output = {"schema_version": 1, "role": "extraction", "project_id": project, "candidates": []}
            if case == "supported":
                output["candidates"] = [{"kind": "constraint", "title": "Fence SQLite job leases", "claim": "Use token-fenced leases for SQLite jobs.", "scope": {"kind": "project", "project_id": project}, "conditions": ["This synthetic project uses SQLite jobs"], "verification": "user_stated", "evidence": [{"bundle": row["evidence"], "item": item}]}]
            options = ["--project-id", project, "--codex-binary", args.codex, "--model", "capture-extraction", "--timeout-seconds", "30", "--loopback", f"127.0.0.1:{rt.SERVER.server_port}"]
            if case == "supported":
                start = len(rt.EVENTS)
                run("extract", *options)
                assert len(rt.EVENTS) == start, "Disallowed project reached the model"
                assert not (state / "generation").exists(), "Disallowed project prepared credentials"
                run("external-policy", "--project-id", project, "allow")
            if case == "guarded":
                sentinel = rt.ROOT / "tool-must-not-run"
                rt.PLANS.put({"items": [{"type": "function_call", "name": "exec_command", "call_id": "denied-tool", "arguments": json.dumps({"cmd": f"touch {sentinel}", "yield_time_ms": 1000})}]})
            rt.PLANS.put({"text": json.dumps(output)})
            rows = json.loads(run("extract", *options))
            extracted = next(r for r in rows if r["key"]["turn"] == turn)
            expected = {"supported": "succeeded", "empty": "succeeded_empty", "guarded": "held"}[case]
            assert extracted["state"] == expected, extracted
            if case == "guarded":
                assert extracted["reason"] == "generation_tool_attempt", extracted
                assert not sentinel.exists(), "PreToolUse failed to block command execution"
            results.append({"case": case, "state": extracted["state"], "reason": extracted["reason"]})
        candidates = json.loads(run("candidates", "--project-id", project))
        assert len(candidates) == 1 and candidates[0]["candidate"]["conditions"] == ["This synthetic project uses SQLite jobs"], candidates
        home = state / "generation/codex-home"
        assert not (home / "auth.json").exists(), "Synthetic probe created credentials"
        owned = subprocess.run(base + ["reconcile", "--codex-home", str(home), "--codex-binary", args.codex, "--project", str(rt.WORK)], check=True, capture_output=True, text=True).stdout
        assert owned.strip() == "0", owned
        print(json.dumps({"version": subprocess.check_output([args.codex, "--version"], text=True).strip(), "results": results, "candidate_count": len(candidates), "owned_reconciliation": 0, "state_directory": str(state)}, indent=2))
    finally:
        client.close()


if __name__ == "__main__":
    main()
