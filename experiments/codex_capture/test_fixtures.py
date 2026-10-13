"""Offline assertions for observed Codex capture contracts, not an adapter."""

import json
from pathlib import Path
import unittest


FIXTURES = Path(__file__).with_name("fixtures")


class CaptureFixtures(unittest.TestCase):
    def setUp(self):
        self.fixtures = [
            json.loads((FIXTURES / name).read_text())
            for name in ("cli-0.162.1.json", "app-bundled-0.162.0-alpha.17.2.json")
        ]

    def test_cli_notify_has_ids_missing_from_exec_terminal(self):
        for fixture in self.fixtures:
            case = fixture["cases"]["exec_normal"]
            terminal = next(e for e in case["events"] if e["type"] == "turn.completed")
            self.assertNotIn("turn_id", terminal)
            thread = case["read"]["thread"]
            notify = next(
                h["event"]
                for h in fixture["hooks"]
                if h["event"].get("thread-id") == thread["id"]
            )
            self.assertEqual(thread["turns"][0]["id"], notify["turn-id"])

    def test_stop_continuation_is_one_turn(self):
        for fixture in self.fixtures:
            thread = fixture["cases"]["exec_stop"]["read"]["thread"]
            self.assertEqual(1, len(thread["turns"]))
            turn = thread["turns"][0]
            self.assertEqual("completed", turn["status"])
            messages = [x["text"] for x in turn["items"] if x["type"] == "agentMessage"]
            self.assertEqual(
                ["pre-continuation answer", "post-continuation answer"], messages
            )
            stops = [
                h["event"]
                for h in fixture["hooks"]
                if h["event"].get("session_id") == thread["id"]
                and h["event"].get("hook_event_name") == "Stop"
            ]
            self.assertEqual([False, True], [h["stop_hook_active"] for h in stops])
            self.assertEqual({turn["id"]}, {h["turn_id"] for h in stops})

    def test_later_turn_is_excluded_by_exact_identity(self):
        for fixture in self.fixtures:
            case = fixture["cases"]["later_read"]
            turns = case["read"]["thread"]["turns"]
            self.assertEqual(2, len(turns))
            selected = [t for t in turns if t["id"] == case["earlier_turn_id"]]
            self.assertEqual(1, len(selected))
            encoded = json.dumps(selected)
            self.assertIn("synthetic-tool-result", encoded)
            self.assertNotIn("later instruction", encoded)
            self.assertNotIn("later answer", encoded)
            self.assertNotEqual(case["earlier_turn_id"], case["later_turn_id"])

    def test_steering_and_tool_evidence(self):
        for fixture in self.fixtures:
            case = fixture["cases"]["steering"]
            self.assertEqual(case["turn_id"], case["steer"]["turnId"])
            turns = case["read"]["thread"]["turns"]
            self.assertEqual(1, len(turns))
            self.assertEqual(
                2, sum(i["type"] == "userMessage" for i in turns[0]["items"])
            )
            tools = fixture["cases"]["tools"]["read"]["thread"]["turns"][0]["items"]
            commands = [i for i in tools if i["type"] == "commandExecution"]
            self.assertEqual(0, commands[0]["exitCode"])
            self.assertEqual("synthetic-tool-result", commands[0]["aggregatedOutput"])

    def test_pagination_can_retrieve_an_earlier_turn_without_later_items(self):
        for fixture in self.fixtures:
            case = fixture["cases"]["pagination"]
            self.assertEqual(
                [case["later_turn_id"]], [t["id"] for t in case["newest"]["data"]]
            )
            self.assertEqual(
                [case["earlier_turn_id"]], [t["id"] for t in case["older"]["data"]]
            )
            self.assertIn("synthetic-tool-result", json.dumps(case["earlier_items"]))
            self.assertNotIn("later answer", json.dumps(case["earlier_items"]))

    def test_failed_and_interrupted_do_not_become_completed(self):
        for fixture in self.fixtures:
            for name, status in (
                ("failure", "failed"),
                ("interruption", "interrupted"),
            ):
                case = fixture["cases"][name]
                self.assertEqual(status, case["terminal"]["turn"]["status"])
                self.assertEqual(status, case["read"]["thread"]["turns"][0]["status"])
                self.assertFalse(
                    any(
                        h["event"].get("turn-id") == case["turn_id"]
                        for h in fixture["hooks"]
                    )
                )

    def test_exec_exit_code_does_not_distinguish_failure_from_interruption(self):
        for fixture in self.fixtures:
            for name, status in (
                ("exec_failure", "failed"),
                ("exec_interruption", "interrupted"),
            ):
                case = fixture["cases"][name]
                self.assertEqual(1, case["exit"])
                self.assertEqual(status, case["read"]["thread"]["turns"][0]["status"])

    def test_an_observer_status_alone_is_not_terminal_confirmation(self):
        for fixture in self.fixtures:
            case = fixture["cases"]["interruption"]
            provisional = case["observer_read_before_interrupt"]["thread"]["turns"][0]
            self.assertEqual(case["turn_id"], provisional["id"])
            self.assertNotEqual("completed", provisional["status"])
            self.assertIsNone(provisional["completedAt"])
            confirmed = case["read"]["thread"]["turns"][0]
            self.assertEqual("interrupted", confirmed["status"])
            self.assertIsNotNone(confirmed["completedAt"])

    def test_cold_read_resume_and_compaction_preserve_old_turns(self):
        for fixture in self.fixtures:
            cases = fixture["cases"]
            before = cases["cold_read"]["read"]["thread"]
            resumed = cases["resume"]["read"]["thread"]
            self.assertEqual(before["id"], resumed["id"])
            self.assertEqual(before["turns"], resumed["turns"][:2])
            final = cases["compaction"]["final_read"]["thread"]["turns"]
            self.assertEqual(resumed["turns"], final[:3])
            self.assertEqual("completed", final[-1]["status"])
            self.assertEqual(
                ["contextCompaction"], [x["type"] for x in final[-1]["items"]]
            )

    def test_fork_has_inherited_turns_and_an_explicit_prefix_boundary(self):
        for fixture in self.fixtures:
            fork = fixture["cases"]["fork"]
            thread = fork["read"]["thread"]
            self.assertEqual(fork["parent_thread_id"], thread["forkedFromId"])
            self.assertEqual(
                [fork["inherited_turn_id"], fork["new_turn_id"]],
                [t["id"] for t in thread["turns"]],
            )
            meta = next(
                m for m in fixture["rollout_metadata"] if m["id"] == thread["id"]
            )
            self.assertEqual(
                meta["forked_from_ordinal_exclusive"],
                meta["history_base"]["end_ordinal_exclusive"],
            )
            self.assertEqual(thread["forkedFromId"], meta["history_base"]["thread_id"])


if __name__ == "__main__":
    unittest.main()
