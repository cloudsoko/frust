from __future__ import annotations

import copy
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT_PATH = REPO_ROOT / "scripts" / "validate_maturity.py"
SPEC = importlib.util.spec_from_file_location("validate_maturity", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
VALIDATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATOR)


def capability() -> dict:
    return {
        "id": "kernel.example",
        "name": "Example capability",
        "surface": "Kernel",
        "status": "experimental",
        "implementation": "partial",
        "operational_proof": "automated-only",
        "owner": "kernel",
        "evidence": [{"path": "tests/check.py", "kind": "automated-test"}],
        "verification": [
            {"command": "python tests/check.py", "automated": True, "result": "unknown"}
        ],
        "operations_artifact": None,
        "known_gaps": ["Not production proven."],
    }


def registry(*capabilities: dict) -> dict:
    return {
        "schema_version": 1,
        "status_vocabulary": ["planned", "experimental", "pilot", "production-ready"],
        "capabilities": list(capabilities),
    }


class LedgerValidationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name) / "repo"
        (self.root / "tests").mkdir(parents=True)
        (self.root / "tests" / "check.py").write_text("pass\n", encoding="utf-8")
        (self.root / "ops").mkdir()
        (self.root / "ops" / "runbook.md").write_text("# Runbook\n", encoding="utf-8")

    def assert_invalid(self, value: dict, message: str) -> None:
        with self.assertRaisesRegex(VALIDATOR.LedgerError, message):
            VALIDATOR.validate_registry(value, self.root)

    def test_accepts_valid_registry(self) -> None:
        VALIDATOR.validate_registry(registry(capability()), self.root)

    def test_rejects_schema_version_and_unknown_fields(self) -> None:
        wrong_version = registry(capability())
        wrong_version["schema_version"] = 2
        self.assert_invalid(wrong_version, "schema_version")

        extra_field = registry(capability())
        extra_field["capabilities"][0]["confidence"] = "high"
        self.assert_invalid(extra_field, "unknown fields: confidence")

    def test_rejects_duplicate_stable_ids(self) -> None:
        duplicate = copy.deepcopy(capability())
        self.assert_invalid(registry(capability(), duplicate), "duplicate stable ID")

    def test_rejects_missing_and_escaping_evidence(self) -> None:
        missing = capability()
        missing["evidence"][0]["path"] = "tests/missing.py"
        self.assert_invalid(registry(missing), "evidence file does not exist")

        outside = self.root.parent / "outside.txt"
        outside.write_text("outside\n", encoding="utf-8")
        escaping = capability()
        escaping["evidence"][0]["path"] = "../outside.txt"
        self.assert_invalid(registry(escaping), "path escapes the repository")

    def test_production_ready_requires_passing_automated_test_evidence(self) -> None:
        production = capability()
        production.update(
            status="production-ready",
            implementation="implemented",
            operational_proof="production",
            operations_artifact="ops/runbook.md",
        )
        production["verification"][0]["result"] = "passing"
        production["evidence"] = [
            {"path": "tests/check.py", "kind": "source"},
            {"path": "ops/runbook.md", "kind": "runbook"},
        ]
        self.assert_invalid(registry(production), "automated-test evidence")

    def test_accepts_production_ready_only_with_automation_and_runbook(self) -> None:
        production = capability()
        production.update(
            status="production-ready",
            implementation="implemented",
            operational_proof="production",
            operations_artifact="ops/runbook.md",
        )
        production["verification"][0]["result"] = "passing"
        production["evidence"].append({"path": "ops/runbook.md", "kind": "runbook"})
        VALIDATOR.validate_registry(registry(production), self.root)

    def test_production_ready_requires_operations_artifact_and_runbook_evidence(self) -> None:
        production = capability()
        production.update(
            status="production-ready",
            implementation="implemented",
            operational_proof="production",
        )
        production["verification"][0]["result"] = "passing"
        self.assert_invalid(registry(production), "operations artifact and runbook evidence")

    def test_planned_capability_cannot_claim_passing_evidence(self) -> None:
        planned = capability()
        planned.update(status="planned", implementation="not-started", operational_proof="none")
        planned["verification"][0]["result"] = "passing"
        self.assert_invalid(registry(planned), "planned capabilities cannot claim passing")

    def test_markdown_is_deterministic_and_sorted_by_stable_id(self) -> None:
        second = capability()
        second["id"] = "zeta.second"
        first = capability()
        first["id"] = "alpha.first"
        value = registry(second, first)
        rendered_once = VALIDATOR.render_markdown(value)
        rendered_twice = VALIDATOR.render_markdown(copy.deepcopy(value))
        self.assertEqual(rendered_once, rendered_twice)
        self.assertLess(rendered_once.index("`alpha.first`"), rendered_once.index("`zeta.second`"))

    def test_check_mode_detects_generated_markdown_drift(self) -> None:
        registry_path = self.root / "maturity" / "capabilities.json"
        registry_path.parent.mkdir()
        registry_path.write_text(json.dumps(registry(capability())), encoding="utf-8")
        output_path = registry_path.parent / "CAPABILITIES.md"
        output_path.write_text("stale\n", encoding="utf-8")

        result = VALIDATOR.main(
            [
                "--repo-root",
                str(self.root),
                "--registry",
                "maturity/capabilities.json",
                "--output",
                "maturity/CAPABILITIES.md",
                "--check",
            ]
        )
        self.assertEqual(result, 1)


if __name__ == "__main__":
    unittest.main()
