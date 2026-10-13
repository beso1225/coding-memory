"""Opt-in synthetic generation using an isolated copy of the existing login."""

import argparse
import json
import shutil
import tempfile
from pathlib import Path

from contract import SCHEMA
from probe_support import credential_copy, execute_role, install_policy


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--allow-external", action="store_true")
    parser.add_argument("--binary", default=shutil.which("codex"))
    parser.add_argument(
        "--auth-source", type=Path, default=Path.home() / ".codex/auth.json"
    )
    parser.add_argument("--model", default="gpt-6.1-sol")
    options = parser.parse_args(argv)
    if not options.allow_external:
        print("held_external_policy")
        return
    if not options.binary:
        parser.error("Codex executable not found; supply --binary")
    root = Path(
        tempfile.mkdtemp(prefix="coding-memory-generation-live-", dir="/private/tmp")
    )
    home = root / "state"
    home.mkdir(mode=0o700)
    work = root / "project"
    work.mkdir()
    install_policy(root, home)
    (root / "schema.json").write_text(json.dumps(SCHEMA))
    results = []
    try:
        with credential_copy(
            options.auth_source, home / "auth.json", root / "refreshed-auth.json"
        ):
            for role in SCHEMA["properties"]["role"]["enum"]:
                out = root / (role + ".json")
                prompt = f"You are the {role} role in a synthetic protocol test. Use no tools. Evidence: a synthetic test reported PASS for fixture E1. Return role={role}, items=[] and approved=false. Do not infer reusable knowledge."
                args = [
                    options.binary,
                    "exec",
                    "--skip-git-repo-check",
                    "--ignore-user-config",
                    "--ignore-rules",
                    "--json",
                    "--sandbox",
                    "read-only",
                    "--enable",
                    "hooks",
                    "--disable",
                    "plugins",
                    "--dangerously-bypass-hook-trust",
                    "-c",
                    "notify=[]",
                    "-c",
                    "project_doc_max_bytes=0",
                    "-c",
                    'web_search="disabled"',
                    "-c",
                    'cli_auth_credentials_store="file"',
                    "--model",
                    options.model,
                    "--output-schema",
                    str(root / "schema.json"),
                    "-o",
                    str(out),
                    prompt,
                ]
                result = execute_role(args, role, options.model, home, work, out)
                results.append(result)
                print(role, result["outcome"], flush=True)
    finally:
        (root / "result.json").write_text(
            json.dumps(
                {
                    "roles": results,
                    "credential_copy_removed": not (home / "auth.json").exists(),
                    "refreshed_credentials_preserved": (
                        root / "refreshed-auth.json"
                    ).exists(),
                    "policy_calls": (root / "policy.jsonl").exists(),
                },
                indent=2,
            )
            + "\n"
        )
        print(root)


if __name__ == "__main__":
    main()
