#!/usr/bin/env python3
"""Validate release metadata and reproducible runtime inputs without dependencies."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SEMVER_TAG = re.compile(r"^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?$")


def fail(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(1)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest().upper()


def workspace_version() -> str:
    with (ROOT / "frust-kernel" / "Cargo.toml").open("rb") as source:
        manifest = tomllib.load(source)
    package = manifest.get("workspace", {}).get("package", {})
    version = package.get("version")
    if not isinstance(version, str):
        fail("frust-kernel/Cargo.toml has no workspace.package.version")
    if package.get("publish") is not False:
        fail("kernel workspace packages must remain publish = false until licensing is decided")
    return version


def check_compatibility(version: str) -> None:
    with (ROOT / "release" / "compatibility.toml").open("rb") as source:
        compatibility = tomllib.load(source)
    declared = compatibility.get("framework", {}).get("version")
    if declared != version:
        fail(f"release/compatibility.toml declares {declared!r}, expected {version!r}")

    with (ROOT / "rust-toolchain.toml").open("rb") as source:
        toolchain = tomllib.load(source)
    rust = compatibility["framework"]["rust"]
    if toolchain.get("toolchain", {}).get("channel") != rust:
        fail("compatibility Rust version does not match rust-toolchain.toml")

    artifact_lock = json.loads((ROOT / compatibility["wasm"]["artifact_lock"]).read_text("utf-8"))
    if artifact_lock.get("rust") != rust:
        fail("WASM artifact lock Rust version does not match compatibility metadata")
    if artifact_lock.get("target") != compatibility["wasm"]["target"]:
        fail("WASM artifact target does not match compatibility metadata")


def check_artifacts() -> None:
    lock = json.loads((ROOT / "wasm-spike" / "artifacts.lock.json").read_text("utf-8"))
    errors: list[str] = []
    for relative, expected in lock.get("artifacts", {}).items():
        artifact = ROOT / relative
        if not artifact.is_file():
            errors.append(f"{relative}: missing")
            continue
        actual = sha256(artifact)
        if actual != expected.upper():
            errors.append(f"{relative}: expected {expected.upper()}, got {actual}")
    if errors:
        fail("runtime artifact verification failed:\n  " + "\n  ".join(errors))


def check_submodules() -> None:
    result = subprocess.run(
        ["git", "submodule", "status", "--recursive"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        fail(f"git submodule status failed: {result.stderr.strip()}")
    bad = [line for line in result.stdout.splitlines() if line.startswith(("-", "+", "U"))]
    if bad:
        fail("submodules are missing or not at recorded commits:\n  " + "\n  ".join(bad))


def check_license(required: bool) -> None:
    candidates = [
        path
        for pattern in ("LICENSE", "LICENSE.*", "COPYING", "COPYING.*")
        for path in ROOT.glob(pattern)
        if path.is_file()
    ]
    if candidates:
        print("license: " + ", ".join(path.name for path in sorted(set(candidates))))
        return
    message = "no project license has been selected; public releases are legally blocked"
    if required:
        fail(message)
    print(f"warning: {message}", file=sys.stderr)


def check_tag(tag: str | None, version: str) -> None:
    if tag is None:
        return
    if not SEMVER_TAG.fullmatch(tag):
        fail(f"release tag {tag!r} is not vMAJOR.MINOR.PATCH with an optional prerelease suffix")
    if tag.removeprefix("v").split("-", 1)[0] != version:
        fail(f"release tag {tag!r} does not match workspace version {version!r}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", help="tag being released, for example v0.1.0")
    parser.add_argument("--require-license", action="store_true")
    parser.add_argument("--skip-artifacts", action="store_true")
    parser.add_argument("--skip-submodules", action="store_true")
    args = parser.parse_args()

    version = workspace_version()
    check_compatibility(version)
    check_tag(args.tag, version)
    check_license(args.require_license)
    if not args.skip_artifacts:
        check_artifacts()
    if not args.skip_submodules:
        check_submodules()
    print(f"release preflight passed for Frust {version}")


if __name__ == "__main__":
    main()
