"""Keep archive checks selective while source changes use Linux CI."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


def load_scope():
    spec = importlib.util.spec_from_file_location(
        "ci_scope", Path(__file__).resolve().parents[1] / "ci_scope.py"
    )
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


class ScopeTests(unittest.TestCase):
    def test_source_changes_do_not_request_archive_jobs(self):
        scope = load_scope()
        for paths in [[], ["docs/worker.md"], ["src/consolidation.rs", "tests/consolidation.rs"],
                      ["src/main.rs", "src/lib.rs"], ["src/service.rs", "src/generation.rs", "src/runtime_probe.rs"],
                      ["tests/worker.rs", "experiments/codex_capture/test_contract.py"]]:
            self.assertEqual(scope.select(paths), {"archive": False})

    def test_packaging_inputs_keep_their_archive_checks(self):
        scope = load_scope()
        for path in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "build_inputs.rs",
                     ".cargo/config.toml", "dist-workspace.toml", "CHANGELOG.md",
                     "scripts/build_release.sh", "scripts/release_notices.py",
                     "scripts/tests/test_release.py", "LICENSE-MIT", "third-party/rmcp-LICENSE"]:
            with self.subTest(path=path):
                self.assertTrue(scope.select([path])["archive"])
        for path in ["flake.nix", "flake.lock", "Taskfile.pkl", "scripts/ci_scope.py",
                     ".github/workflows/ci.yml", ".github/actions/setup-dev/action.yml"]:
            self.assertEqual(scope.select([path]), {"archive": True})

    def test_dashboard_changes_use_linux_checks_without_archive_jobs(self):
        scope = load_scope()
        paths = ["src/dashboard.rs", "src/dashboard_server.rs", "src/dashboard.js",
                 "src/dashboard.html", "tests/dashboard.rs", "tests/dashboard_server.rs"]
        for path in paths:
            with self.subTest(path=path):
                self.assertEqual(scope.select([path]), {"archive": False})
        self.assertEqual(scope.select(paths), {"archive": False})
        self.assertEqual(scope.select(paths + ["src/main.rs"]), {"archive": False})
        self.assertEqual(scope.select(paths + ["build_inputs.rs"]), {"archive": True})

    def test_pr_range_ignores_base_only_changes_and_includes_deleted_inputs(self):
        scope = load_scope()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.check_output(["git", "-C", str(root), *args], text=True, stderr=subprocess.PIPE).strip()
            git("init", "-q")
            git("config", "user.name", "CI test")
            git("config", "user.email", "ci@example.invalid")
            (root / "Cargo.toml").write_text("initial")
            git("add", ".")
            git("commit", "-qm", "initial")
            common = git("rev-parse", "HEAD")
            (root / "Cargo.toml").write_text("base update")
            git("commit", "-qam", "base-only")
            base = git("rev-parse", "HEAD")
            git("checkout", "--detach", common)
            (root / "guide.md").write_text("guide")
            git("add", ".")
            git("commit", "-qm", "docs")
            head = git("rev-parse", "HEAD")
            self.assertEqual(scope.changed_paths(root, base, head, pull_request=True), ["guide.md"])
            git("rm", "Cargo.toml")
            git("commit", "-qm", "remove input")
            paths = scope.changed_paths(root, base, git("rev-parse", "HEAD"), pull_request=True)
            self.assertTrue(scope.select(paths)["archive"])
            self.assertIn("Cargo.toml", scope.changed_paths(root, "0" * 40, base))

    def test_manual_dispatch_selects_archive_checks_without_a_git_range(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            event = root / "event.json"
            event.write_text(json.dumps({}))
            output = root / "output"
            script = Path(__file__).resolve().parents[1] / "ci_scope.py"
            subprocess.run([os.sys.executable, str(script)], cwd=root, check=True,
                           stdout=subprocess.PIPE, env={**os.environ, "GITHUB_EVENT_NAME": "workflow_dispatch",
                                                        "GITHUB_EVENT_PATH": str(event), "GITHUB_OUTPUT": str(output)})
            self.assertEqual(output.read_text(), "archive=true\n")

    def test_invalid_or_unavailable_comparison_fails_instead_of_skipping(self):
        scope = load_scope()
        with self.assertRaises(ValueError):
            scope.changed_paths(Path.cwd(), "--help", "0" * 40)
        with self.assertRaises(subprocess.CalledProcessError):
            scope.changed_paths(Path.cwd(), "f" * 40, "e" * 40)


if __name__ == "__main__":
    unittest.main()
