#!/usr/bin/env python3
"""Verify committed staging evidence against its raw report and GitHub records."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_RECORD = Path("maturity/evidence/v0.1.0-rc.3-staging.json")
HEX_SHA256 = re.compile(r"^[0-9a-f]{64}$")
HEX_COMMIT = re.compile(r"^[0-9a-f]{40}$")
RECOVERY_REPORT = re.compile(
    r"^backup (?:complete|verified): tenant=(?P<tenant>\S+) "
    r"(?:topology=(?P<topology>\S+) )?target=(?P<target>\S+) "
    r"(?:output=\S+ )?bytes=(?P<bytes>\d+) sha256=(?P<sha>[0-9a-f]{64})$"
)
TIMING_MAP = {
    "baseline_deploy": "baseline_deploy_ms",
    "upgrade_to_candidate": "upgrade_to_candidate_ms",
    "rollback_to_baseline": "rollback_to_baseline_ms",
    "redeploy_candidate": "redeploy_candidate_ms",
    "backup": "backup_ms",
    "restore": "restore_ms",
}
RELEASE_ASSET_DIGESTS = {
    "linux_archive_sha256": "frust-{tag}-linux-x86_64.zip",
    "linux_sbom_sha256": "frust-{tag}-linux-x86_64.sbom.spdx.json",
    "windows_archive_sha256": "frust-{tag}-windows-x86_64.zip",
    "windows_sbom_sha256": "frust-{tag}-windows-x86_64.sbom.spdx.json",
}


class EvidenceError(ValueError):
    """Raised when recorded evidence cannot be reproduced."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise EvidenceError(message)


