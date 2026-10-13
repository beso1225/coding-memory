import unittest
from unittest.mock import Mock

from contract import classify, is_internal, permitted_call


class ContractTests(unittest.TestCase):
    def test_empty_is_success(self):
        self.assertEqual(
            "empty",
            classify(
                "extraction", 0, '{"role":"extraction","items":[],"approved":true}'
            ),
        )

    def test_failures_are_distinct(self):
        self.assertEqual("process_failure", classify("review", 1, "{}"))
        self.assertEqual("invalid_output", classify("review", 0, "oops"))
        self.assertEqual(
            "invalid_output",
            classify("review", 0, '{"role":"review","items":[],"approved":1}'),
        )
        self.assertEqual(
            "review_rejected",
            classify("review", 0, '{"role":"review","items":[],"approved":false}'),
        )

    def test_external_policy_precedes_execution(self):
        spawn = Mock()
        self.assertEqual("held_external_policy", permitted_call(False, spawn))
        spawn.assert_not_called()

    def test_namespace_excludes_live_and_reconciled_runs(self):
        for event in [
            {"store": "owned", "thread": "a"},
            {"store": "owned", "thread": "missed"},
        ]:
            self.assertTrue(is_internal(event, {"owned"}, set()))
        self.assertFalse(
            is_internal({"store": "development", "thread": "a"}, {"owned"}, set())
        )


class StrictOutputTests(unittest.TestCase):
    def test_unknown_role_and_duplicate_keys_are_invalid(self):
        self.assertEqual(
            "invalid_output",
            classify("unknown", 0, '{"role":"unknown","items":[],"approved":true}'),
        )
        self.assertEqual(
            "invalid_output",
            classify(
                "review",
                0,
                '{"role":"review","items":[],"approved":false,"approved":true}',
            ),
        )
