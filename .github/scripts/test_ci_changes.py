from __future__ import annotations

import importlib.util
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


if __name__ == "__main__":
    unittest.main()
