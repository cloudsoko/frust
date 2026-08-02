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
LICENSE_ID = "AGPL-3.0-only"
AGPL_V3_SHA256 = "8d56b405468aad11f87ab5763f901e276e08d9646ff5c8481b1762b6b789e9ed"
TOPCOAT_MIT_SHA256 = "d01d24e419be110d18be7e4944ecf1355d730189f35002df029b6cea9ec56a63"
SURREALDB_BSL_SHA256 = "572bd7844df95c83477520c334a8ab0ab5c4642cef9cb2c7f8c664454b2efe86"
WORKSPACE_LICENSE_MANIFESTS = (
    "frust-kernel/app-sdk/Cargo.toml",
    "frust-kernel/kernel/Cargo.toml",
    "frust-kernel/orm/Cargo.toml",
)
STANDALONE_LICENSE_MANIFESTS = (
    "frust-kernel/app-sdk/templates/rust-hook/Cargo.toml",
    "wasm-spike/plugin-demo/Cargo.toml",
    "wasm-spike/script-engine/Cargo.toml",
)


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
    if package.get("license") != LICENSE_ID:
        fail(f"kernel workspace license must be {LICENSE_ID}")
    if package.get("publish") is not False:
        fail("kernel workspace packages must remain publish = false until crates.io policy is decided")
    return version


def mapping_end(lines: list[str], start: int, indent: int, limit: int) -> int:
    for index in range(start + 1, limit):
        line = lines[index]
        if line.strip() and len(line) - len(line.lstrip()) <= indent:
            return index
    return limit


def check_pnpm_lock_dependency(package: str, expected: str, lock_text: str | None = None) -> None:
    if lock_text is None:
        lock_text = (ROOT / "wasm-spike" / "pnpm-lock.yaml").read_text("utf-8")
    lines = lock_text.splitlines()
    try:
        importers = lines.index("importers:")
        packages = lines.index("packages:", importers + 1)
        root_importer = lines.index("  .:", importers + 1, packages)
        root_end = mapping_end(lines, root_importer, 2, packages)
        dev_dependencies = lines.index("    devDependencies:", root_importer + 1, root_end)
        dev_end = mapping_end(lines, dev_dependencies, 4, root_end)
        dependency = lines.index(f"      '{package}':", dev_dependencies + 1, dev_end)
    except ValueError:
        fail(f"pnpm lockfile has no root devDependency entry for {package}")

    values: dict[str, str] = {}
    dependency_end = mapping_end(lines, dependency, 6, dev_end)
    for line in lines[dependency + 1 : dependency_end]:
        match = re.fullmatch(r"        (specifier|version):\s+(.+)", line)
        if match:
            values[match.group(1)] = match.group(2).strip("'\"")
    if values.get("specifier") != expected or values.get("version") != expected:
        fail(f"pnpm lockfile does not pin {package} exactly to {expected}")
    packages_end = mapping_end(lines, packages, 0, len(lines))
    if f"  '{package}@{expected}':" not in lines[packages + 1 : packages_end]:
        fail(f"pnpm lockfile has no package resolution for {package}@{expected}")


def check_compatibility(version: str) -> dict:
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

    legal = compatibility.get("legal", {})
    if legal.get("license") != LICENSE_ID:
        fail(f"release approval must declare legal.license = {LICENSE_ID!r}")
    approved = legal.get("approved_candidate")
    if not isinstance(approved, str) or not SEMVER_TAG.fullmatch(approved):
        fail("release approval must declare a SemVer legal.approved_candidate")
    if approved.removeprefix("v").split("-", 1)[0] != version:
        fail("approved release candidate does not match the framework version")

    surreal = compatibility["framework"]["surrealdb"]
    lanes = json.loads((ROOT / "test" / "lanes.json").read_text("utf-8"))
    if lanes.get("surreal", {}).get("image") != f"surrealdb/surrealdb:v{surreal}":
        fail("SurrealDB test image does not match compatibility metadata")

    build = compatibility["build"]
    if "jco" in build:
        fail("legacy build.jco metadata is unsupported; use build.jco_transpile")
    package = json.loads((ROOT / "wasm-spike" / "package.json").read_text("utf-8"))
    pnpm = build["pnpm"]
    if package.get("packageManager") != f"pnpm@{pnpm}":
        fail("pnpm packageManager does not match compatibility metadata")
    jco_transpile = build["jco_transpile"]
    dependency = package.get("devDependencies", {}).get("@bytecodealliance/jco-transpile")
    if dependency != jco_transpile:
        fail("jco-transpile package version does not match compatibility metadata")
    check_pnpm_lock_dependency("@bytecodealliance/jco-transpile", jco_transpile)
    return compatibility


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


def check_deployment_line_endings() -> None:
    errors = [
        path.relative_to(ROOT).as_posix()
        for path in sorted((ROOT / "deploy" / "bin").glob("*.sh"))
        if b"\r" in path.read_bytes()
    ]
    if errors:
        fail("deployment shell scripts must use LF line endings: " + ", ".join(errors))


def check_submodules(legal: dict) -> None:
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
    recorded = {
        line[42:].split(" ", 1)[0]: line[1:41]
        for line in result.stdout.splitlines()
        if len(line) >= 42
    }
    for path, field in (
        ("frust-desk", "frust_desk_commit"),
        ("frust-ui", "frust_ui_commit"),
        ("topcoat", "topcoat_commit"),
    ):
        expected = legal.get(field)
        if not isinstance(expected, str) or not re.fullmatch(r"[0-9a-f]{40}", expected):
            fail(f"release/compatibility.toml legal.{field} must be a full commit SHA")
        if recorded.get(path) != expected:
            fail(f"release approval pins {path} at {expected}, checked out {recorded.get(path)!r}")


