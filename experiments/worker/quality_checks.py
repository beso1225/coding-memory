"""Offline evaluation predicates; no runtime, model, credential or YAML dependency."""
import json
import re


def semantic_text(note):
    metadata = note.get('metadata', {})
    markdown = note.get('markdown', '')
    if markdown.startswith('---\n'):
        _, separator, markdown = markdown[4:].partition('\n---\n')
        if not separator:
            return ''
    return '\n'.join([metadata.get('title', ''), metadata.get('summary', ''), markdown])


def token_rule(note):
    # Require a token increment expression, not a digit in an ID, digest or date.
    return bool(re.search(r'\b(?:increment\w*|advanc\w*)\b[^.\n]{0,100}\b(?:seven|7)\b', semantic_text(note), re.I))


def applicable_wal(note):
    return any(re.search(r'\bsqlite\b', c, re.I) and re.search(r'\bwal\b', c, re.I)
               for c in note.get('metadata', {}).get('conditions', []))


def payload(call):
    if call.get('server') != 'coding_memory' or call.get('status') != 'completed' or call.get('error'):
        return None
    result = call.get('result') or {}
    if result.get('isError') or result.get('is_error'):
        return None
    structured = result.get('structured_content', result.get('structuredContent'))
    if isinstance(structured, dict):
        return structured
    for block in result.get('content', []):
        if block.get('type') == 'text':
            try:
                parsed = json.loads(block['text'])
                if isinstance(parsed, dict):
                    return parsed
            except (ValueError, KeyError):
                pass
    return None


def retrieved_rule(calls, published):
    expected = {n['metadata']['id']: n for n in published if token_rule(n) and applicable_wal(n)}
    hits = set()
    for call in calls:
        data = payload(call)
        if data is None:
            continue
        if call.get('tool') == 'memory_search':
            hits.update(hit.get('metadata', {}).get('id') for hit in data.get('results', [])
                        if hit.get('metadata', {}).get('id') in expected)
        elif call.get('tool') == 'memory_read':
            identity = data.get('metadata', {}).get('id')
            if identity in hits and call.get('arguments', {}).get('id') == identity:
                n = expected[identity]
                if all(data.get(k) == n.get(k) for k in ['metadata', 'markdown', 'revision']):
                    return True
    return False
