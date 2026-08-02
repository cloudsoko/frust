from __future__ import annotations

import contextlib
import importlib.util
import io
import shutil
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("release-preflight.py")
SPEC = importlib.util.spec_from_file_location("release_preflight", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
PREFLIGHT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PREFLIGHT)

PACKAGE = "@bytecodealliance/jco-transpile"
VERSION = "0.5.2"


def importer(entry: str, package_resolution: bool = True) -> str:
    resolution = f"  '{PACKAGE}@{VERSION}':\n    resolution: {{}}\n" if package_resolution else ""
    return f"""lockfileVersion: '9.0'

importers:

  .:
{entry}

packages:
{resolution}
snapshots:
"""


class PnpmLockValidationTests(unittest.TestCase):
    def assert_rejected(self, lock_text: str) -> None:
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            PREFLIGHT.check_pnpm_lock_dependency(PACKAGE, VERSION, lock_text)

    def test_accepts_the_root_dev_dependency_and_package_resolution(self) -> None:
        lock_text = importer(
            f"""    devDependencies:
      '{PACKAGE}':
        specifier: {VERSION}
        version: {VERSION}"""
        )
        PREFLIGHT.check_pnpm_lock_dependency(PACKAGE, VERSION, lock_text)

    def test_rejects_a_root_runtime_dependency(self) -> None:
        lock_text = importer(
            f"""    devDependencies:
      '@example/tool':
        specifier: 1.0.0
        version: 1.0.0
    dependencies:
      '{PACKAGE}':
        specifier: {VERSION}
        version: {VERSION}"""
        )
        self.assert_rejected(lock_text)

    def test_rejects_a_sibling_importer_dependency(self) -> None:
        lock_text = importer(
            f"""    devDependencies:
      '@example/tool':
        specifier: 1.0.0
        version: 1.0.0
  workspace:
    devDependencies:
      '{PACKAGE}':
        specifier: {VERSION}
        version: {VERSION}"""
        )
        self.assert_rejected(lock_text)

    def test_rejects_a_snapshot_in_place_of_a_package_resolution(self) -> None:
        lock_text = importer(
            f"""    devDependencies:
      '{PACKAGE}':
        specifier: {VERSION}
        version: {VERSION}""",
            package_resolution=False,
        ) + f"  '{PACKAGE}@{VERSION}':\n    dependencies: {{}}\n"
        self.assert_rejected(lock_text)


class ReleaseApprovalTests(unittest.TestCase):
    def test_accepts_the_recorded_release_candidate(self) -> None:
        PREFLIGHT.check_tag(
            "v0.1.0-rc.1",
            "0.1.0",
            {"approved_candidate": "v0.1.0-rc.1"},
        )

    def test_rejects_an_unapproved_release_candidate(self) -> None:
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            PREFLIGHT.check_tag(
                "v0.1.0-rc.2",
                "0.1.0",
                {"approved_candidate": "v0.1.0-rc.1"},
            )


