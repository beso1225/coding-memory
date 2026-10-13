"""Export small projections of synthetic runs, excluding runtime instructions."""

import copy
import json
from pathlib import Path
import sys


def project(result):
    cases = copy.deepcopy(result["cases"])
    for case in cases.values():
        for key in ("read", "final_read", "observer_read_before_interrupt"):
            if key in case:
                thread = case[key]["thread"]
                case[key]["thread"] = {
                    k: thread[k]
                    for k in (
                        "id",
                        "forkedFromId",
                        "cliVersion",
                        "source",
                        "historyMode",
                        "status",
                        "turns",
                    )
                }
        if case is cases["compaction"]:
            case["events"] = [
                e
                for e in case["events"]
                if e.get("method") in ("item/started", "item/completed")
            ]

    hooks = []
    for hook in result["hooks"]:
        event = hook["event"]
        ident = event.get("session_id", event.get("thread-id"))
        visible = []
        for path, lines in hook["tails"].items():
            if ident and ident in path:
                for line in lines:
                    row = json.loads(line)
                    visible.append(
                        {
                            "ordinal": row.get("ordinal"),
                            "type": row["type"],
                            "event_type": row["payload"].get("type"),
                        }
                    )
        hooks.append({"event": event, "visible_tail": visible})

    metadata = []
    for rows in result["rollouts"].values():
        meta = rows[0]["payload"]
        metadata.append(
            {
                k: meta[k]
                for k in (
                    "id",
                    "cli_version",
                    "history_mode",
                    "forked_from_id",
                    "forked_from_ordinal_exclusive",
                    "history_base",
                )
                if k in meta
            }
        )

    fixture = {
        "format": "coding-memory-capture-fixture-v1",
        "runtime_version": result["runtime_version"],
        "observation": "isolated installed binary with loopback synthetic Responses provider; not App GUI",
        "cases": cases,
        "hooks": hooks,
        "rollout_metadata": metadata,
    }
    return (
        json.dumps(fixture, indent=2).replace(result["root"], "/synthetic/capture")
        + "\n"
    )


if __name__ == "__main__":
    source = json.loads(Path(sys.argv[1]).read_text())
    Path(sys.argv[2]).write_text(project(source))
