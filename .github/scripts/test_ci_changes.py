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

        pull_request = CI_CHANGES.live_matrix("pull_request", code=True)["include"]
        self.assertEqual(
            pull_request,
            [
                {
                    "lane": "smoke",
                    "shard_index": 0,
                    "shard_count": 1,
                    "shard_label": "1/1",
                }
            ],
        )

        main = CI_CHANGES.live_matrix("push", code=True)["include"]
        self.assertEqual(len(main), CI_CHANGES.LIVE_SHARD_COUNT)
        self.assertEqual([entry["shard_index"] for entry in main], list(range(4)))
        self.assertTrue(all(entry["lane"] == "live" for entry in main))

        documentation = CI_CHANGES.live_matrix("push", code=False)["include"]
        self.assertEqual(len(documentation), 1)
        self.assertTrue(all(entry["shard_count"] == 1 for entry in documentation))

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


@unittest.skipUnless(POWERSHELL, "PowerShell is required to verify live test shards")
class LiveShardTests(unittest.TestCase):
    def test_exhaustive_workers_keep_datastore_safe_intra_binary_parallelism(self) -> None:
        worker = LIVE_WORKER.read_text(encoding="utf-8")
        exhaustive_step = worker.split("- name: Run exhaustive hermetic live shard", 1)[1]
        run_block = exhaustive_step.split("\n      run: >-", 1)[1].split("\n    - name:", 1)[0]
        self.assertEqual(re.findall(r"-TestThreads\s+(\d+)", run_block), ["2"])

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

        self.assertEqual([len(shard) for shard in shards], [12, 12, 11, 11])
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
