"""Check the human-run App GUI probe against its separate cold reader."""

import json
from pathlib import Path
import unittest


class AppGuiFixture(unittest.TestCase):
    def test_trusted_app_hooks_match_the_persisted_turn(self):
        fixture = json.loads(
            (
                Path(__file__).with_name("fixtures") / "app-gui-26.1007.30912.json"
            ).read_text()
        )
        thread = fixture["read"]["thread"]
        self.assertEqual("Codex Desktop", thread["originator"])
        self.assertEqual("0.162.0-alpha.17.2", thread["cliVersion"])
        self.assertEqual("notLoaded", thread["status"]["type"])
        self.assertFalse(fixture["thread_started_observed"])
        self.assertEqual(
            ["initialize", "thread/read", "thread/items/list"],
            fixture["requested_methods"],
        )
        self.assertTrue(fixture["project_trusted"])
        self.assertEqual(5, len(fixture["trusted_hook_events"]))
        self.assertEqual(
            ["UserPromptSubmit", "Stop"],
            [h["hook_event_name"] for h in fixture["hooks"]],
        )
        turn = thread["turns"][-1]
        self.assertEqual(2, len(thread["turns"]))
        self.assertNotEqual(thread["turns"][0]["id"], turn["id"])
        self.assertEqual("completed", turn["status"])
        self.assertIsNotNone(turn["completedAt"])
        for hook in fixture["hooks"]:
            self.assertEqual(thread["id"], hook["session_id"])
            self.assertEqual(turn["id"], hook["turn_id"])
        self.assertEqual(
            ["APP_CAPTURE_OK"],
            [i["text"] for i in turn["items"] if i["type"] == "agentMessage"],
        )
        self.assertIn("APP_CAPTURE_OK", json.dumps(fixture["items"]))
        self.assertIsNone(fixture["items"]["nextCursor"])


if __name__ == "__main__":
    unittest.main()
