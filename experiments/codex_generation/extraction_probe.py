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
    parser.add_argument("--consolidate", action="store_true")
    args = parser.parse_args()
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "codex_capture"))
    import runtime as rt

    subprocess.run(["git", "init", str(rt.WORK)], check=True, capture_output=True)
    state = rt.ROOT / "engine"
    base = [str(args.engine.resolve()), "--state-dir", str(state)]
    runtime = ["--codex-home", str(rt.HOME_DIR), "--codex-binary", args.codex, "--project", str(rt.WORK)]

    def run(command, *options, timeout=45):
        return subprocess.run(base + [command, *options], check=True, text=True, capture_output=True, timeout=timeout).stdout

    project = run("register", *runtime).strip()
    time.sleep(2)
    model_timeout = 30
    options = ["--project-id", project, "--codex-binary", args.codex, "--model", "capture-extraction", "--timeout-seconds", str(model_timeout), "--loopback", f"127.0.0.1:{rt.SERVER.server_port}"]
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
        if args.consolidate:
            vault = rt.ROOT / "published-vault"
            vault.mkdir()
            c = candidates[0]["candidate"]
            proposal = {"schema_version": 1, "role": "consolidation", "operations": [{"id": "op1", "action": "create", "candidates": [candidates[0]["id"]], "target": None, "draft": {"title": c["title"], "summary": c["claim"], "scope": c["scope"], "conditions": c["conditions"], "verification": c["verification"], "evidence": c["evidence"], "related": [], "body": c["claim"]}, "dependencies": [], "rationale": "One supported synthetic constraint"}]}
            review = {"schema_version": 1, "role": "review", "decisions": [{"operation": "op1", "decision": "accept", "rationale": "Explicit user instruction with applicability preserved"}]}
            rt.PLANS.put({"text": json.dumps(proposal)})
            rt.PLANS.put({"text": json.dumps(review)})
            result = json.loads(run("consolidate", "--vault", str(vault), "--project-id", project, "--force", "--codex-binary", args.codex, "--model", "capture-consolidation", "--review-model", "capture-review", "--loopback", f"127.0.0.1:{rt.SERVER.server_port}"))
            status = json.loads(run("consolidation-status"))
            assert status[0]["id"] == result["batch"] and status[0]["state"] == "applied", status
            hits = json.loads(run("search", "--vault", str(vault), "--project-id", project, "leases"))["results"]
            assert len(hits) == 1, hits
            note = json.loads(run("read", "--vault", str(vault), "--project-id", project, hits[0]["metadata"]["id"]))
            assert note["metadata"]["review"]["model"] == "capture-review", note
            assert note["metadata"]["conditions"] == c["conditions"], note
            assert note["evidence_available"] == [True], note
            results.append({"case": "independent_review_and_publication", "state": status[0]["state"], "retrieved": True})

        # Chunked extraction. A capture admits bundles far above one 256 KiB input, so the
        # engine must split them and merge one CodexExec call per chunk. Every plan answers
        # from its own request, which pins the per-chunk schema, input budget and attempt
        # identity instead of trusting the engine's own accounting.
        def request_texts(value):
            if isinstance(value, str):
                yield value
            elif isinstance(value, dict):
                for child in value.values():
                    yield from request_texts(child)
            elif isinstance(value, list):
                for child in value:
                    yield from request_texts(child)

        def attempt_names():
            directory = state / "generation/attempts"
            return {p.name for p in directory.iterdir()} if directory.exists() else set()

        def drain_plans():
            drained = 0
            while not rt.PLANS.empty():
                rt.PLANS.get_nowait()
                drained += 1
            return drained

        def extraction_timeout(items):
            """One backend-bounded call per chunk, and chunks cannot outnumber the items."""
            return model_timeout * len(items) + 45

        def bulk_turn(label):
            """Capture one own turn whose completed items exceed two chunk budgets."""
            for index in range(6):
                command = "python3 -c \"print('%s' * 100000)\"" % chr(ord("a") + index)
                rt.PLANS.put({"items": [{"type": "function_call", "name": "exec_command", "call_id": f"bulk-{label}-{index}", "arguments": json.dumps({"cmd": command, "yield_time_ms": 4000})}]})
            rt.PLANS.put({"text": "Synthetic bulk outputs recorded."})
            turn = client.call("turn/start", {"threadId": thread, "input": [{"type": "text", "text": f"Synthetic bulk constraint {label}."}]})["turn"]["id"]
            client.terminal(turn)
            run("reconcile", *runtime)
            run("capture", *runtime)
            row = next(r for r in json.loads(run("status")) if r["key"]["turn"] == turn)
            assert row["state"] == "ready", row
            return row, json.loads(run("evidence", row["evidence"]))

        def chunk_plan(items, calls, cites=None, foreign_on=None, renew=False):
            """Answer one chunk from its own request.

            `renew` keeps exactly one spare plan queued, so the number of plans follows
            the number of chunks instead of a guessed upper bound. `foreign_on` makes the
            zero-based call with that index cite a completed item from another part.
            """

            def plan(body):
                if renew:
                    rt.PLANS.put(plan)
                schema = body["text"]["format"]["schema"]
                declared = schema["properties"]["candidates"]["items"]["properties"]["evidence"]["items"]["properties"]
                prompt = next(text for text in request_texts(body) if "INPUT_JSON\n" in text)
                supplied = json.loads(prompt.split("INPUT_JSON\n", 1)[1])
                chunk = supplied["evidence"]["items"]
                known = {i["id"] for i in chunk}
                assert declared["item"]["enum"] == [i["id"] for i in chunk], "Schema offered items outside the chunk"
                assert declared["bundle"]["enum"] == [supplied["bundle"]], "Schema offered another bundle"
                input_bytes = len(prompt.encode()) + len(json.dumps(schema, separators=(",", ":"), ensure_ascii=False).encode())
                assert input_bytes <= 256 * 1024, ("Chunk exceeds the model input budget", input_bytes)
                calls.append(input_bytes)
                if foreign_on == len(calls) - 1:
                    # A completed item that is in the bundle but only in another part. The
                    # engine checks references against the supplied chunk alone, so this
                    # fails exactly like an invented ID; stating the choice here is what
                    # separates the two meanings.
                    target = next(i for i in items if i["id"] not in known and i["type"] == "commandExecution" and i["status"] == "completed")
                else:
                    target = next(i for i in chunk if i["type"] == "commandExecution" and i["status"] == "completed")
                if cites is not None:
                    cites.append({"item": target["id"], "in_part": target["id"] in known})
                output = {"schema_version": 1, "role": "extraction", "project_id": project, "candidates": [{"kind": "procedure", "title": f"Synthetic chunk claim {target['id']}", "claim": f"Chunk cited {target['id']} from its own part of the bundle.", "scope": {"kind": "project", "project_id": project}, "conditions": ["Synthetic loopback evidence"], "verification": "observed", "evidence": [{"bundle": supplied["bundle"], "item": target["id"]}]}]}
                return {"text": json.dumps(output)}
            return plan

        try:
            assert drain_plans() == 0, "Previous cases left loopback plans queued"
            row, bundle = bulk_turn("chunked")
            assert len(bundle["items"]) == 8, bundle["items"]
            before_attempts = attempt_names()
            before_candidates = json.loads(run("candidates", "--project-id", project))
            calls, cites = [], []
            rt.PLANS.put(chunk_plan(bundle["items"], calls, cites=cites, renew=True))
            extracted = next(r for r in json.loads(run("extract", *options, timeout=extraction_timeout(bundle["items"]))) if r["key"]["turn"] == row["key"]["turn"])
            assert extracted["state"] == "succeeded", extracted
            provenance = json.loads(extracted["provenance"])
            chunk_count = provenance["chunk_count"]
            assert chunk_count >= 2 and provenance["completed_chunks"] == chunk_count, provenance
            assert len(calls) == chunk_count, (len(calls), chunk_count)
            # provenance keeps only the latest chunk, so this pins the last call alone; every
            # earlier chunk is bounded by the plan's own input budget assertion.
            assert calls[-1] == provenance["input_bytes"], (calls[-1], provenance["input_bytes"])
            created = sorted(attempt_names() - before_attempts)
            assert len(created) == chunk_count, created
            assert sorted(name.rsplit("-", 1)[1] for name in created) == [str(i) for i in range(chunk_count)], created
            assert len({name.rsplit("-", 1)[0] for name in created}) == 1, created
            known = {c["id"] for c in before_candidates}
            staged = [c for c in json.loads(run("candidates", "--project-id", project)) if c["id"] not in known]
            assert len(staged) == chunk_count, staged
            assert len({c["candidate"]["evidence"][0]["item"] for c in staged}) == chunk_count, staged
            assert all(c["in_part"] for c in cites), cites
            assert drain_plans() == 1, "Chunked extraction left an unexpected renewing plan count"
            results.append({"case": "chunked_extraction", "state": extracted["state"], "chunks": chunk_count, "calls": len(calls), "input_bytes": calls, "cited": cites})

            row, bundle = bulk_turn("omitted")
            before_candidates = json.loads(run("candidates", "--project-id", project))
            calls, cites = [], []
            rt.PLANS.put(chunk_plan(bundle["items"], calls, cites=cites, foreign_on=1, renew=True))
            extracted = next(r for r in json.loads(run("extract", *options, timeout=extraction_timeout(bundle["items"]))) if r["key"]["turn"] == row["key"]["turn"])
            assert extracted["state"] == "held", extracted
            assert extracted["reason"] == "held_invalid_candidates", extracted
            # The first call answered validly and only the second cited an omitted part, so the
            # held job also proves the earlier chunk's candidates were never committed.
            assert len(calls) == 2 and len(cites) == 2, (calls, cites)
            # The second citation must be a real bundle item from the other part, never an
            # invented ID, or the case would not describe an omitted-part reference at all.
            assert cites[0]["in_part"] and not cites[1]["in_part"], cites
            assert cites[1]["item"] in {i["id"] for i in bundle["items"]}, cites
            assert drain_plans() == 1, "Omitted-chunk reference left an unexpected renewing plan count"
            assert json.loads(run("candidates", "--project-id", project)) == before_candidates, "Omitted-chunk reference staged a candidate"
            results.append({"case": "omitted_chunk_reference", "state": extracted["state"], "reason": extracted["reason"], "calls": len(calls), "cited": cites, "staged": 0})
        finally:
            # A failing plan leaves its renewing spare queued; never leak plans into the next
            # case's accounting.
            drain_plans()

        home = state / "generation/codex-home"
        assert not (home / "auth.json").exists(), "Synthetic probe created credentials"
        owned = subprocess.run(base + ["reconcile", "--codex-home", str(home), "--codex-binary", args.codex, "--project", str(rt.WORK)], check=True, capture_output=True, text=True).stdout
        assert owned.strip() == "0", owned
        print(json.dumps({"version": subprocess.check_output([args.codex, "--version"], text=True).strip(), "results": results, "candidate_count": len(json.loads(run("candidates", "--project-id", project))), "owned_reconciliation": 0, "state_directory": str(state)}, indent=2))
    finally:
        client.close()


if __name__ == "__main__":
    main()
