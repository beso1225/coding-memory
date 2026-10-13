import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import authenticated_probe
from probe_support import credential_copy, execute_role, install_policy


class SupportTests(unittest.TestCase):
    def test_patch_and_collaboration_attempts_are_recorded(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            events = [
                {"type": "item.completed", "item": {"type": kind, "status": "failed"}}
                for kind in ["file_change", "collab_tool_call"]
            ]
            with patch(
                "probe_support.subprocess.run",
                return_value=subprocess.CompletedProcess(
                    [], 0, stdout="\n".join(map(json.dumps, events)), stderr=""
                ),
            ):
                result = execute_role([], "review", "model", root, root, root / "out")
            self.assertEqual(events, result["tool_items"])

    def test_policy_writes_real_json_lines_and_denies_every_call(self):
        with tempfile.TemporaryDirectory(prefix="policy space ") as d:
            root = Path(d)
            home = root / "state"
            home.mkdir()
            install_policy(root, home)
            for _ in range(2):
                command = json.loads((home / "hooks.json").read_text())["hooks"][
                    "PreToolUse"
                ][0]["hooks"][0]["command"]
                r = subprocess.run(
                    command,
                    shell=True,
                    input='{"tool_name":"Bash"}',
                    text=True,
                    capture_output=True,
                    check=False,
                )
                self.assertEqual(2, r.returncode)
            self.assertEqual(
                2,
                len(
                    [
                        json.loads(l)
                        for l in (root / "policy.jsonl").read_text().splitlines()
                    ]
                ),
            )

    def test_partial_copy_is_cleaned_up(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            source = root / "source"
            source.write_text("secret")
            dest = root / "copy"

            def fail(src, out):
                out.write(b"partial")
                raise OSError("copy failed")

            with (
                patch("probe_support.shutil.copyfileobj", side_effect=fail),
                self.assertRaises(OSError),
                credential_copy(source, dest, root / "refreshed"),
            ):
                pass
            self.assertFalse(dest.exists())
            self.assertEqual("secret", source.read_text())

    def test_refreshed_copy_is_preserved_privately(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            source = root / "source"
            source.write_text("old")
            dest = root / "copy"
            fresh = root / "refreshed"
            with credential_copy(source, dest, fresh):
                dest.write_text("new")
            self.assertFalse(dest.exists())
            self.assertEqual("old", source.read_text())
            self.assertEqual("new", fresh.read_text())
            self.assertEqual(0o600, fresh.stat().st_mode & 0o777)

    def test_timeout_and_malformed_final_are_results(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            out = root / "final"
            with patch(
                "probe_support.subprocess.run",
                side_effect=subprocess.TimeoutExpired("codex", 1),
            ):
                self.assertEqual(
                    "timeout",
                    execute_role([], "review", "model", root, root, out)["outcome"],
                )
            with patch(
                "probe_support.subprocess.run",
                return_value=subprocess.CompletedProcess(
                    [], 0, stdout="{}\n", stderr=""
                ),
            ):
                self.assertEqual(
                    "invalid_output",
                    execute_role([], "review", "model", root, root, out)["outcome"],
                )

    def test_denied_probe_never_reads_credentials_or_creates_home(self):
        with (
            patch("authenticated_probe.tempfile.mkdtemp") as create,
            patch("authenticated_probe.credential_copy") as copy,
        ):
            authenticated_probe.main([])
            create.assert_not_called()
            copy.assert_not_called()

    def test_spawn_error_is_a_process_failure(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            with patch("probe_support.subprocess.run", side_effect=FileNotFoundError()):
                self.assertEqual(
                    "process_failure",
                    execute_role([], "review", "model", root, root, root / "out")[
                        "outcome"
                    ],
                )

    def test_stale_output_is_not_reused(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            out = root / "out"
            out.write_text("{}")
            with patch("probe_support.subprocess.run") as run:
                with self.assertRaises(ValueError):
                    execute_role([], "review", "model", root, root, out)
                run.assert_not_called()
