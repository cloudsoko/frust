from __future__ import annotations

import importlib.util
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("ci-changes.py")
ROOT = SCRIPT.parents[2]
TEST_RUNNER = ROOT / "scripts" / "test.ps1"
LIVE_WORKER = ROOT / ".github" / "actions" / "live-worker" / "action.yml"
MVP_WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"
INTEGRATION_WORKFLOW = ROOT / ".github" / "workflows" / "integration.yml"
POWERSHELL = shutil.which("pwsh") or shutil.which("powershell")
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

    def test_code_beside_documentation_keeps_code_gates(self) -> None:
        changes = CI_CHANGES.classify(["README.md", "frust-kernel/kernel/src/rest.rs"])
        self.assertTrue(changes.code)

    def test_manual_and_scheduled_runs_force_every_gate(self) -> None:
        changes = CI_CHANGES.classify([], force_all=True)
        self.assertTrue(changes.code)

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


class CiWorkflowTests(unittest.TestCase):
    def test_protected_workflow_stays_database_free(self) -> None:
        workflow = MVP_WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("name: Checks", workflow)
        self.assertNotIn("live-worker", workflow)
        self.assertNotIn("-Lane offline", workflow)
        self.assertNotIn("FRUST_ROOT_AUTH", workflow)
        self.assertIn("--skip-artifacts", workflow)

    def test_full_integration_is_explicit_and_basic_only(self) -> None:
        workflow = INTEGRATION_WORKFLOW.read_text(encoding="utf-8")
        self.assertNotIn("pull_request:", workflow)
        self.assertIn("FRUST_ROOT_AUTH: basic", workflow)
        self.assertNotIn("root-auth: jwt", workflow)
        self.assertIn("build-artifacts", workflow)


@unittest.skipUnless(POWERSHELL, "PowerShell is required to verify live test shards")
class LiveShardTests(unittest.TestCase):
    def test_exhaustive_workers_serialize_datastore_setup(self) -> None:
        worker = LIVE_WORKER.read_text(encoding="utf-8")
        exhaustive_step = worker.split("- name: Run exhaustive hermetic live shard", 1)[1]
        run_block = exhaustive_step.split("\n      run: >-", 1)[1].split("\n    - name:", 1)[0]
        self.assertEqual(re.findall(r"-TestThreads\s+(\d+)", run_block), ["1"])
        self.assertEqual(re.findall(r"-TimeoutSeconds\s+(\d+)", run_block), ["2400"])

    def listed_targets(self, index: int, count: int) -> list[str]:
        result = subprocess.run(
            [
                POWERSHELL,
                "-NoProfile",
                "-File",
                str(TEST_RUNNER),
                "-Lane",
                "live",
                "-List",
                "-ShardIndex",
                str(index),
                "-ShardCount",
                str(count),
            ],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        lines = result.stdout.splitlines()
        start = next(i for i, line in enumerate(lines) if line.startswith("Live integration targets"))
        end = next(i for i, line in enumerate(lines[start + 1 :], start + 1) if line == "Live library packages:")
        return [line.strip() for line in lines[start + 1 : end] if line.startswith("  ")]

    def test_four_shards_cover_every_live_target_exactly_once(self) -> None:
        complete = self.listed_targets(0, 1)
        shards = [self.listed_targets(index, 4) for index in range(4)]
        selected = [target for shard in shards for target in shard]

        # Shards must stay balanced (a fixed size list here would break on
        # every added test binary; coverage and uniqueness are asserted below).
        sizes = [len(shard) for shard in shards]
        self.assertLessEqual(max(sizes) - min(sizes), 1, sizes)
        self.assertGreater(min(sizes), 0, sizes)
        self.assertEqual(len(selected), len(set(selected)))
        self.assertEqual(sorted(selected), sorted(complete))

    def test_invalid_shard_is_rejected(self) -> None:
        result = subprocess.run(
            [
                POWERSHELL,
                "-NoProfile",
                "-File",
                str(TEST_RUNNER),
                "-Lane",
                "live",
                "-List",
                "-ShardIndex",
                "4",
                "-ShardCount",
                "4",
            ],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
