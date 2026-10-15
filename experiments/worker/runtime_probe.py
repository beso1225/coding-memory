"""Isolated worker and launchd lifecycle probe with a synthetic loopback model."""
import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", type=Path, required=True)
    parser.add_argument("--codex", default=shutil.which("codex"))
    parser.add_argument("--launchd", action="store_true")
    args = parser.parse_args()
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "codex_capture"))
    import runtime as rt
    subprocess.run(["git", "init", str(rt.WORK)], check=True, capture_output=True)
    state = rt.ROOT / "engine"
    vault = rt.ROOT / "vault"
    vault.mkdir()
    engine = str(args.engine.resolve())
    binary = str(Path(args.codex).resolve())
    base = [engine, "--state-dir", str(state)]
    def run(command, *options, check=True):
        return subprocess.run(base + [command, *options], capture_output=True, text=True, check=check, timeout=90)
    source = ["--codex-home", str(rt.HOME_DIR), "--codex-binary", binary, "--project", str(rt.WORK)]
    project = run("register", *source).stdout.strip()
    run("external-policy", "--project-id", project, "allow")
    time.sleep(2)
    client = rt.Client(binary)
    rt.PLANS.put({"text": "Acknowledged the synthetic SQLite WAL constraint."})
    thread = client.call("thread/start", {"cwd": str(rt.WORK), "model": "capture-test", "approvalPolicy": "never"})["thread"]["id"]
    turn = client.call("turn/start", {"threadId": thread, "input": [{"type": "text", "text": "Synthetic established project constraint: SQLite WAL lease tokens advance by seven."}]})["turn"]["id"]
    client.terminal(turn)
    client.close()
    def input_json(value):
        if isinstance(value, str) and "INPUT_JSON\n" in value:
            try:
                return json.loads(value.split("INPUT_JSON\n")[-1])
            except ValueError:
                pass
        if isinstance(value, dict):
            value = value.values()
        if isinstance(value, (list, tuple)) or type(value).__name__ == "dict_values":
            for item in value:
                result = input_json(item)
                if result is not None:
                    return result
        return None
    def extraction(body):
        data = input_json(body)
        assert data is not None, "Closed extraction input missing"
        bundle = data["evidence"]
        item = next(i["id"] for i in bundle["items"] if i["type"] == "userMessage")
        c = {"kind": "constraint", "title": "SQLite WAL token protocol", "claim": "Lease tokens advance by seven.", "scope": {"kind": "project", "project_id": project}, "conditions": ["This synthetic SQLite WAL project"], "verification": "user_stated", "evidence": [{"bundle": data["bundle"], "item": item}]}
        return {"text": json.dumps({"schema_version": 1, "role": "extraction", "project_id": project, "candidates": [c]})}
    def consolidation(body):
        data = input_json(body)
        staged = data["batch"]["candidates"][0]
        c = staged["candidate"]
        draft = {k: c[k] for k in ["title", "scope", "conditions", "verification", "evidence"]}
        draft.update(summary=c["claim"], body=c["claim"], related=[])
        return {"text": json.dumps({"schema_version": 1, "role": "consolidation", "operations": [{"id": "op1", "action": "create", "candidates": [staged["id"]], "target": None, "draft": draft, "dependencies": [], "rationale": "Explicit fixture constraint"}]})}
    rt.PLANS.put(extraction)
    rt.PLANS.put(consolidation)
    rt.PLANS.put({"text": json.dumps({"schema_version": 1, "role": "review", "decisions": [{"operation": "op1", "decision": "accept", "rationale": "Explicit fixture scope and support preserved"}]})})
    config = {"schema_version": 1, "vault": str(vault), "sources": [{"project": str(rt.WORK), "codex_home": str(rt.HOME_DIR), "codex_binary": binary}], "generation_binary": binary, "models": {"extraction": "capture-extraction", "consolidation": "capture-consolidation", "review": "capture-review"}, "poll_seconds": 2, "timeout_seconds": 30, "min_ready": 1, "max_age_seconds": 600, "loopback": f"127.0.0.1:{rt.SERVER.server_port}"}
    config_path = rt.ROOT / "worker.json"
    config_path.write_text(json.dumps(config))
    first = run("worker", "--config", str(config_path), "--once")
    status = json.loads(run("worker-status").stdout)
    assert status["counts"]["runs"] == {"ready": 1}, status
    assert status["counts"]["extraction_jobs"] == {"succeeded": 1}, status
    assert status["counts"]["candidates"] == {"consumed": 1}, status
    assert not status["active"], status
    start = len(rt.EVENTS)
    run("worker", "--config", str(config_path), "--once")
    assert len(rt.EVENTS) == start, "Restart generated duplicate work"
    log = rt.ROOT / "worker.log"
    with log.open("w") as output:
        worker = subprocess.Popen(base + ["worker", "--config", str(config_path)], stdout=output, stderr=output)
        try:
            for _ in range(100):
                if json.loads(run("worker-status").stdout)["active"]:
                    break
                time.sleep(.1)
            assert worker.poll() is None
            duplicate = run("worker", "--config", str(config_path), "--once", check=False)
            assert duplicate.returncode != 0 and "Another worker" in duplicate.stderr, duplicate.stderr
            initial_cycle = json.loads(run("worker-status").stdout)["last_cycle"]["started_at"]
            for _ in range(300):
                status = json.loads(run("worker-status").stdout)
                checks = status["runtime_checks"]
                if any(c["last_result"] == "passed" for c in checks):
                    break
                assert not any(c["last_result"] == "failed" for c in checks), checks
                time.sleep(.1)
            assert any(c["last_result"] == "passed" for c in checks), status
            assert status["last_cycle"]["started_at"] > initial_cycle, status
        finally:
            worker.send_signal(signal.SIGTERM)
            worker.wait(timeout=10)
    assert worker.returncode == 0
    assert not json.loads(run("worker-status").stdout)["active"]
    launched = False
    if args.launchd:
        label = "com.beso1225.coding-memory-test-" + str(os.getpid())
        plist = rt.ROOT / "worker.plist"
        run("service-plist", "--config", str(config_path), "--output", str(plist), "--label", label)
        subprocess.run(["/usr/bin/plutil", "-lint", str(plist)], check=True, capture_output=True)
        domain = "gui/" + str(os.getuid())
        try:
            subprocess.run(["/bin/launchctl", "bootstrap", domain, str(plist)], check=True, capture_output=True)
            launched = True
            for _ in range(150):
                status = json.loads(run("worker-status").stdout)
                if status["active"] and status["last_cycle"]["completed_at"] is not None:
                    break
                time.sleep(.1)
            assert status["active"], "launchd did not acquire worker ownership"
            assert not status["last_cycle"]["failures"], status
        finally:
            if launched:
                subprocess.run(["/bin/launchctl", "bootout", domain + "/" + label], check=True, capture_output=True)
    print(json.dumps({"runtime_version": subprocess.check_output([binary, "--version"], text=True).strip(), "first_cycle": json.loads(first.stdout), "restart_deduplicated": True, "duplicate_worker_rejected": True, "background_runtime_verified": True, "cycles_progressed_during_verification": True, "sigterm_released_ownership": True, "launchd_bootstrap_passed": launched, "state_directory": str(state)}, indent=2))


if __name__ == "__main__":
    main()
