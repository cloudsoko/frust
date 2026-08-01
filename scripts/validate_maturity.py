#!/usr/bin/env python3
"""Validate Frust's capability ledger and generate its Markdown view."""

from __future__ import annotations

import argparse
import json
import sys
from collections import Counter
from pathlib import Path
from typing import Any


SCHEMA_VERSION = 1
STATUSES = ("planned", "experimental", "pilot", "production-ready")
IMPLEMENTATION_STATES = ("not-started", "partial", "implemented")
OPERATIONAL_PROOF_STATES = ("none", "automated-only", "staging", "production")
EVIDENCE_KINDS = ("source", "automated-test", "contract", "ci-workflow", "runbook")
VERIFICATION_RESULTS = ("unknown", "passing", "failing")
TOP_LEVEL_KEYS = {"schema_version", "status_vocabulary", "capabilities"}
CAPABILITY_KEYS = {
    "id",
    "name",
    "surface",
    "status",
    "implementation",
    "operational_proof",
    "owner",
    "evidence",
    "verification",
    "operations_artifact",
    "known_gaps",
}
EVIDENCE_KEYS = {"path", "kind"}
VERIFICATION_KEYS = {"command", "automated", "result"}


class LedgerError(ValueError):
    """Raised when the maturity registry violates its schema or claim rules."""


def _expect_exact_keys(value: dict[str, Any], expected: set[str], context: str) -> None:
    missing = sorted(expected - value.keys())
    unknown = sorted(value.keys() - expected)
    if missing:
        raise LedgerError(f"{context}: missing fields: {', '.join(missing)}")
    if unknown:
        raise LedgerError(f"{context}: unknown fields: {', '.join(unknown)}")