def normalized_text_sha256(path: Path) -> str:
    text = path.read_text(encoding="utf-8").replace("\r\n", "\n")
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def read_toml(path: Path) -> dict:
    return tomllib.loads(path.read_text(encoding="utf-8-sig"))


def check_agpl_component(root: Path, relative: str, repository: str) -> None:
    component = root / relative if relative else root
    license_path = component / "LICENSE"
    notice_path = component / "NOTICE"
    manifest_path = component / "Cargo.toml" if relative else root / "frust-kernel" / "Cargo.toml"
    label = relative or "root"

    if not license_path.is_file():
        fail(f"{label} has no LICENSE file")
    if normalized_text_sha256(license_path) != AGPL_V3_SHA256:
        fail(f"{label}/LICENSE is not the canonical GNU AGPL version 3 text")
    if not notice_path.is_file():
        fail(f"{label} has no NOTICE file")
    notice = notice_path.read_text(encoding="utf-8")
    if LICENSE_ID not in notice or repository not in notice:
        fail(f"{label}/NOTICE must name {LICENSE_ID} and {repository}")

    manifest = read_toml(manifest_path)
    package = manifest.get("workspace", {}).get("package", {}) if not relative else manifest.get("package", {})
    if package.get("license") != LICENSE_ID:
        fail(f"{label}/Cargo.toml must declare license = {LICENSE_ID!r}")


def check_first_party_cargo_licenses(root: Path) -> None:
    for relative in WORKSPACE_LICENSE_MANIFESTS:
        package = read_toml(root / relative).get("package", {})
        if package.get("license") != {"workspace": True}:
            fail(f"{relative} must declare license.workspace = true")
    for relative in STANDALONE_LICENSE_MANIFESTS:
        package = read_toml(root / relative).get("package", {})
        if package.get("license") != LICENSE_ID:
            fail(f"{relative} must declare license = {LICENSE_ID!r}")


def check_license(required: bool, root: Path = ROOT) -> None:
    if not (root / "LICENSE").is_file():
        message = "no project license has been selected; public releases are legally blocked"
        if required:
            fail(message)
        print(f"warning: {message}", file=sys.stderr)
        return

    check_agpl_component(root, "", "https://github.com/cloudsoko/frust")
    check_agpl_component(root, "frust-desk", "https://github.com/cloudsoko/frust-desk")
    check_agpl_component(root, "frust-ui", "https://github.com/cloudsoko/frust-ui")
    check_first_party_cargo_licenses(root)

    topcoat_manifest = read_toml(root / "topcoat" / "Cargo.toml")
    if topcoat_manifest.get("workspace", {}).get("package", {}).get("license") != "MIT":
        fail("topcoat/Cargo.toml must retain its upstream MIT license")
    if not (root / "topcoat" / "LICENSE").is_file():
        fail("topcoat has no MIT LICENSE file")
    topcoat_license = normalized_text_sha256(root / "topcoat" / "LICENSE")
    if topcoat_license != TOPCOAT_MIT_SHA256:
        fail("topcoat/LICENSE does not match the approved upstream MIT text")
    for component in ("frust-desk", "frust-ui"):
        bundled = root / component / "THIRD_PARTY_LICENSES" / "TOPCOAT-MIT.txt"
        if not bundled.is_file() or normalized_text_sha256(bundled) != topcoat_license:
            fail(f"{component} does not bundle the recorded Topcoat MIT license")

    surreal_license = root / "deploy" / "licenses" / "SURREALDB-BSL-1.1.txt"
    if not surreal_license.is_file():
        fail("the packaged SurrealDB license is missing")
    surreal_text = surreal_license.read_text(encoding="utf-8")
    if "Business Source License 1.1" not in surreal_text or "SurrealDB Ltd." not in surreal_text:
        fail("the packaged SurrealDB license does not identify the pinned upstream terms")
    if normalized_text_sha256(surreal_license) != SURREALDB_BSL_SHA256:
        fail("the packaged SurrealDB license does not match the approved upstream text")
    print(f"license: {LICENSE_ID}; independently licensed components verified")


def check_tag(tag: str | None, version: str, legal: dict) -> None:
    if tag is None:
        return
    if not SEMVER_TAG.fullmatch(tag):
        fail(f"release tag {tag!r} is not vMAJOR.MINOR.PATCH with an optional prerelease suffix")
    if tag.removeprefix("v").split("-", 1)[0] != version:
        fail(f"release tag {tag!r} does not match workspace version {version!r}")
    if legal.get("approved_candidate") != tag:
        fail(f"release tag {tag!r} is not the approved candidate in release/compatibility.toml")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", help="tag being released, for example v0.1.0")
    parser.add_argument("--require-license", action="store_true")
    parser.add_argument("--skip-artifacts", action="store_true")
    parser.add_argument("--skip-submodules", action="store_true")
    args = parser.parse_args()

    version = workspace_version()
    compatibility = check_compatibility(version)
    legal = compatibility.get("legal", {})
    check_tag(args.tag, version, legal)
    check_license(args.require_license)
    check_deployment_line_endings()
    if not args.skip_artifacts:
        check_artifacts()
    if not args.skip_submodules:
        check_submodules(legal)
    print(f"release preflight passed for Frust {version}")


if __name__ == "__main__":
    main()
