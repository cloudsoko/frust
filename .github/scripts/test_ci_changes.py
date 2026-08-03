from __future__ import annotations

import importlib.util
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("ci-changes.py")
SPEC = importlib.util.spec_from_file_location("ci_changes", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
CI_CHANGES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI_CHANGES)


class ChangeClassificationTests(unittest.TestCase):
    def test_vault_and_markdown_only_changes_skip_code_gates(self) -> None:
        changes = CI_CHANGES.classify(
            ["frust/03 Architecture Decisions/ADR-016.md", "deploy/recovery-runbook.md"]
        )
        self.assertFalse(changes.code)
        self.assertFalse(changes.artifacts)

    def test_code_beside_documentation_keeps_code_gates(self) -> None:
        changes = CI_CHANGES.classify(["README.md", "frust-kernel/kernel/src/rest.rs"])
        self.assertTrue(changes.code)
        self.assertFalse(changes.artifacts)

    def test_artifact_sources_and_desk_gitlink_trigger_rebuilds(self) -> None:
        for path in (
            "wasm-spike/script-engine/src/lib.rs",
            "frust-desk/assets/engine/script_engine.js",
            "frust-desk",
            "pnpm-lock.yaml",
            "rust-toolchain.toml",
        ):
            with self.subTest(path=path):
                changes = CI_CHANGES.classify([path])
                self.assertTrue(changes.code)
                self.assertTrue(changes.artifacts)

    def test_manual_and_scheduled_runs_force_every_gate(self) -> None:
        changes = CI_CHANGES.classify([], force_all=True)
        self.assertTrue(changes.code)
        self.assertTrue(changes.artifacts)

    def test_pull_requests_smoke_jwt_while_protected_main_runs_both_modes(self) -> None:
        self.assertEqual(CI_CHANGES.auth_matrix("pull_request"), ["jwt"])
        self.assertEqual(CI_CHANGES.auth_matrix("push"), ["jwt", "basic"])
        self.assertEqual(CI_CHANGES.auth_matrix("workflow_dispatch"), ["jwt", "basic"])

    def test_deleted_files_remain_in_the_classified_diff(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repository = Path(temporary)
            commands = (
                ["git", "init", "--quiet"],
                ["git", "config", "user.name", "CI Test"],
                ["git", "config", "user.email", "ci@example.invalid"],
            )
            for command in commands:
                subprocess.run(command, cwd=repository, check=True)

            source = repository / "kernel.rs"
            source.write_text("fn main() {}\n", encoding="utf-8")
            subprocess.run(["git", "add", "kernel.rs"], cwd=repository, check=True)
            subprocess.run(["git", "commit", "--quiet", "-m", "add source"], cwd=repository, check=True)
            base = subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=repository,
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()

            source.unlink()
            subprocess.run(["git", "add", "kernel.rs"], cwd=repository, check=True)
            subprocess.run(
                ["git", "commit", "--quiet", "-m", "delete source"], cwd=repository, check=True
            )
            head = subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=repository,
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()

            self.assertEqual(CI_CHANGES.changed_paths(base, head, cwd=repository), ["kernel.rs"])


if __name__ == "__main__":
    unittest.main()
