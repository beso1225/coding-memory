import unittest
from quality_checks import token_rule, applicable_wal, retrieved_rule


def note(body='Each claim must increment lease_token by exactly seven (7).', identity='note'):
    return {'metadata': {'id': identity, 'title': 'Queue rule', 'summary': '', 'conditions': ['This synthetic SQLite WAL queue only'], 'evidence': [{'bundle': 'a7b'}], 'created_at': 1791951127}, 'markdown': '---\nid: note\ncreated_at: 1791951127\n---\n' + body, 'revision': 'revision'}


def call(tool, payload, **extra):
    return {'server': 'coding_memory', 'tool': tool, 'status': 'completed', 'arguments': {'id': 'note'}, 'result': {'structured_content': payload}, **extra}


class QualityChecks(unittest.TestCase):
    def test_metadata_digits_are_not_a_token_rule(self):
        self.assertFalse(token_rule(note('Use fenced acknowledgments.')))
        self.assertFalse(token_rule(note('Increment tokens by 17.')))
        self.assertFalse(token_rule(note('There are 7 unrelated records.')))
        self.assertTrue(token_rule(note()))
        self.assertTrue(token_rule(note('Advance lease_token by exactly 7.')))

    def test_wal_must_be_an_applicability_condition(self):
        n = note('SQLite WAL is mentioned in the body.')
        n['metadata']['conditions'] = []
        self.assertFalse(applicable_wal(n))
        self.assertTrue(applicable_wal(note()))

    def test_completed_empty_search_or_unrelated_read_does_not_establish_reuse(self):
        n = note()
        hit = {'results': [{'metadata': n['metadata']}]}
        self.assertFalse(retrieved_rule([call('memory_search', {'results': []}), call('memory_read', n)], [n]))
        self.assertFalse(retrieved_rule([call('memory_search', hit), call('memory_read', note(identity='other'))], [n]))
        self.assertFalse(retrieved_rule([call('memory_read', n), call('memory_search', hit)], [n]))
        self.assertTrue(retrieved_rule([call('memory_search', hit), call('memory_read', n)], [n]))

    def test_failed_foreign_or_changed_payload_is_not_reuse(self):
        n = note()
        hit = {'results': [{'metadata': n['metadata']}]}
        for extra in ({'status': 'failed'}, {'server': 'other'}, {'error': {'message': 'failure'}}):
            self.assertFalse(retrieved_rule([call('memory_search', hit), call('memory_read', n, **extra)], [n]))
        changed = note()
        changed['revision'] = 'changed'
        self.assertFalse(retrieved_rule([call('memory_search', hit), call('memory_read', changed)], [n]))
        bad = call('memory_read', n)
        bad['result']['isError'] = True
        self.assertFalse(retrieved_rule([call('memory_search', hit), bad], [n]))

    def test_text_payload_and_numeric_rule_are_supported(self):
        import json
        n = note('Each claim increments lease_token by 7.')
        hit = {'results': [{'metadata': n['metadata']}]}
        calls = [call('memory_search', hit), call('memory_read', n)]
        for c in calls:
            payload = c['result'].pop('structured_content')
            c['result']['content'] = [{'type': 'text', 'text': json.dumps(payload)}]
        self.assertTrue(retrieved_rule(calls, [n]))