def exact_keys(value: dict[str, Any], expected: set[str], label: str) -> None:
    actual = set(value)
    require(actual == expected, f"{label} fields differ: expected {sorted(expected)}, got {sorted(actual)}")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def load_object(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise EvidenceError(f"cannot load {path}: {exc}") from exc
    require(isinstance(value, dict), f"{path}: expected a JSON object")
    return value


def embedded_report(raw: dict[str, Any], name: str) -> str:
    try:
        event = json.loads(raw["recovery_reports"][name])
        report = event["report"]
    except (KeyError, TypeError, json.JSONDecodeError) as exc:
        raise EvidenceError(f"raw recovery report {name!r} is invalid") from exc
    require(event.get("evt") == "recovery_complete", f"{name} is not a recovery completion event")
    require(isinstance(report, str), f"{name}.report must be text")
    return report


def normalized_utc(value: str) -> str:
    return value[:-6] + "Z" if value.endswith("+00:00") else value


def verify_local(record: dict[str, Any], raw: dict[str, Any], raw_digest: str) -> None:
    exact_keys(
        record,
        {
            "schema_version",
            "record_type",
            "candidate",
            "deployment",
            "supply_chain_gates",
            "timings_ms",
            "checks",
            "recovery",
            "source_evidence",
        },
        "record",
    )
    require(record["schema_version"] == 1, "unsupported record schema")
    require(record["record_type"] == "release-candidate-staging-drill", "unsupported record type")

    candidate = record["candidate"]
    deployment = record["deployment"]
    supply = record["supply_chain_gates"]
    source = record["source_evidence"]
    recovery = record["recovery"]
    exact_keys(
        candidate,
        {"tag", "commit", "release_url", "release_workflow_run", "published_at", "prerelease"},
        "candidate",
    )
    exact_keys(
        deployment,
        {
            "environment",
            "workflow_run_id",
            "workflow_run_url",
            "orchestration_commit",
            "baseline_commit",
            "started_at",
            "finished_at",
            "elapsed_ms",
            "result",
        },
        "deployment",
    )
    exact_keys(
        source,
        {"artifact_id", "artifact_name", "retention_days", "raw_report_path", "raw_report_sha256"},
        "source_evidence",
    )
    exact_keys(
        supply,
        {
            "published_prerelease_resolved",
            "candidate_commit_pinned",
            "release_preflight_passed",
            "archive_sha256_verified",
            "archive_sigstore_verified",
            "sbom_sigstore_verified",
            "github_build_provenance_verified",
            "linux_archive_sha256",
            "linux_sbom_sha256",
            "windows_archive_sha256",
            "windows_sbom_sha256",
        },
        "supply_chain_gates",
    )
    exact_keys(
        recovery,
        {
            "tenant",
            "topology",
            "namespace",
            "database",
            "backup_bytes",
            "backup_sha256",
            "backup_verified",
            "safety_backup_created",
            "sessions_invalidated",
            "post_restore_keyguard_passed",
        },
        "recovery",
    )

    require(raw.get("schema_version") == 1, "unsupported raw report schema")
    require(raw.get("status") == "passed" and raw.get("error") is None, "raw drill did not pass")
    require(candidate["tag"] == raw.get("tag"), "candidate tag differs from raw report")
    require(candidate["commit"] == raw.get("commit"), "candidate commit differs from raw report")
    require(HEX_COMMIT.fullmatch(candidate["commit"]) is not None, "candidate commit is invalid")
    expected_prerelease = "-rc." in str(candidate.get("tag", ""))
    require(
        candidate["prerelease"] is expected_prerelease,
        "candidate prerelease flag must match its tag kind (prerelease for rc tags, full release for finals)",
    )
    require(
        candidate["release_url"] == f"https://github.com/cloudsoko/frust/releases/tag/{candidate['tag']}",
        "release URL does not match the candidate tag",
    )
    release_run_id = candidate["release_workflow_run"].rsplit("/", 1)[-1]
    require(release_run_id.isdigit() and int(release_run_id) > 0, "release workflow run URL is invalid")
    require(
        candidate["release_workflow_run"]
        == f"https://github.com/cloudsoko/frust/actions/runs/{release_run_id}",
        "release workflow run URL is invalid",
    )

    require(deployment["environment"] == raw.get("environment"), "deployment environment differs")
    require(deployment["baseline_commit"] == raw.get("baseline_commit"), "baseline commit differs")
    require(HEX_COMMIT.fullmatch(deployment["orchestration_commit"]) is not None, "orchestration commit is invalid")
    require(deployment["started_at"] == normalized_utc(raw["started_at"]), "start time differs")
    require(deployment["finished_at"] == normalized_utc(raw["finished_at"]), "finish time differs")
    require(deployment["elapsed_ms"] == raw.get("elapsed_ms"), "elapsed duration differs")
    require(deployment["result"] == raw.get("status"), "deployment result differs")
    run_id = deployment["workflow_run_id"]
    require(type(run_id) is int and run_id > 0, "workflow run ID must be positive")
    require(raw.get("project") == f"frust-rc-{run_id}", "raw project does not bind the workflow run")
    require(
        deployment["workflow_run_url"] == f"https://github.com/cloudsoko/frust/actions/runs/{run_id}",
        "workflow run URL does not match its ID",
    )

    require(
        record["timings_ms"] == {name: raw["timings_ms"][raw_name] for name, raw_name in TIMING_MAP.items()},
        "derived drill timings differ from the raw report",
    )
    require(record["checks"] == raw.get("checks"), "derived checks differ from the raw report")
    require(record["checks"] and all(value is True for value in record["checks"].values()), "not all drill checks passed")

    backup = RECOVERY_REPORT.fullmatch(embedded_report(raw, "backup"))
    verified = RECOVERY_REPORT.fullmatch(embedded_report(raw, "verify"))
    require(backup is not None and verified is not None, "backup or verification report is malformed")
    manifest = json.loads(raw["recovery_reports"]["manifest"])
    restore = embedded_report(raw, "restore")
    target = f"{recovery['namespace']}/{recovery['database']}"
    require(backup["tenant"] == recovery["tenant"] == verified["tenant"], "recovery tenant differs")
    require(verified["topology"] == recovery["topology"] == manifest["topology"], "recovery topology differs")
    require(backup["target"] == target == verified["target"], "recovery target differs")
    require(manifest["namespace"] == recovery["namespace"], "recovery namespace differs")
    require(manifest["database"] == recovery["database"], "recovery database differs")
    require(manifest["tenant"] == recovery["tenant"] and manifest["tenant_isolated"] is True, "manifest tenant isolation differs")
    require(int(backup["bytes"]) == int(verified["bytes"]) == manifest["dump_bytes"] == recovery["backup_bytes"], "backup byte counts differ")
    require(backup["sha"] == verified["sha"] == manifest["dump_sha256"] == recovery["backup_sha256"], "backup SHA-256 values differ")
    require(HEX_SHA256.fullmatch(recovery["backup_sha256"]) is not None, "backup SHA-256 is invalid")
    require(recovery["backup_verified"] is True, "backup verification was not recorded")
    require(recovery["safety_backup_created"] is ("safety_backup=" in restore), "safety-backup result differs")
    require(recovery["sessions_invalidated"] is ("all sessions invalidated" in restore), "session invalidation result differs")
    require(recovery["post_restore_keyguard_passed"] is ("keyguard passed" in restore), "keyguard result differs")

    require(source["raw_report_sha256"] == raw_digest, "raw report SHA-256 differs")
    require(HEX_SHA256.fullmatch(source["raw_report_sha256"]) is not None, "raw report SHA-256 is invalid")
    require(type(source["artifact_id"]) is int and source["artifact_id"] > 0, "artifact ID must be positive")
    require(source["retention_days"] == 90, "unexpected artifact retention period")
    require(
        source["artifact_name"] == f"staging-evidence-{candidate['tag']}-{run_id}",
        "artifact name does not bind the candidate and workflow run",
    )

    for name, value in supply.items():
        if name.endswith("_sha256"):
            require(isinstance(value, str) and HEX_SHA256.fullmatch(value) is not None, f"{name} is invalid")
        else:
            require(value is True, f"supply-chain gate {name} did not pass")


def github_json(repository: str, path: str) -> Any:
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    request = urllib.request.Request(
        f"https://api.github.com/repos/{repository}{path}",
        headers={
            "Accept": "application/vnd.github+json",
            "User-Agent": "frust-staging-evidence-verifier",
            **({"Authorization": f"Bearer {token}"} if token else {}),
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except (OSError, urllib.error.HTTPError, json.JSONDecodeError) as exc:
        raise EvidenceError(f"GitHub API request failed for {path}: {exc}") from exc


def successful_step(job: dict[str, Any], name: str) -> None:
    steps = {step["name"]: step.get("conclusion") for step in job.get("steps", [])}
    require(steps.get(name) == "success", f"GitHub step did not pass: {name}")


def verify_github(record: dict[str, Any], repository: str) -> None:
    candidate = record["candidate"]
    deployment = record["deployment"]
    source = record["source_evidence"]
    supply = record["supply_chain_gates"]
    run_id = deployment["workflow_run_id"]

    run = github_json(repository, f"/actions/runs/{run_id}")
    require(run.get("conclusion") == "success", "staging workflow did not succeed")
    require(run.get("head_sha") == deployment["orchestration_commit"], "staging workflow commit differs")
    jobs = github_json(repository, f"/actions/runs/{run_id}/jobs?per_page=100")["jobs"]
    staging_jobs = [job for job in jobs if job.get("name") == "staging / staging"]
    require(len(staging_jobs) == 1 and staging_jobs[0].get("conclusion") == "success", "staging job did not succeed")
    for step in (
        "Resolve immutable published candidate",
        "Enforce candidate approval and legal readiness",
        "Verify signatures, checksum, and provenance",
        "Deploy, upgrade, rollback, and restore in staging",
        "Upload staging and recovery evidence",
    ):
        successful_step(staging_jobs[0], step)

    artifact = github_json(repository, f"/actions/artifacts/{source['artifact_id']}")
    require(artifact.get("id") == source["artifact_id"], "GitHub artifact ID differs")
    require(artifact.get("name") == source["artifact_name"], "GitHub artifact name differs")
    require(artifact.get("workflow_run", {}).get("id") == run_id, "GitHub artifact belongs to another run")

    release = github_json(repository, f"/releases/tags/{candidate['tag']}")
    require(release.get("html_url") == candidate["release_url"], "GitHub release URL differs")
    expected_prerelease = "-rc." in str(candidate.get("tag", ""))
    require(
        release.get("draft") is False and release.get("prerelease") is expected_prerelease,
        "GitHub release state differs",
    )
    require(release.get("published_at") == candidate["published_at"], "GitHub release publication time differs")
    assets = {asset["name"]: asset for asset in release.get("assets", [])}
    for field, template in RELEASE_ASSET_DIGESTS.items():
        name = template.format(tag=candidate["tag"])
        require(name in assets, f"GitHub release asset is missing: {name}")
        require(assets[name].get("digest") == f"sha256:{supply[field]}", f"GitHub digest differs: {name}")

    commit = github_json(repository, f"/commits/{candidate['tag']}")
    require(commit.get("sha") == candidate["commit"], "candidate tag resolves to another commit")
    release_run_id = int(candidate["release_workflow_run"].rsplit("/", 1)[-1])
    release_jobs = github_json(repository, f"/actions/runs/{release_run_id}/jobs?per_page=100")["jobs"]
    by_name = {job["name"]: job for job in release_jobs}
    for name in ("Build linux-x86_64", "Build windows-x86_64", "publish"):
        require(by_name.get(name, {}).get("conclusion") == "success", f"release job did not pass: {name}")
    successful_step(by_name["Build linux-x86_64"], "Attest archive provenance")
    successful_step(by_name["Build windows-x86_64"], "Attest archive provenance")
    successful_step(by_name["publish"], "Verify signed payloads before publishing")
    successful_step(by_name["publish"], "Publish immutable GitHub release")


def verify_files(record_path: Path, *, online: bool = False, repository: str = "cloudsoko/frust") -> None:
    record = load_object(record_path)
    source = record.get("source_evidence")
    require(isinstance(source, dict), "record has no source_evidence object")
    raw_relative = Path(str(source.get("raw_report_path", "")))
    require(raw_relative and not raw_relative.is_absolute(), "raw report path must be repository-relative")
    raw_path = (ROOT / raw_relative).resolve()
    require(raw_path.is_relative_to(ROOT.resolve()), "raw report path escapes the repository")
    raw = load_object(raw_path)
    verify_local(record, raw, sha256(raw_path))
    if online:
        verify_github(record, repository)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--record", type=Path, default=DEFAULT_RECORD)
    parser.add_argument("--verify-github", action="store_true")
    parser.add_argument("--repository", default="cloudsoko/frust")
    args = parser.parse_args(argv)
    record_path = args.record if args.record.is_absolute() else ROOT / args.record
    try:
        verify_files(record_path, online=args.verify_github, repository=args.repository)
    except EvidenceError as exc:
        print(f"staging evidence invalid: {exc}", file=sys.stderr)
        return 1
    scope = "local and GitHub" if args.verify_github else "local"
    print(f"staging evidence verified ({scope}): {record_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