def _expect_non_empty_string(value: Any, context: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise LedgerError(f"{context}: expected a non-empty string")
    return value


def _expect_string_list(value: Any, context: str, *, allow_empty: bool = False) -> list[str]:
    if not isinstance(value, list) or (not allow_empty and not value):
        qualifier = "a list" if allow_empty else "a non-empty list"
        raise LedgerError(f"{context}: expected {qualifier} of non-empty strings")
    for index, item in enumerate(value):
        _expect_non_empty_string(item, f"{context}[{index}]")
    return value


def _resolve_repo_file(repo_root: Path, raw_path: Any, context: str) -> Path:
    path_text = _expect_non_empty_string(raw_path, context)
    relative = Path(path_text)
    if relative.is_absolute():
        raise LedgerError(f"{context}: evidence paths must be repository-relative: {path_text}")

    root = repo_root.resolve()
    candidate = (root / relative).resolve()
    try:
        candidate.relative_to(root)
    except ValueError as exc:
        raise LedgerError(f"{context}: path escapes the repository: {path_text}") from exc
    if not candidate.is_file():
        raise LedgerError(f"{context}: evidence file does not exist: {path_text}")
    return candidate


def load_registry(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise LedgerError(f"cannot read registry {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise LedgerError("registry: expected a JSON object")
    return value


def validate_registry(registry: dict[str, Any], repo_root: Path) -> None:
    _expect_exact_keys(registry, TOP_LEVEL_KEYS, "registry")
    if type(registry["schema_version"]) is not int or registry["schema_version"] != SCHEMA_VERSION:
        raise LedgerError(
            f"registry.schema_version: expected {SCHEMA_VERSION}, got {registry['schema_version']!r}"
        )
    if registry["status_vocabulary"] != list(STATUSES):
        raise LedgerError(
            "registry.status_vocabulary: must exactly match the ordered validator vocabulary "
            f"{list(STATUSES)!r}"
        )

    capabilities = registry["capabilities"]
    if not isinstance(capabilities, list) or not capabilities:
        raise LedgerError("registry.capabilities: expected a non-empty list")

    seen_ids: set[str] = set()
    for index, capability in enumerate(capabilities):
        context = f"capabilities[{index}]"
        if not isinstance(capability, dict):
            raise LedgerError(f"{context}: expected an object")
        _expect_exact_keys(capability, CAPABILITY_KEYS, context)

        capability_id = _expect_non_empty_string(capability["id"], f"{context}.id")
        if capability_id in seen_ids:
            raise LedgerError(f"{context}.id: duplicate stable ID: {capability_id}")
        seen_ids.add(capability_id)
        if not all(part and part.replace("-", "").isalnum() for part in capability_id.split(".")):
            raise LedgerError(
                f"{context}.id: use lowercase dot-separated alphanumeric or hyphenated segments"
            )
        if capability_id.lower() != capability_id:
            raise LedgerError(f"{context}.id: stable IDs must be lowercase")

        for field in ("name", "surface", "owner"):
            _expect_non_empty_string(capability[field], f"{context}.{field}")

        status = capability["status"]
        if status not in STATUSES:
            raise LedgerError(f"{context}.status: unsupported status {status!r}")
        implementation = capability["implementation"]
        if implementation not in IMPLEMENTATION_STATES:
            raise LedgerError(f"{context}.implementation: unsupported state {implementation!r}")
        proof = capability["operational_proof"]
        if proof not in OPERATIONAL_PROOF_STATES:
            raise LedgerError(f"{context}.operational_proof: unsupported state {proof!r}")

        evidence = capability["evidence"]
        if not isinstance(evidence, list):
            raise LedgerError(f"{context}.evidence: expected a list")
        evidence_kinds: set[str] = set()
        evidence_paths: set[str] = set()
        for evidence_index, item in enumerate(evidence):
            evidence_context = f"{context}.evidence[{evidence_index}]"
            if not isinstance(item, dict):
                raise LedgerError(f"{evidence_context}: expected an object")
            _expect_exact_keys(item, EVIDENCE_KEYS, evidence_context)
            kind = item["kind"]
            if kind not in EVIDENCE_KINDS:
                raise LedgerError(f"{evidence_context}.kind: unsupported kind {kind!r}")
            _resolve_repo_file(repo_root, item["path"], f"{evidence_context}.path")
            if item["path"] in evidence_paths:
                raise LedgerError(f"{evidence_context}.path: duplicate evidence path {item['path']}")
            evidence_paths.add(item["path"])
            evidence_kinds.add(kind)

        verification = capability["verification"]
        if not isinstance(verification, list) or not verification:
            raise LedgerError(f"{context}.verification: expected a non-empty list")
        passing_automated = False
        has_passing = False
        for verification_index, item in enumerate(verification):
            verification_context = f"{context}.verification[{verification_index}]"
            if not isinstance(item, dict):
                raise LedgerError(f"{verification_context}: expected an object")
            _expect_exact_keys(item, VERIFICATION_KEYS, verification_context)
            _expect_non_empty_string(item["command"], f"{verification_context}.command")
            if not isinstance(item["automated"], bool):
                raise LedgerError(f"{verification_context}.automated: expected a boolean")
            if item["result"] not in VERIFICATION_RESULTS:
                raise LedgerError(
                    f"{verification_context}.result: unsupported result {item['result']!r}"
                )
            if item["result"] == "passing":
                has_passing = True
                passing_automated = passing_automated or item["automated"]

        operations_artifact = capability["operations_artifact"]
        if operations_artifact is not None:
            _resolve_repo_file(repo_root, operations_artifact, f"{context}.operations_artifact")
        _expect_string_list(capability["known_gaps"], f"{context}.known_gaps", allow_empty=True)

        if status == "planned" and has_passing:
            raise LedgerError(f"{context}: planned capabilities cannot claim passing verification")
        if status == "planned" and proof != "none":
            raise LedgerError(f"{context}: planned capabilities must have operational_proof 'none'")
        if status == "production-ready":
            if implementation != "implemented":
                raise LedgerError(f"{context}: production-ready requires implementation 'implemented'")
            if proof != "production":
                raise LedgerError(f"{context}: production-ready requires operational_proof 'production'")
            if not passing_automated or "automated-test" not in evidence_kinds:
                raise LedgerError(
                    f"{context}: production-ready requires passing automated verification and "
                    "automated-test evidence"
                )
            if operations_artifact is None or "runbook" not in evidence_kinds:
                raise LedgerError(
                    f"{context}: production-ready requires an operations artifact and runbook evidence"
                )


def _escape_table(value: str) -> str:
    return value.replace("|", "\\|").replace("\n", " ")


def render_markdown(registry: dict[str, Any]) -> str:
    capabilities = sorted(registry["capabilities"], key=lambda item: item["id"])
    counts = Counter(item["status"] for item in capabilities)
    lines = [
        "# Frust Capability Maturity",
        "",
        "This file is generated from `maturity/capabilities.json`. Do not edit it directly.",
        "Implementation and operational proof are tracked separately; code presence alone does not",
        "make a capability production-ready.",
        "",
        "## Summary",
        "",
        f"- Total capabilities: {len(capabilities)}",
    ]
    lines.extend(f"- `{status}`: {counts.get(status, 0)}" for status in STATUSES)
    lines.extend(
        [
            "",
            "## Status Rules",
            "",
            "- `planned`: intended work; it cannot claim passing verification or operational proof.",
            "- `experimental`: code may exist, but its contract or operating model is still unstable.",
            "- `pilot`: implemented and test-backed for bounded use, without production proof.",
            "- `production-ready`: implemented, passing automated tests, and backed by a runbook and production proof.",
            "",
            "## Capability Matrix",
            "",
            "| ID | Surface | Capability | Status | Implementation | Operational proof | Owner |",
            "| --- | --- | --- | --- | --- | --- | --- |",
        ]
    )
    for item in capabilities:
        row = (
            item["id"],
            item["surface"],
            item["name"],
            item["status"],
            item["implementation"],
            item["operational_proof"],
            item["owner"],
        )
        lines.append("| " + " | ".join(_escape_table(str(value)) for value in row) + " |")

    lines.extend(["", "## Non-Production Gaps", ""])
    for item in capabilities:
        if item["status"] == "production-ready":
            continue
        lines.append(f"- `{item['id']}` (`{item['status']}`): " + " ".join(sorted(item["known_gaps"])))

    lines.extend(
        [
            "",
            "## Maintenance",
            "",
            "Evidence paths, owners, verification commands, and complete gap records live in",
            "`maturity/capabilities.json`.",
            "",
            "Run `python scripts/validate_maturity.py` to validate and regenerate this file.",
            "Run `python scripts/validate_maturity.py --check` in CI to reject invalid claims or drift.",
            "",
        ]
    )
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    default_root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=default_root)
    parser.add_argument("--registry", type=Path, default=Path("maturity/capabilities.json"))
    parser.add_argument("--output", type=Path, default=Path("maturity/CAPABILITIES.md"))
    parser.add_argument("--check", action="store_true", help="fail if generated Markdown is stale")
    args = parser.parse_args(argv)

    root = args.repo_root.resolve()
    registry_path = args.registry if args.registry.is_absolute() else root / args.registry
    output_path = args.output if args.output.is_absolute() else root / args.output
    try:
        registry = load_registry(registry_path)
        validate_registry(registry, root)
        generated = render_markdown(registry)
    except LedgerError as exc:
        print(f"maturity ledger invalid: {exc}", file=sys.stderr)
        return 1

    if args.check:
        try:
            current = output_path.read_text(encoding="utf-8")
        except OSError as exc:
            print(f"maturity ledger drift: cannot read {output_path}: {exc}", file=sys.stderr)
            return 1
        if current != generated:
            print(
                f"maturity ledger drift: regenerate {output_path} with "
                "python scripts/validate_maturity.py",
                file=sys.stderr,
            )
            return 1
        print(f"maturity ledger valid and current: {output_path}")
        return 0

    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(generated, encoding="utf-8", newline="\n")
    print(f"maturity ledger valid; generated {output_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
