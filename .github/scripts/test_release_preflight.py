from __future__ import annotations

import contextlib
import importlib.util
import io
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


if __name__ == "__main__":
    unittest.main()
