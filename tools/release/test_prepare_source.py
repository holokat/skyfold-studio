"""Focused guards for the public-source release exporter."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("prepare_source.py")
SPEC = importlib.util.spec_from_file_location("prepare_source", SCRIPT)
assert SPEC and SPEC.loader
PREPARE_SOURCE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PREPARE_SOURCE
SPEC.loader.exec_module(PREPARE_SOURCE)


def write_release_skeleton(root: Path) -> None:
    for name in PREPARE_SOURCE.FILES:
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("public fixture\n", encoding="utf-8")
    for name in PREPARE_SOURCE.TREES:
        directory = root / name
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "fixture.rs").write_text("pub fn fixture() {}\n", encoding="utf-8")
    (root / "tools/release/prepare_source.py").write_text("# fixture\n", encoding="utf-8")


class PrepareSourceAuditTests(unittest.TestCase):
    def audit_private(self, change) -> list[object]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_release_skeleton(root)
            change(root)
            failures, _warnings = PREPARE_SOURCE.audit(root)
            return failures

    def test_private_preflight_ignores_editor_backup(self) -> None:
        failures = self.audit_private(
            lambda root: (root / "src/editor.rs.orig").write_text("local backup\n", encoding="utf-8")
        )
        self.assertEqual([], failures)

    def test_private_preflight_rejects_oversized_allowlisted_file(self) -> None:
        def add_large_file(root: Path) -> None:
            (root / "src/large.rs").write_bytes(b"x" * (PREPARE_SOURCE.MAX_FILE_BYTES + 1))

        failures = self.audit_private(add_large_file)
        self.assertTrue(any(finding.kind == "size" for finding in failures))

    def test_private_preflight_rejects_symlink(self) -> None:
        def add_symlink(root: Path) -> None:
            (root / "src/link.rs").symlink_to(root / "src/fixture.rs")

        failures = self.audit_private(add_symlink)
        self.assertTrue(any(finding.kind == "symlink" for finding in failures))

    def test_private_preflight_rejects_credential_pattern(self) -> None:
        def add_pattern(root: Path) -> None:
            # Keep the synthetic detection string out of this test source.
            (root / "src/key.rs").write_text("AK" + "IA" + "0" * 16, encoding="utf-8")

        failures = self.audit_private(add_pattern)
        self.assertTrue(any(finding.kind == "secret" for finding in failures))

    def test_public_tree_rejects_unallowlisted_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_release_skeleton(root)
            (root / ".gitignore").write_text("# fixture\n", encoding="utf-8")
            (root / "unrelated.txt").write_text("must not ship\n", encoding="utf-8")
            failures, _warnings = PREPARE_SOURCE.audit_public_tree(root)
            self.assertTrue(
                any(finding.kind == "allowlist" and finding.path == Path("unrelated.txt") for finding in failures)
            )


if __name__ == "__main__":
    unittest.main()
