"""Spike-only closed output contract; not the production domain schema."""

import json

SCHEMA = {
    "type": "object",
    "properties": {
        "role": {"type": "string", "enum": ["extraction", "consolidation", "review"]},
        "items": {"type": "array", "items": {"type": "string"}},
        "approved": {"type": "boolean"},
    },
    "required": ["role", "items", "approved"],
    "additionalProperties": False,
}


def classify(role, exit_code, final):
    if exit_code != 0:
        return "process_failure"
    if len(final.encode()) > 65536:
        return "output_limit"
    try:
        value = json.loads(final)
    except (ValueError, TypeError):
        return "invalid_output"
    if not isinstance(value, dict) or set(value) != set(SCHEMA["required"]):
        return "invalid_output"
    if (
        value["role"] != role
        or not isinstance(value["items"], list)
        or not all(isinstance(x, str) for x in value["items"])
        or type(value["approved"]) is not bool
    ):
        return "invalid_output"
    if role == "review" and not value["approved"]:
        return "review_rejected"
    if role == "extraction" and not value["items"]:
        return "empty"
    return "success"


def permitted_call(allow_external, execute):
    return execute() if allow_external else "held_external_policy"


def is_internal(event, owned_stores, owned_threads):
    return (
        event["store"] in owned_stores
        or (event["store"], event["thread"]) in owned_threads
    )
