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
            self.assertIn(call_seq, result["data"]["sourceEventSeqs"])

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

    def test_bridge_exports_compaction_wait(self):
        source = BRIDGE.read_text()
        self.assertIn("COMPACTION_SETTLE_MS", source)
        self.assertIn("waitForCompaction", source)
        self.assertIn("coding-memory/compaction-requested", source)

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
