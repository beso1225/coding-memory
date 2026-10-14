"""Verify installed Codex calls the Rust MCP server using only synthetic input."""

import argparse
import json
import os
import shutil
import subprocess
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", type=Path, required=True)
    parser.add_argument("--codex", default=shutil.which("codex"))
    args = parser.parse_args()
    import runtime as probe

    engine = args.engine.resolve()
    vault = probe.ROOT / "vault"
    vault.mkdir()
    fixture = Path(__file__).resolve().parents[2] / "tests/fixtures/vault/queue.md"
    (vault / "queue.md").write_text(fixture.read_text())
    state = probe.ROOT / "engine"
    subprocess.run(
        [
            str(engine),
            "--state-dir",
            str(state),
            "index-rebuild",
            "--vault",
            str(vault),
        ],
        check=True,
        capture_output=True,
    )
    with (probe.HOME_DIR / "config.toml").open("a") as config:
        config.write(
            "\n[mcp_servers.coding_memory]\ncommand = "
            + json.dumps(str(engine))
            + "\nargs = "
            + json.dumps(
                [
                    "--state-dir",
                    str(state),
                    "mcp",
                    "--vault",
                    str(vault),
                    "--project-id",
                    "alpha",
                ]
            )
            + "\n"
        )
    for name, arguments, call in [
        ("memory_search", {"query": "sync_all"}, "search"),
        ("memory_read", {"id": "queue-leases"}, "read"),
    ]:
        # Codex 0.162.1 exposes this server as a Responses namespace, not a
        # flattened mcp__server__tool function name.
        probe.PLANS.put(
            {
                "items": [
                    {
                        "type": "function_call",
                        "namespace": "mcp__coding_memory",
                        "name": name,
                        "arguments": json.dumps(arguments),
                        "call_id": call,
                        "id": call,
                    }
                ]
            }
        )
    probe.PLANS.put({"text": "MCP_RETRIEVAL_OK"})
    process = subprocess.run(
        [
            args.codex,
            "exec",
            "--skip-git-repo-check",
            "--json",
            "--disable",
            "hooks",
            "-c",
            "notify=[]",
            "Retrieve the synthetic coding_memory fixture via MCP.",
        ],
        cwd=probe.WORK,
        env={**os.environ, "CODEX_HOME": str(probe.HOME_DIR)},
        text=True,
        capture_output=True,
        check=False,
        timeout=90,
        stdin=subprocess.DEVNULL,
    )
    (probe.ROOT / "mcp-exec.stderr").write_text(process.stderr)
    events = [json.loads(line) for line in process.stdout.splitlines()]
    calls = [
        event["item"]
        for event in events
        if event.get("type") == "item.completed"
        and event.get("item", {}).get("type") == "mcp_tool_call"
    ]
    assert process.returncode == 0, process.returncode
    assert [call["tool"] for call in calls] == ["memory_search", "memory_read"], calls
    assert all(
        call["status"] == "completed" and call["error"] is None for call in calls
    )
    search = calls[0]["result"]["structured_content"]
    read = calls[1]["result"]["structured_content"]
    assert search["results"][0]["metadata"]["id"] == "queue-leases"
    assert "State::claim_store" in read["markdown"]
    assert read["evidence_available"] == [False]
    report = {
        "runtime_version": subprocess.check_output(
            [args.codex, "--version"], text=True
        ).strip(),
        "provider": "synthetic loopback; no authenticated model",
        "state_directory": str(state),
        "calls": calls,
    }
    (probe.ROOT / "mcp-result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(
        json.dumps({"state_directory": str(state), "tools": [c["tool"] for c in calls]})
    )


if __name__ == "__main__":
    main()
