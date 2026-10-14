"""Opt-in real-model evaluation of synthetic evidence and separate-session reuse.
Never copies authentication. Requires an independently logged-in Engine home.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import time
import re
from quality_checks import token_rule, applicable_wal, retrieved_rule


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", type=Path, required=True)
    parser.add_argument("--codex", default=shutil.which("codex"))
    parser.add_argument("--state-dir", type=Path, required=True)
    parser.add_argument("--model", default="gpt-6.1-sol")
    parser.add_argument("--allow-external", action="store_true")
    args = parser.parse_args()
    if not args.allow_external:
        parser.error("Real-model evaluation requires explicit --allow-external")
    state = args.state_dir.resolve()
    auth_home = state / "generation/codex-home"
    if not (auth_home / "auth.json").is_file() or (auth_home / "auth.json").is_symlink():
        parser.error("Independent Engine login required; credentials are never copied")
    database = state / "state.sqlite"
    if database.exists():
        with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as db:
            if db.execute("SELECT count(*) FROM projects").fetchone()[0]:
                parser.error("Use a fresh evaluation state with no registered projects")
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "codex_capture"))
    import runtime as rt
    engine = str(args.engine.resolve())
    base = [engine, "--state-dir", str(state)]
    source = ["--codex-home", str(rt.HOME_DIR), "--codex-binary", args.codex, "--project", str(rt.WORK)]
    subprocess.run(["git", "init", str(rt.WORK)], check=True, capture_output=True)
    vault = rt.ROOT / "vault"
    vault.mkdir()
    def run(command, *options):
        return subprocess.run(base + [command, *options], text=True, capture_output=True, check=True, timeout=420).stdout
    project = run("register", *source).strip()
    run("external-policy", "--project-id", project, "allow")
    time.sleep(2)
    cases = [
        ("supported", "Established constraint for this synthetic SQLite WAL queue: each claim or reclamation increments lease_token by exactly seven (7), because the external protocol reserves the six intervening values. Acknowledgment must match job_id, the current lease_token, and state=running in one UPDATE. A stale token or already-done job must affect zero rows and return false. This applies only to this project's SQLite WAL queue.", "The explicit SQLite WAL protocol requires a token increment of seven and a fenced acknowledgment."),
        ("progress_only", "Give a progress report only; no conclusions or reusable findings have been established.", "I opened two files and started investigating. Work is still in progress."),
        ("unsupported", "Brainstorm an unverified hypothesis; no tests or measurements have been run. Do not adopt it as a fact.", "Maybe disabling SQLite journaling prevents corruption. This is speculation without evidence."),
    ]
    report = {"runtime_version": subprocess.check_output([args.codex, "--version"], text=True).strip(), "model": args.model, "corpus": [], "state_directory": str(state)}
    for name, prompt, answer in cases:
        print("Evaluating synthetic case: " + name, flush=True)
        rt.PLANS.put({"text": answer})
        proc = subprocess.run([args.codex, "exec", "--skip-git-repo-check", "--json", "--disable", "hooks", "--disable", "plugins", "-c", "notify=[]", prompt], cwd=rt.WORK, env={**os.environ, "CODEX_HOME": str(rt.HOME_DIR)}, capture_output=True, text=True, timeout=60)
        assert proc.returncode == 0, "Synthetic source CLI failed"
        events = [json.loads(line) for line in proc.stdout.splitlines()]
        thread = next(e["thread_id"] for e in events if e["type"] == "thread.started")
        run("reconcile", *source)
        run("capture", *source)
        rows = json.loads(run("status"))
        row = next(r for r in rows if r["key"]["thread"] == thread)
        assert row["state"] == "ready", row
        run("extract", "--project-id", project, "--codex-binary", args.codex, "--model", args.model, "--timeout-seconds", "180")
        jobs = json.loads(run("extraction-status", "--project-id", project))
        job = next(j for j in jobs if j["key"]["thread"] == thread)
        candidates = json.loads(run("candidates", "--project-id", project))
        own = [c for c in candidates if any(r["bundle"] == row["evidence"] for r in c["candidate"]["evidence"])]
        ready = [c for c in own if c["state"] == "ready"]
        report["corpus"].append({"case": name, "job_state": job["state"], "reason": job["reason"], "candidate_count": len(own), "ready_count": len(ready), "expected_ready": name == "supported", "expectation_met": bool(ready) if name == "supported" else not ready})
    print("Generating and independently reviewing the synthetic proposal", flush=True)
    run("consolidate", "--vault", str(vault), "--project-id", project, "--force", "--codex-binary", args.codex, "--model", args.model, "--review-model", args.model, "--timeout-seconds", "180")
    report["batches"] = json.loads(run("consolidation-status"))
    published = list(vault.glob("*.md"))
    notes = []
    for path in published:
        header = path.read_text().split('\n---\n', 1)[0]
        identity = re.search(r'^id: ([A-Za-z0-9_-]+)$', header, re.M).group(1)
        notes.append(json.loads(run("read", "--vault", str(vault), "--project-id", project, identity)))
    report["published_count"] = len(published)
    report["token_rule_preserved"] = any(token_rule(n) for n in notes)
    report["sqlite_wal_condition_preserved"] = any(applicable_wal(n) for n in notes)
    report["reuse"] = {"attempted": False}
    if published:
        print("Testing a separate real-model coding session with MCP", flush=True)
        consumer = rt.ROOT / "consumer"
        consumer.mkdir()
        (consumer / "lease_queue.py").write_text('''import sqlite3
class Queue:
    def __init__(self, connection):
        self.db = connection
        self.db.execute("CREATE TABLE IF NOT EXISTS jobs(id INTEGER PRIMARY KEY,lease_token INTEGER NOT NULL,state TEXT NOT NULL)")
        self.db.execute("INSERT INTO jobs VALUES(1,0,'queued')")
    def claim(self):
        self.db.execute("UPDATE jobs SET lease_token=lease_token+1,state='running' WHERE id=1")
        return self.db.execute("SELECT lease_token FROM jobs WHERE id=1").fetchone()[0]
    def ack(self, token):
        return self.db.execute("UPDATE jobs SET state='done' WHERE id=1").rowcount == 1
''')
        mcp = {"command": engine, "args": ["--state-dir", str(state), "mcp", "--vault", str(vault), "--project-id", project]}
        # TOML inline-table override; no persistent user/Codex configuration change.
        override = "mcp_servers.coding_memory={command=" + json.dumps(engine) + ",args=" + json.dumps(mcp["args"]) + "}"
        env = {k: v for k, v in os.environ.items() if k not in ("OPENAI_API_KEY", "CODEX_API_KEY", "OPENAI_BASE_URL")}
        env["CODEX_HOME"] = str(auth_home)
        proc = subprocess.run([args.codex, "exec", "--skip-git-repo-check", "--ignore-user-config", "--ignore-rules", "--json", "--sandbox", "workspace-write", "--disable", "hooks", "--disable", "plugins", "-c", "notify=[]", "-c", 'web_search="disabled"', "-c", 'cli_auth_credentials_store="file"', "-c", override, "--model", args.model, "Fix lease_queue.py to comply with this project's SQLite queue protocol. Search coding_memory for the applicable project memory and read the relevant notes before editing. The current claim/reclamation and acknowledgment behavior is wrong. Preserve the public Queue(connection), claim(), ack(token) interface. Change only files inside the current working directory. Do not inspect credentials or parent directories."], cwd=consumer, env=env, text=True, capture_output=True, timeout=300)
        (rt.ROOT / "consumer.stdout.jsonl").write_text(proc.stdout)
        (rt.ROOT / "consumer.stderr").write_text(proc.stderr)
        events = [json.loads(line) for line in proc.stdout.splitlines() if line.strip().startswith("{")]
        calls = [e["item"] for e in events if e.get("type") == "item.completed" and e.get("item", {}).get("type") == "mcp_tool_call"]
        test = subprocess.run([sys.executable, "-c", "import sqlite3; from lease_queue import Queue; db=sqlite3.connect('quality-check.sqlite'); assert db.execute('PRAGMA journal_mode=WAL').fetchone()[0]=='wal'; q=Queue(db); a=q.claim(); b=q.claim(); assert (a,b)==(7,14),(a,b); assert not q.ack(a); assert q.ack(b); assert not q.ack(b); print('REUSE_TEST_PASS')"], cwd=consumer, text=True, capture_output=True)
        report["reuse"] = {"attempted": True, "agent_exit": proc.returncode, "mcp_tools": [c.get("tool") for c in calls], "successful_search_and_read": retrieved_rule(calls, notes), "protocol_tests_passed": test.returncode == 0}
    report["limitations"] = ["Three synthetic cases; not a statistical quality estimate.", "Source CLI outputs are controlled, while extraction/consolidation/review and consumer decisions use a real model.", "No multi-note semantic-link quality estimate; links are validated in deterministic fixtures.", "Consumer uses the dedicated Engine login sequentially with its own fresh context; generation deny hooks remain enabled for memory-processing calls."]
    (rt.ROOT / "quality-report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    assert all(case["expectation_met"] for case in report["corpus"]), "Extraction expectations failed"
    assert report["token_rule_preserved"] and report["sqlite_wal_condition_preserved"], "Memory lost its rule or condition"
    assert report["reuse"].get("agent_exit") == 0 and report["reuse"].get("successful_search_and_read") and report["reuse"].get("protocol_tests_passed"), "Cross-session reuse failed; inspect the saved report"


if __name__ == "__main__":
    main()
