#!/usr/bin/env python3
"""Classify a CI diff without relying on GitHub's required-check path filters."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import NamedTuple


ARTIFACT_PREFIXES = ("wasm-spike/", "frust-desk/assets/engine/")
ARTIFACT_FILES = {
    ".cargo/config.toml",
    "frust-desk",
    "package.json",
    "pnpm-lock.yaml",
    "rust-toolchain.toml",
    "scripts/frust.ps1",
}


class ChangeSet(NamedTuple):
    code: bool
    artifacts: bool


def is_documentation(path: str) -> bool:
    return path.startswith("frust/") or path.lower().endswith(".md")


def affects_artifacts(path: str) -> bool:
    return path in ARTIFACT_FILES or path.startswith(ARTIFACT_PREFIXES)


def classify(paths: list[str], *, force_all: bool = False) -> ChangeSet:
    if force_all:
        return ChangeSet(code=True, artifacts=True)
    code_paths = [path for path in paths if not is_documentation(path)]
    return ChangeSet(
        code=bool(code_paths),
        artifacts=any(affects_artifacts(path) for path in code_paths),
    )


def auth_matrix(event: str) -> list[str]:
    return ["jwt"] if event == "pull_request" else ["jwt", "basic"]


def changed_paths(base: str, head: str) -> list[str]:
    if not base or set(base) == {"0"}:
        raise ValueError("a real base commit is required")
    result = subprocess.run(
        ["git", "diff", "--name-only", "--diff-filter=ACMRTUXB", base, head],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or "git diff failed")
    return [line for line in result.stdout.splitlines() if line]


def write_outputs(values: dict[str, str]) -> None:
    output = os.environ.get("GITHUB_OUTPUT")
    if output:
        with Path(output).open("a", encoding="utf-8") as destination:
            for name, value in values.items():
                destination.write(f"{name}={value}\n")
    for name, value in values.items():
        print(f"{name}={value}")


def main() -> None:
    event = os.environ.get("GITHUB_EVENT_NAME", "workflow_dispatch")
    force_all = event in {"workflow_dispatch", "schedule"}
    try:
        paths = [] if force_all else changed_paths(
            os.environ.get("CI_BASE_SHA", ""), os.environ.get("GITHUB_SHA", "HEAD")
        )
    except (ValueError, RuntimeError) as exc:
        print(f"error: cannot classify CI changes: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc

    changes = classify(paths, force_all=force_all)
    write_outputs(
        {
            "code": str(changes.code).lower(),
            "artifacts": str(changes.artifacts).lower(),
            "auth_matrix": json.dumps(auth_matrix(event), separators=(",", ":")),
        }
    )
    print(f"classified {len(paths)} changed path(s)")


if __name__ == "__main__":
    main()