class LegalReadinessTests(unittest.TestCase):
    def fixture(self) -> tuple[tempfile.TemporaryDirectory[str], Path]:
        temporary = tempfile.TemporaryDirectory()
        root = Path(temporary.name)
        for relative, repository in (
            ("", "https://github.com/cloudsoko/frust"),
            ("frust-desk", "https://github.com/cloudsoko/frust-desk"),
            ("frust-ui", "https://github.com/cloudsoko/frust-ui"),
        ):
            component = root / relative
            component.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(PREFLIGHT.ROOT / "LICENSE", component / "LICENSE")
            (component / "NOTICE").write_text(
                f"{PREFLIGHT.LICENSE_ID}\n{repository}\n", encoding="utf-8"
            )
            if relative:
                manifest = f'[package]\nlicense = "{PREFLIGHT.LICENSE_ID}"\n'
                manifest_path = component / "Cargo.toml"
            else:
                manifest = f'[workspace.package]\nlicense = "{PREFLIGHT.LICENSE_ID}"\n'
                manifest_path = component / "frust-kernel" / "Cargo.toml"
                manifest_path.parent.mkdir()
            manifest_path.write_text(manifest, encoding="utf-8")

        for relative in PREFLIGHT.WORKSPACE_LICENSE_MANIFESTS:
            manifest_path = root / relative
            manifest_path.parent.mkdir(parents=True, exist_ok=True)
            manifest_path.write_text(
                "[package]\nlicense.workspace = true\n", encoding="utf-8"
            )
        for relative in PREFLIGHT.STANDALONE_LICENSE_MANIFESTS:
            manifest_path = root / relative
            manifest_path.parent.mkdir(parents=True, exist_ok=True)
            manifest_path.write_text(
                f'[package]\nlicense = "{PREFLIGHT.LICENSE_ID}"\n', encoding="utf-8"
            )

        topcoat = root / "topcoat"
        topcoat.mkdir()
        (topcoat / "Cargo.toml").write_text(
            '[workspace.package]\nlicense = "MIT"\n', encoding="utf-8"
        )
        shutil.copyfile(PREFLIGHT.ROOT / "topcoat" / "LICENSE", topcoat / "LICENSE")
        for component in ("frust-desk", "frust-ui"):
            licenses = root / component / "THIRD_PARTY_LICENSES"
            licenses.mkdir()
            shutil.copyfile(topcoat / "LICENSE", licenses / "TOPCOAT-MIT.txt")
        surreal = root / "deploy" / "licenses"
        surreal.mkdir(parents=True)
        shutil.copyfile(
            PREFLIGHT.ROOT / "deploy" / "licenses" / "SURREALDB-BSL-1.1.txt",
            surreal / "SURREALDB-BSL-1.1.txt",
        )
        return temporary, root

    def assert_rejected(self, root: Path) -> None:
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            PREFLIGHT.check_license(True, root)

    def test_accepts_canonical_agpl_metadata_and_notices(self) -> None:
        temporary, root = self.fixture()
        with temporary:
            PREFLIGHT.check_license(True, root)

    def test_rejects_modified_license_text(self) -> None:
        temporary, root = self.fixture()
        with temporary:
            license_path = root / "LICENSE"
            license_path.write_text(
                license_path.read_text(encoding="utf-8").replace("GNU AFFERO", "GNU MODIFIED", 1),
                encoding="utf-8",
            )
            self.assert_rejected(root)

    def test_rejects_missing_component_notice(self) -> None:
        temporary, root = self.fixture()
        with temporary:
            (root / "frust-desk" / "NOTICE").unlink()
            self.assert_rejected(root)

    def test_rejects_spdx_metadata_drift(self) -> None:
        temporary, root = self.fixture()
        with temporary:
            (root / "frust-ui" / "Cargo.toml").write_text(
                '[package]\nlicense = "MIT"\n', encoding="utf-8"
            )
            self.assert_rejected(root)

    def test_rejects_standalone_crate_spdx_metadata_drift(self) -> None:
        temporary, root = self.fixture()
        with temporary:
            manifest = root / "wasm-spike" / "script-engine" / "Cargo.toml"
            manifest.write_text('[package]\nlicense = "MIT"\n', encoding="utf-8")
            self.assert_rejected(root)

    def test_rejects_third_party_license_drift(self) -> None:
        temporary, root = self.fixture()
        with temporary:
            (root / "frust-desk" / "THIRD_PARTY_LICENSES" / "TOPCOAT-MIT.txt").write_text(
                "modified\n", encoding="utf-8"
            )
            self.assert_rejected(root)

    def test_rejects_coordinated_topcoat_license_drift(self) -> None:
        temporary, root = self.fixture()
        with temporary:
            for path in (
                root / "topcoat" / "LICENSE",
                root / "frust-desk" / "THIRD_PARTY_LICENSES" / "TOPCOAT-MIT.txt",
                root / "frust-ui" / "THIRD_PARTY_LICENSES" / "TOPCOAT-MIT.txt",
            ):
                path.write_text("coordinated drift\n", encoding="utf-8")
            self.assert_rejected(root)

    def test_rejects_surrealdb_license_body_drift(self) -> None:
        temporary, root = self.fixture()
        with temporary:
            path = root / "deploy" / "licenses" / "SURREALDB-BSL-1.1.txt"
            path.write_text(
                path.read_text(encoding="utf-8").replace("Change Date", "Changed Date", 1),
                encoding="utf-8",
            )
            self.assert_rejected(root)

if __name__ == "__main__":
    unittest.main()
