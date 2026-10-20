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

    def test_bridge_exports_payload_preserving_projection(self):
        source = BRIDGE.read_text()
        self.assertIn("export function projectRecord", source)
        self.assertIn("records: events.map(projectRecord)", source)

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
        source = BRIDGE.read_text()
        self.assertIn("randomUUID()}.tmp", source)
        # A bracket that never closes must not block capture forever.
        self.assertIn("did not finish within", source)
        self.assertIn("compacting.delete(id)", source)

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
        source = BRIDGE.read_text()
        self.assertIn("hint.state === 'deferred'", source)
        self.assertIn(
            "publishTurn(stateDir, session.id, turn, read)",
            source,
            "the deferred bootstrap publishes the snapshot register consumes",
        )

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
        self.assertEqual(result["left"], 0, "a retired projection is deleted")
        source = BRIDGE.read_text()
        self.assertIn("CAPTURE_ATTEMPTS", source)
        self.assertIn("attemptsLeft - 1", source)

    def test_bridge_exports_compaction_wait(self):
        source = BRIDGE.read_text()
        self.assertIn("waitForCompaction", source)
        self.assertIn("coding-memory/compaction-requested", source)

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
            "console.log(JSON.stringify({ stale, bytes }));"
        )
        output = subprocess.run(
            ["node", "--input-type=module", "-e", script, str(BRIDGE)],
            capture_output=True, text=True, check=True,
        ).stdout
        result = jsonlib.loads(output)
        self.assertTrue(result["stale"], "an oversized stale snapshot is retryable")
        self.assertEqual(result["bytes"], 0, "nothing over the bound is published")
        source = BRIDGE.read_text()
        self.assertIn("hint.state === 'held'", source)

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

    def test_bridge_projects_header_and_engine_vocabulary(self):
        self.assertTrue(BRIDGE.is_file())
        source = BRIDGE.read_text()
        for name in ("project", "engineOwned", "apply"):
            self.assertIn(f"export function {name}", source)
        for constant in ("MAX_PROJECTION_BYTES", "MAX_SIGNAL_BYTES", "ENGINE_SESSION"):
            self.assertIn(f"export const {constant}", source)
        self.assertIn("readSession", source)
        self.assertIn("turn/end", source)
        self.assertIn("agent/created", source)


if __name__ == "__main__":
    unittest.main()
