import json
from pathlib import Path
import unittest
from contract import classify


class ObservedTests(unittest.TestCase):
    def test_installed_runtime_contract(self):
        f = json.loads(Path(__file__).with_name("observed.json").read_text())
        self.assertFalse(f["hook_log_exists"])
        self.assertFalse(f["mcp_started"])
        self.assertEqual("timeout", f["timeout_outcome"])
        ids = [r["thread_id"] for r in f["results"]]
        self.assertEqual(len(ids), len(set(ids)))
        self.assertTrue(set(ids).issubset(f["persisted_native_ids"]))
        for r in f["results"]:
            self.assertEqual(r["outcome"], classify(r["role"], r["exit"], r["final"]))
            self.assertEqual(r["model"], r["requests"][0]["model"])
            self.assertTrue(r["requests"][0]["response_format"])
            self.assertTrue(r["internal_live"] and r["internal_reconciled"])
        self.assertTrue(f["results"][-1]["tool_items"])
