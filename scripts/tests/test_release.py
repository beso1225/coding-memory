import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch


def module(name):
    spec = importlib.util.spec_from_file_location(
        name, Path(__file__).resolve().parents[1] / f"{name}.py"
    )
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


class ReleaseTests(unittest.TestCase):
    def test_draft_release_is_found_across_paginated_release_lists(self):
        release = module("release")
        draft = {"id": 42, "tag_name": "v0.1.0-rc.2", "draft": True}
        result = subprocess.CompletedProcess([], 0, json.dumps([
            [{"id": 1, "tag_name": "v0.0.1", "draft": False}], [draft]
        ]), "")
        with patch.object(release, "gh", return_value=result) as api:
            self.assertEqual(release.find_release("owner/repo", "v0.1.0-rc.2"), draft)
            self.assertIn("--paginate", api.call_args.args)
            self.assertNotIn("releases/tags", " ".join(api.call_args.args))

    def test_archive_cannot_escape_the_extraction_directory(self):
        smoke = module("release_smoke")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "bad.tar.gz"
            with tarfile.open(archive, "w:gz") as stream:
                entry = tarfile.TarInfo("../outside")
                entry.size = 3
                stream.addfile(entry, io.BytesIO(b"bad"))
            with self.assertRaisesRegex(ValueError, "Unsafe archive"):
                smoke.extract(archive, root / "unpacked")
            self.assertFalse((root / "outside").exists())

    def test_publication_refuses_dry_run_or_mismatched_commit_without_api_calls(self):
        release = module("release")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for publish, ref, sha in [(False, "refs/heads/main", "expected"),
                                      (True, "refs/heads/topic", "expected"),
                                      (True, "refs/heads/main", "wrong")]:
                (root / "plan.json").write_text(json.dumps({"publish": publish, "sha": "expected"}))
                with patch.dict(os.environ, {"GITHUB_REF": ref, "GITHUB_SHA": sha}), patch.object(release, "gh") as api:
                    with self.assertRaises(ValueError):
                        release.publish(root)
                    api.assert_not_called()

    def test_published_release_is_not_overwritten(self):
        release = module("release")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "plan.json").write_text(json.dumps({"publish": True, "sha": "expected", "tag": "v0.1.0"}))
            with patch.dict(os.environ, {"GITHUB_REF": "refs/heads/main", "GITHUB_SHA": "expected", "GITHUB_REPOSITORY": "owner/repo"}), patch.object(release, "api_optional", return_value=None), patch.object(release, "find_release", return_value={"draft": False}), patch.object(release, "gh") as api:
                with self.assertRaisesRegex(ValueError, "immutable"):
                    release.publish(root)
                api.assert_not_called()

    def test_existing_draft_is_verified_by_id_before_announcement(self):
        release = module("release")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "plan.json").write_text(json.dumps({"publish": True, "sha": "expected", "tag": "v0.1.0-rc.2", "prerelease": True}))
            (root / "notes.md").write_text("Release notes")
            (root / "artifacts").mkdir()
            (root / "artifacts/archive.tar.gz").write_bytes(b"tested archive")
            def respond(*args, **kwargs):
                if args == ("api", "repos/owner/repo/commits/v0.1.0-rc.2"):
                    data = {"sha": "expected"}
                elif args == ("api", "repos/owner/repo/releases/42"):
                    data = {"assets": [{"name": "archive.tar.gz"}]}
                else:
                    data = {}
                return subprocess.CompletedProcess([], 0, json.dumps(data), "")
            with patch.dict(os.environ, {"GITHUB_REF": "refs/heads/main", "GITHUB_SHA": "expected", "GITHUB_REPOSITORY": "owner/repo"}), patch.object(release, "api_optional", return_value={"object": {}}), patch.object(release, "find_release", return_value={"id": 42, "draft": True}), patch.object(release, "gh", side_effect=respond) as api:
                release.publish(root)
                calls = [call.args for call in api.call_args_list]
                self.assertIn(("api", "repos/owner/repo/releases/42"), calls)
                self.assertFalse(any("create" in call for call in calls))
                self.assertIn("--draft=false", calls[-1])

    def test_api_authentication_failure_is_not_treated_as_a_missing_release(self):
        release = module("release")
        failed = subprocess.CompletedProcess([], 1, "", "authentication failed (HTTP 401)")
        with patch.object(release, "gh", return_value=failed):
            with self.assertRaisesRegex(RuntimeError, "authentication"):
                release.api_optional("repos/owner/repo/releases/tags/v0.1.0")

    def test_changelog_must_contain_exactly_one_nonempty_version_section(self):
        release = module("release")
        self.assertEqual(
            release.release_notes("# Changelog\n\n## [0.1.0-rc.1]\n\n- Added.\n\n## [0.0.1]\nOld.\n", "0.1.0-rc.1"),
            "- Added.\n",
        )
        for text in ["## [0.1.0]\n", "## [0.1.0]\nA\n## [0.1.0]\nB\n", "## [0.2.0]\nA\n"]:
            with self.assertRaises(ValueError):
                release.release_notes(text, "0.1.0")

    def test_unknown_dependency_license_blocks_packaging(self):
        notices = module("release_notices")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "LICENSE").write_text("permission")
            package = {"name": "unexpected", "version": "1.0.0", "license": "GPL-3.0-only", "manifest_path": str(root / "Cargo.toml")}
            with self.assertRaisesRegex(ValueError, "Unreviewed license"):
                notices.package_notices(package)

    def test_missing_license_text_blocks_packaging(self):
        notices = module("release_notices")
        with tempfile.TemporaryDirectory() as directory:
            package = {"name": "missing", "version": "1.0.0", "license": "MIT", "manifest_path": str(Path(directory) / "Cargo.toml")}
            with self.assertRaisesRegex(ValueError, "Missing license"):
                notices.package_notices(package)

    def test_nested_notices_and_unicode_license_are_preserved(self):
        notices = module("release_notices")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "LICENSE-MIT").write_text("mit permission")
            (root / "LICENSE-UNICODE").write_text("unicode permission")
            (root / "src/spin").mkdir(parents=True)
            (root / "src/spin/LICENSE").write_text("nested copyright")
            package = {"name": "example", "version": "1.0.0", "license": "(MIT OR Apache-2.0) AND Unicode-3.0", "manifest_path": str(root / "Cargo.toml")}
            text, inventory = notices.package_notices(package)
            for expected in ["mit permission", "unicode permission", "nested copyright"]:
                self.assertIn(expected, text)
            self.assertEqual(len(inventory), 3)

    def test_sdk_supplement_is_bound_to_published_source_revision(self):
        notices = module("release_notices")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".cargo_vcs_info.json").write_text('{"git":{"sha1":"wrong"}}')
            package = {"name": "rmcp", "version": "3.5.1", "license": "Apache-2.0", "manifest_path": str(root / "Cargo.toml")}
            with self.assertRaisesRegex(ValueError, "SDK source revision"):
                notices.package_notices(package)


if __name__ == "__main__":
    unittest.main()
