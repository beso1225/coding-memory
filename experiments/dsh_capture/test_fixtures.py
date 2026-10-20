"""Offline assertions for the observed DeepSeek Harness capture contract.

The fixture is synthetic and mirrors the record shapes seen in the isolated
experiment ([docs/dsh-capture-experiment.md](../../docs/dsh-capture-experiment.md)).
It contains no personal ids, paths, or transcript content.
"""

import json
from pathlib import Path
import unittest


FIXTURES = Path(__file__).with_name("fixtures")
BRIDGE = Path(__file__).parents[2] / "bridge" / "dsh-capture" / "index.mjs"


class DshFixture(unittest.TestCase):
    def setUp(self):
        self.document = json.loads((FIXTURES / "session-v4.json").read_text())

    def test_records_are_contiguous_from_zero(self):
        seqs = [record["seq"] for record in self.document["records"]]
        self.assertEqual(list(range(len(seqs))), seqs)

    def test_every_record_has_a_top_level_time(self):
        for record in self.document["records"]:
            self.assertIsInstance(record["time"], int, record["type"])

    def test_human_runs_are_only_direct_user_messages(self):
        user = [r for r in self.document["records"] if r["type"] == "user/message"]
        human = [r for r in user if r["data"]["source"]["kind"] == "user"]
        self.assertEqual(1, len(human))
        self.assertLess(len(human), len(user), "injected context must not count as human")

    def test_turn_boundaries_close_every_started_turn(self):
        started = [r["data"]["turn"] for r in self.document["records"] if r["type"] == "turn/start"]
        ended = [r["data"]["turn"] for r in self.document["records"] if r["type"] == "turn/end"]
        self.assertEqual(started, ended)
        for record in self.document["records"]:
            if record["type"] == "turn/end":
                self.assertIn(record["data"]["reason"]["kind"], {"completed", "error", "aborted", "interrupted"})

    def test_tool_results_cite_their_call(self):
        calls = {
            r["data"]["callId"]
            for r in self.document["records"]
            if r["type"] == "tool/call"
        }
        results = [r for r in self.document["records"] if r["type"] == "tool/result"]
        self.assertTrue(results)
        for result in results:
            self.assertIn(result["data"]["message"]["toolCallId"], calls)
            call_seq = [r["seq"] for r in self.document["records"]
                        if r["type"] == "tool/call"
                        and r["data"]["callId"] == result["data"]["message"]["toolCallId"]][0]
            self.assertIn(call_seq, result["sourceEventSeqs"], "locators are envelope fields")

    def test_message_shapes_match_the_harness(self):
        # `user/message` has no turn or step: ownership comes from the enclosing
        # `turn/start`. A tool call's arguments stay the raw model string.
        for record in self.document["records"]:
            if record["type"] == "user/message":
                self.assertNotIn("turn", record["data"])
                self.assertNotIn("step", record["data"])
            if record["type"] == "tool/call":
                self.assertIsInstance(record["data"]["arguments"], str)
            if record["type"] == "assistant/message":
                self.assertIn("stream", record["data"])

    def test_projection_keeps_message_payloads_replayable(self):
        # A projected record keeps DSH's envelope and its `data` unchanged; only
        # the envelope fields are selected. Rewriting a message payload makes
        # the session unreadable on resume.
        records = self.document["records"]
        projected = [
            {k: v for k, v in record.items() if k in ("seq", "type", "time", "surfaceOp", "sourceEventSeqs", "ignorable", "data")}
            for record in records
        ]
        for before, after in zip(records, projected):
            self.assertEqual(before.get("data"), after.get("data"))
        assistant = [r for r in records if r["type"] == "assistant/message"]
        for message in assistant:
            self.assertEqual(message["data"]["message"]["source"]["kind"], "model")
            self.assertIn("provider", message["data"]["message"]["source"])

    def test_bridge_project_record_preserves_payload(self):
        # Only the envelope fields are selected; `data` must survive unchanged,
        # and an absent key must stay absent rather than become `undefined`.
        import json as jsonlib
        import subprocess
        script = (
            "const m = await import(process.argv[1]);"
            "console.log(JSON.stringify(m.projectRecord({ seq: 1, type: 'user/message',"
            " time: 2, surfaceOp: 'append', data: { id: 'm', content: [] }, extra: 'drop' })));"
        )
        output = subprocess.run(
            ["node", "--input-type=module", "-e", script, str(BRIDGE)],
            capture_output=True, text=True, check=True,
        ).stdout
        self.assertEqual(
            jsonlib.loads(output),
            {
                "seq": 1,
                "type": "user/message",
                "time": 2,
                "surfaceOp": "append",
                "data": {"id": "m", "content": []},
            },
        )

    def test_bridge_config_requires_absolute_paths(self):
        import json as jsonlib
        import subprocess
        script = (
            "const { Config } = await import(process.argv[1]);"
            "const v = (c) => JSON.stringify(Config['~standard'].validate(c).issues?.map(i => i.message) ?? []);"
            "console.log(v({}) + '\\n' + v({ executable: 'relative', stateDir: '/s', dshHome: '/h' })"
            " + '\\n' + v({ executable: '/e', stateDir: '/s', dshHome: '/h' })"
            " + '\\n' + v({ executable: '/e', stateDir: '/s', dshHome: '/h', timeoutMs: -1 }));"
        )
        output = subprocess.run(
            ["node", "--input-type=module", "-e", script, str(BRIDGE)],
            capture_output=True, text=True, check=True,
        ).stdout.splitlines()
        self.assertEqual(len(jsonlib.loads(output[0])), 3, "empty config names all three paths")
        self.assertEqual(jsonlib.loads(output[1]), ["executable must be an absolute path"])
        self.assertEqual(jsonlib.loads(output[2]), [], "valid config is accepted")
        self.assertEqual(jsonlib.loads(output[3]), ["timeoutMs must be a positive number"])

    def test_bridge_publication_is_race_free_and_waits_expire(self):
        import json as jsonlib
        import subprocess
        import tempfile
        script = (
            "const m = await import(process.argv[1]);"
            "const fs = await import('node:fs/promises');"
            "const os = await import('node:os');"
            "const path = await import('node:path');"
            "const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'bridge-pub-'));"
            "const target = path.join(dir, 's.json');"
            "await Promise.all([m.publish(target, 'A'), m.publish(target, 'B'), m.publish(target, 'C')]);"
            "const left = (await fs.readdir(dir)).filter(n => n.endsWith('.tmp')).length;"
            "console.log(JSON.stringify({ text: await fs.readFile(target, 'utf8'), left }));"
            "await fs.rm(dir, { recursive: true });"
        )
        output = subprocess.run(
            ["node", "--input-type=module", "-e", script, str(BRIDGE)],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
        result = jsonlib.loads(output)
        # Concurrent renames complete in any order, so the final file is one
        # complete member of the input set: never a torn or mixed document.
        self.assertIn(result["text"], {"A", "B", "C"}, "concurrent publishes never tear")
        self.assertEqual(result["left"], 0, "a concurrent publish leaves no temporary file")

    def test_bridge_bounds_a_long_session_projection(self):
        import json as jsonlib
        import subprocess
        script = (
            "const { projectForCapture } = await import(process.argv[1]);"
            "const header = { version: 4, id: 's', createdAt: 1, cwd: '/repo', isSeeded: false };"
            "const big = 'x'.repeat(1 << 20); const events = []; let seq = 0;"
            "const add = (type, data) => events.push({ seq: seq++, type, time: 1000 + seq, data });"
            "for (let turn = 1; turn <= 20; turn++) {"
            " add('turn/start', { turn });"
            " add('user/message', { id: 'm' + turn, role: 'user', content: [{ type: 'text', text: turn === 20 ? 'hi' : big }], source: { kind: 'user' } });"
            " add('assistant/message', { turn, step: 1, message: { id: 'a' + turn, role: 'assistant', content: [{ type: 'text', text: 'ok' }], source: { kind: 'model', provider: 'p', model: 'm' } } });"
            " add('turn/end', { turn, reason: { kind: 'completed' } }); }"
            "const text = projectForCapture(header, events, 20);"
            "const doc = JSON.parse(text);"
            "console.log(JSON.stringify({ bytes: Buffer.byteLength(text),"
            " seqs: doc.records.map(r => r.seq), first: doc.records[0], last: doc.records.at(-1) }));"
        )
        output = subprocess.run(
            ["node", "--input-type=module", "-e", script, str(BRIDGE)],
            capture_output=True, text=True, check=True,
        ).stdout
        result = jsonlib.loads(output)
        self.assertLess(result["bytes"], 16 * 1024 * 1024, "a 19 MiB session projects within the bound")
        self.assertEqual(result["first"]["type"], "turn/start")
        self.assertEqual(result["first"]["data"]["turn"], 20, "the window starts at the captured turn")
        self.assertEqual(result["last"]["type"], "turn/end")
        self.assertEqual(result["last"]["data"]["turn"], 20)
        seqs = result["seqs"]
        self.assertEqual(
            seqs, list(range(seqs[0], seqs[0] + len(seqs))),
            "the bounded window keeps native seqs contiguous",
        )

    def test_bridge_publishes_one_per_turn_projection(self):
        # The same publication serves a deferred (unregistered) turn, which has
        # no capture invocation to write the snapshot registration consumes.
        import json as jsonlib
        import subprocess
        script = (
            "const m = await import(process.argv[1]);"
            "const fs = await import('node:fs/promises');"
            "const os = await import('node:os');"
            "const path = await import('node:path');"
            "const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'bridge-pub-'));"
            "const events = ["
            " { seq: 0, type: 'turn/start', time: 1000, data: { turn: 3 } },"
            " { seq: 1, type: 'user/message', time: 1001, data: { id: 'm', role: 'user',"
            " content: [{ type: 'text', text: 'hi' }], source: { kind: 'user' } } },"
            " { seq: 2, type: 'turn/end', time: 1002, data: { turn: 3, reason: { kind: 'completed' } } } ];"
            "const published = await m.publishTurn(dir, 's', 3, async () => ({"
            " session: { version: 4, id: 's', createdAt: 1, cwd: '/repo', isSeeded: false }, events }));"
            "const doc = JSON.parse(await fs.readFile(published.path, 'utf8'));"
            "console.log(JSON.stringify({ rel: path.relative(dir, published.path), cwd: published.cwd,"
            " schema: doc.schema, last: doc.records.at(-1), count: doc.records.length }));"
            "await fs.rm(dir, { recursive: true });"
        )
        output = subprocess.run(
            ["node", "--input-type=module", "-e", script, str(BRIDGE)],
            capture_output=True, text=True, check=True,
        ).stdout
        result = jsonlib.loads(output)
        self.assertEqual(result["rel"], "dsh/projections/s.3.json")
        self.assertEqual(result["cwd"], "/repo")
        self.assertEqual(result["schema"], 1)
        self.assertEqual(result["count"], 3)
        self.assertEqual(result["last"]["type"], "turn/end")

    def test_bridge_retries_pending_captures_and_retires_projections(self):
        import json as jsonlib
        import os
        import subprocess
        import tempfile
        script = (
            "const m = await import(process.argv[1]);"
            "const fs = await import('node:fs/promises');"
            "const os = await import('node:os');"
            "const path = await import('node:path');"
            "const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'bridge-ret-'));"
            "const file = path.join(dir, 's.1.json');"
            "await fs.writeFile(file, '{}'); await m.removeProjection(file);"
            "console.log(JSON.stringify({"
            " pending: m.parseReport(JSON.stringify({ state: 'queued', reason: 'history_pending' })),"
            " terminal: m.parseReport(JSON.stringify({ state: 'ready', evidence: 'x' })),"
            " empty: m.parseReport('not json'),"
            " retryPending: m.retryable('history_pending'),"
            " retryNoRun: m.retryable('no_ready_run'),"
            " retryTerminal: m.retryable('history_missing'),"
            " retryDelay: m.CAPTURE_RETRY_MS,"
            " attempts: m.CAPTURE_ATTEMPTS,"
            " errors: m.CAPTURE_ERRORS,"
            " left: (await fs.readdir(dir)).length }));"
            "await fs.rm(dir, { recursive: true });"
        )
        output = subprocess.run(
            ["node", "--input-type=module", "-e", script, str(BRIDGE)],
            capture_output=True, text=True, check=True,
        ).stdout
        result = jsonlib.loads(output)
        self.assertEqual(result["pending"]["reason"], "history_pending")
        self.assertEqual(result["terminal"]["state"], "ready")
        self.assertEqual(result["empty"], {}, "an unparsable report is never terminal")
        self.assertTrue(result["retryPending"], "a flushed-too-early turn is retried")
        self.assertTrue(
            result["retryNoRun"],
            "a run inside the engine's deferral window is retried, not discarded",
        )
        self.assertFalse(result["retryTerminal"], "terminal reasons stop the retry")
        self.assertGreaterEqual(
            result["retryDelay"], 10000,
            "the retry must not be shorter than the engine's deferral",
        )
        self.assertGreaterEqual(
            result["attempts"], 60,
            "the bridge must drive the engine's pending-attempt limit",
        )
        self.assertGreaterEqual(
            result["errors"] * result["retryDelay"], 60000,
            "no-ready retries must cover the engine's 60-second DSH lease",
        )
        self.assertEqual(result["left"], 0, "a retired projection is deleted")

    def test_bridge_retries_a_stale_oversized_snapshot(self):
        # A read that predates the hinted turn must not publish an over-limit
        # whole-session document; it reports a retryable stale snapshot.
        import json as jsonlib
        import subprocess
        script = (
            "const m = await import(process.argv[1]);"
            "const big = 'x'.repeat(1 << 20);"
            "const header = { version: 4, id: 's', createdAt: 1, cwd: '/repo', isSeeded: false };"
            "const events = []; let seq = 0;"
            "const add = (type, data) => events.push({ seq: seq++, type, time: 1000 + seq, data });"
            "add('turn/start', { turn: 1 });"
            "add('user/message', { id: 'm1', role: 'user', content: [{ type: 'text', text: big }], source: { kind: 'user' } });"
            "for (let i = 0; i < 18; i++) add('tool/result', { turn: 1, step: 1, message: {"
            " id: 't' + i, role: 'tool', toolCallId: 'c' + i, content: [{ type: 'text', text: big }] } });"
            "add('turn/end', { turn: 1, reason: { kind: 'completed' } });"
            "let stale = false; let bytes = 0;"
            "try { const text = m.projectForCapture(header, events, 2); bytes = Buffer.byteLength(text); }"
            "catch (error) { stale = error instanceof m.StaleProjection; }"
            "const small = events.slice(0, 2).concat([{ seq: 99, type: 'turn/end', time: 1,"
            " data: { turn: 1, reason: { kind: 'completed' } } }]);"
            "let staleSmall = false;"
            "try { m.projectForCapture(header, small, 2); }"
            "catch (error) { staleSmall = error instanceof m.StaleProjection; }"
            "console.log(JSON.stringify({ stale, staleSmall, bytes }));"
        )
        output = subprocess.run(
            ["node", "--input-type=module", "-e", script, str(BRIDGE)],
            capture_output=True, text=True, check=True,
        ).stdout
        result = jsonlib.loads(output)
        self.assertTrue(result["stale"], "an oversized stale snapshot is retryable")
        self.assertTrue(result["staleSmall"], "a small stale snapshot is retryable too")
        self.assertEqual(result["bytes"], 0, "nothing over the bound is published")

    def test_bridge_classifies_an_oversized_turn_as_permanent(self):
        # A turn that itself exceeds the transport bound cannot become smaller
        # by re-reading it, so it must not consume the retry budget.
        import json as jsonlib
        import subprocess
        script = (
            "const m = await import(process.argv[1]);"
            "const big = 'x'.repeat(17 * 1024 * 1024);"
            "const header = { version: 4, id: 's', createdAt: 1, cwd: '/repo', isSeeded: false };"
            "const events = ["
            " { seq: 0, type: 'turn/start', time: 1000, data: { turn: 1 } },"
            " { seq: 1, type: 'user/message', time: 1001, data: { id: 'm', role: 'user',"
            " content: [{ type: 'text', text: 'work' }], source: { kind: 'user' } } },"
            " { seq: 2, type: 'tool/result', time: 1002, data: { turn: 1, step: 1, message: {"
            " id: 't', role: 'tool', toolCallId: 'c', content: [{ type: 'text', text: big }] } } },"
            " { seq: 3, type: 'turn/end', time: 1003, data: { turn: 1, reason: { kind: 'completed' } } } ];"
            "let over = false;"
            "try { m.projectForCapture(header, events, 1); }"
            "catch (error) { over = error instanceof m.OverLimitProjection; }"
            "console.log(JSON.stringify({ over, errors: m.CAPTURE_ERRORS, attempts: m.CAPTURE_ATTEMPTS,"
            " errorDelay: m.CAPTURE_ERROR_RETRY_MS }));"
        )
        output = subprocess.run(
            ["node", "--input-type=module", "-e", script, str(BRIDGE)],
            capture_output=True, text=True, check=True,
        ).stdout
        result = jsonlib.loads(output)
        self.assertTrue(result["over"], "an oversized turn is a permanent outcome")
        self.assertEqual(result["attempts"], 60)
        self.assertGreater(result["errors"], 0, "transient failures have a separate budget")
        self.assertLess(result["errorDelay"], 10000)

    def test_publish_removes_a_failed_temporary(self):
        # A failed rename must not leave a raw-session .tmp file behind.
        import json as jsonlib
        import subprocess
        script = (
            "const m = await import(process.argv[1]);"
            "const fs = await import('node:fs/promises');"
            "const os = await import('node:os');"
            "const path = await import('node:path');"
            "const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'bridge-tmp-'));"
            "const target = path.join(dir, 'target');"
            "await fs.mkdir(target);"
            "let failed = false;"
            "try { await m.publish(target, 'x'); } catch { failed = true; }"
            "const left = (await fs.readdir(dir)).filter(name => name.endsWith('.tmp')).length;"
            "console.log(JSON.stringify({ failed, left }));"
            "await fs.rm(dir, { recursive: true });"
        )
        output = subprocess.run(
            ["node", "--input-type=module", "-e", script, str(BRIDGE)],
            capture_output=True, text=True, check=True,
        ).stdout
        result = jsonlib.loads(output)
        self.assertTrue(result["failed"], "renaming onto a directory fails")
        self.assertEqual(result["left"], 0, "the failed temporary is removed")

    def test_bridge_keeps_session_ids_inside_the_projection_directory(self):
        # A DSH session id is an arbitrary string. An unencoded id containing
        # path separators would publish outside the state directory.
        import json as jsonlib
        import subprocess
        script = (
            "const m = await import(process.argv[1]);"
            "const path = await import('node:path');"
            "const hostile = m.projectionPath('/state', '../../tmp/evil', 2);"
            "const nested = m.projectionPath('/state', 'a/b', 3);"
            "let rejected = false;"
            "try { m.projectionPath('/state', 's', '4'); } catch { rejected = true; }"
            "console.log(JSON.stringify({"
            " hostileDir: path.dirname(hostile), hostileName: path.basename(hostile),"
            " nestedDir: path.dirname(nested), nestedName: path.basename(nested), rejected }));"
        )
        output = subprocess.run(
            ["node", "--input-type=module", "-e", script, str(BRIDGE)],
            capture_output=True, text=True, check=True,
        ).stdout
        result = jsonlib.loads(output)
        self.assertEqual(result["hostileDir"], "/state/dsh/projections")
        self.assertEqual(result["nestedDir"], "/state/dsh/projections")
        self.assertNotIn("/", result["hostileName"])
        self.assertNotIn("/", result["nestedName"])
        self.assertTrue(result["rejected"], "a non-integer turn cannot name a file")

    def test_bridge_builds_engine_argv(self):
        # The CLI contract is asserted on the builder's output, not on source
        # fragments, so a refactor can rename internals without breaking this.
        import json as jsonlib
        import subprocess
        script = (
            "const m = await import(process.argv[1]);"
            "console.log(JSON.stringify({"
            " enqueue: m.enqueueArgs('signal'),"
            " capture: m.captureArgs('/projection', '/project', 3, 'session-x'),"
            " hold: m.holdArgs('session-x', 3),"
            " markers: m.COMPACTION_REQUEST_EVENTS }));"
        )
        output = subprocess.run(
            ["node", "--input-type=module", "-e", script, str(BRIDGE)],
            capture_output=True, text=True, check=True,
        ).stdout
        result = jsonlib.loads(output)
        self.assertEqual(result["enqueue"], ["enqueue", "--event", "signal"])
        self.assertEqual(
            result["capture"],
            ["capture", "--session", "/projection", "--project", "/project",
             "--turn", "3", "--thread", "session-x"],
        )
        self.assertEqual(
            result["hold"], ["hold-over-limit", "--thread", "session-x", "--turn", "3"]
        )
        self.assertIn("coding-memory/compaction-requested", result["markers"])

    def _run_apply_probe(self, mode):
        # Mounts the real plugin with a fake Cordis context and a stub engine.
        import json as jsonlib
        import os
        import shutil
        import subprocess
        import tempfile
        from pathlib import Path
        root = Path(tempfile.mkdtemp(prefix="bridge-apply-"))
        try:
            stub = root / "stub.mjs"
            stub.write_text(
                "#!/usr/bin/env node\n"
                "import { appendFileSync, existsSync } from 'node:fs'\n"
                "const args = process.argv.slice(2)\n"
                "const mode = process.env.BRIDGE_PROBE_MODE ?? 'ready'\n"
                "const at = args.indexOf('--session')\n"
                "const session = at >= 0 ? args[at + 1] : null\n"
                "appendFileSync(process.env.BRIDGE_PROBE_LOG, JSON.stringify({"
                " args, sessionExists: session ? existsSync(session) : null }) + '\\n')\n"
                "if (args.includes('enqueue')) console.log(JSON.stringify({"
                " action: 'enqueue', state: mode === 'deferred' ? 'deferred' : 'queued' }))\n"
                "else if (args.includes('capture')) console.log(JSON.stringify("
                "{ action: 'capture', state: mode === 'held' ? 'held' : 'ready',"
                " evidence: 'digest' }))\n"
                "else console.log('{}')\n"
            )
            stub.chmod(0o755)
            state = root / "state"
            state.mkdir()
            log = root / "calls.jsonl"
            probe = Path(__file__).with_name("apply_probe.mjs")
            completed = subprocess.run(
                ["node", str(probe), str(BRIDGE), str(stub), str(state), str(log), mode],
                capture_output=True, text=True, timeout=20,
                env={
                    **os.environ,
                    "BRIDGE_PROBE_LOG": str(log),
                    "BRIDGE_PROBE_MODE": mode,
                },
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            return jsonlib.loads(completed.stdout)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    def test_bridge_apply_enqueues_then_captures(self):
        # This exercises agent/created, turn/end, the projection publication,
        # the hint-then-capture order, `--thread`, and terminal cleanup.
        result = self._run_apply_probe("ready")
        self.assertTrue(result["complete"])
        self.assertFalse(result["projection"], "a finished capture removes its projection")
        calls = result["lines"]
        self.assertEqual(len(calls), 2, "one enqueue and one capture")
        first, second = calls[0]["args"], calls[1]["args"]
        self.assertIn("enqueue", first)
        self.assertLess(first.index("enqueue"), first.index("--event"))
        self.assertIn("capture", second)
        self.assertEqual(second[second.index("--thread") + 1], "session-x")
        self.assertEqual(second[second.index("--turn") + 1], "1")
        self.assertTrue(calls[1]["sessionExists"], "capture reads the published projection")

    def test_bridge_apply_waits_for_compaction(self):
        # The hint goes out immediately; the capture waits for the bracket so
        # the projection reflects the finished surface.
        result = self._run_apply_probe("compact")
        self.assertTrue(result["hintSent"])
        self.assertTrue(
            result["waitingWhileCompacting"],
            "capture must not run while compaction/start is open",
        )
        self.assertTrue(result["complete"])
        self.assertFalse(result["projection"])
        self.assertEqual(len(result["lines"]), 2)

    def test_bridge_apply_retains_a_held_projection(self):
        # A held run is explicitly retryable; deleting its projection would
        # leave `retry` with no input to drive.
        result = self._run_apply_probe("held")
        self.assertEqual(len(result["lines"]), 2)
        self.assertTrue(result["projection"], "a held projection is retained")

    def test_bridge_apply_publishes_a_deferred_projection(self):
        # An unregistered project keeps the hint and publishes the snapshot that
        # register/reconcile consume; no capture is attempted.
        result = self._run_apply_probe("deferred")
        self.assertTrue(result["published"], "the bootstrap snapshot is published")
        self.assertTrue(result["projection"], "it is retained for reconcile")
        calls = result["lines"]
        self.assertEqual(len(calls), 1, "no capture is attempted for a deferred project")
        self.assertIn("enqueue", calls[0]["args"])


if __name__ == "__main__":
    unittest.main()
