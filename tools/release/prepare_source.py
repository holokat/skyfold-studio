#!/usr/bin/env python3
"""Create and audit the small public source release.

This script deliberately copies from an explicit allowlist.  It never edits the
working tree and refuses an output path that already exists, is a symlink, or
would be created inside this repository.

Usage:
  python3 tools/release/prepare_source.py OUT_DIR
  python3 tools/release/prepare_source.py audit [TREE]
  python3 tools/release/prepare_source.py audit-public TREE
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


REPO = Path(__file__).resolve().parents[2]
MAX_FILE_BYTES = 10 * 1024 * 1024

# These are source inputs referenced by include_str!/include_bytes!, plus the
# public build instructions.  Runtime model, texture, audio, and Central Icon
# payloads are intentionally absent.
FILES = (
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "README.md",
    "LICENSE",
    "THIRD_PARTY_NOTICES.md",
    "docs/building.md",
    "tools/asset_rules.json",
    "tools/crops/ATLAS_SPEC.md",
    "assets/branding/skyfold_icon_128.png",
    "assets/branding/skyfold_icon_512.png",
    "assets/textures/flames/flames.json",
    "assets/textures/flames/LICENSE-AetherFX.txt",
    "assets/textures/flames/README.md",
)
TREES = (
    "src",
    "crates/ez_tree",
    "crates/particle_fluids",
    "crates/autorig",
    "assets/shaders",
    "docs/screenshots",
    "docs/readme",
    "tools/release",
)

FORBIDDEN_PARTS = {
    ".git",
    ".claude",
    ".unlazy",
    "art",
    "work",
    "target",
    "worlds",
    "exports",
    "stylepacks",
    "central",
    "__pycache__",
}
FORBIDDEN_PREFIXES = (
    "assets/models/",
    "assets/audio/",
    "assets/icons/",
    "assets/textures/handpainted/",
    "assets/textures/foliage/",
    "tools/stylepack/",
    "tools/audio/",
    "tools/flora/",
    "tools/dungeon_sheets/",
    "tools/fish_models/",
    "tools/crops/scans/",
    "tools/crops/out/",
    "docs/ui-redesign/",
)
BACKUP_SUFFIXES = (".orig", ".bak", ".backup", ".tmp", "~")
SECRET_PATTERNS = {
    "private key": re.compile(r"-----BEGIN (?:RSA|EC|OPENSSH|PRIVATE) KEY-----"),
    "AWS access key": re.compile(r"AKIA[0-9A-Z]{16}"),
    "GitHub token": re.compile(r"(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})"),
    "OpenAI-style key": re.compile(r"sk-[A-Za-z0-9_-]{20,}"),
    "Stripe secret": re.compile(r"(?:sk|rk)_(?:live|test)_[A-Za-z0-9]+"),
    "Slack token": re.compile(r"xox[baprs]-[A-Za-z0-9-]+"),
}
LEGACY_EFFECT_WORDS = ("reference_effects", "effects_studio")
LEGACY_EFFECT_MIGRATION_FILES = {Path("src/io.rs"), Path("src/io/save.rs")}
# These files describe or implement the scan itself; their explanatory literal
# strings are not application path fallbacks.
PATH_LITERAL_SCAN_EXCLUSIONS = {Path("tools/release/prepare_source.py"), Path("docs/building.md")}


@dataclass(frozen=True)
class Finding:
    kind: str
    path: Path
    detail: str


def relative(root: Path, path: Path) -> Path:
    return path.relative_to(root)


def forbidden(rel: Path) -> str | None:
    posix = rel.as_posix()
    if any(part in FORBIDDEN_PARTS for part in rel.parts):
        return "forbidden directory"
    if posix.startswith(FORBIDDEN_PREFIXES):
        return "forbidden release path"
    if rel.name.endswith(BACKUP_SUFFIXES):
        return "backup or editor residue"
    if rel.suffix.lower() in {".blend", ".blend1", ".glb"}:
        return "authoring or buyer artifact"
    return None


def excluded_from_export(rel: Path) -> bool:
    """Return whether an otherwise allowlisted file is local editor residue."""
    return rel.name.endswith(BACKUP_SUFFIXES)


def allowed_paths(root: Path) -> Iterable[Path]:
    """Yield copyable inputs from the private checkout's explicit allowlist."""
    for name in FILES:
        path = root / name
        if not path.is_file():
            raise RuntimeError(f"required release file is missing: {name}")
        rel = relative(root, path)
        if not excluded_from_export(rel):
            yield path
    for name in TREES:
        tree = root / name
        if not tree.is_dir():
            if name == "docs/screenshots":
                continue
            raise RuntimeError(f"required release directory is missing: {name}/")
        for path in tree.rglob("*"):
            if not (path.is_file() or path.is_symlink()):
                continue
            if not excluded_from_export(relative(root, path)):
                yield path


def text_for_scan(path: Path) -> str | None:
    try:
        data = path.read_bytes()
    except OSError:
        return None
    if b"\0" in data[:8192]:
        return None
    return data.decode("utf-8", errors="replace")


def all_public_paths(root: Path) -> Iterable[Path]:
    """Yield every public-tree entry except git metadata, including symlinks."""
    for path in root.rglob("*"):
        rel = relative(root, path)
        if rel.parts and rel.parts[0] == ".git":
            continue
        if path.is_file() or path.is_symlink():
            yield path


def audit_paths(root: Path, paths: Iterable[Path], *, require_allowlist: bool) -> tuple[list[Finding], list[Finding]]:
    """Audit paths without printing secret contents or literal values."""
    failures: list[Finding] = []
    warnings: list[Finding] = []
    expected: set[Path] | None = None
    if require_allowlist:
        try:
            expected = {relative(root, path) for path in allowed_paths(root)} | {Path(".gitignore")}
        except RuntimeError as error:
            return [Finding("allowlist", root, str(error))], warnings

    for path in paths:
        rel = relative(root, path)
        if expected is not None and rel not in expected:
            failures.append(Finding("allowlist", rel, "not in the public release allowlist"))
        reason = forbidden(rel)
        if reason:
            failures.append(Finding("path", rel, reason))
            continue
        if path.is_symlink():
            failures.append(Finding("symlink", rel, "release trees must not contain symlinks"))
            continue
        try:
            size = path.stat().st_size
        except OSError as error:
            failures.append(Finding("read", rel, str(error)))
            continue
        if size > MAX_FILE_BYTES:
            failures.append(Finding("size", rel, f"{size} bytes exceeds {MAX_FILE_BYTES}"))
        text = text_for_scan(path)
        if text is None:
            continue
        for label, pattern in SECRET_PATTERNS.items():
            if pattern.search(text):
                failures.append(Finding("secret", rel, label))
        # Retired Effects Studio names are source-code policy only.  This
        # avoids flagging the exporter or public documentation that describes
        # the policy itself.
        if rel.parts[:1] == ("src",) and rel.suffix == ".rs" and any(word in text for word in LEGACY_EFFECT_WORDS):
            # The migration record is intentionally confined to io.rs and its
            # save helper. Any other source reintroduces retired Effects
            # Studio behaviour.
            if rel not in LEGACY_EFFECT_MIGRATION_FILES:
                failures.append(Finding("retired effects", rel, "outside LegacyEffectsMetadata migration"))
        if rel not in PATH_LITERAL_SCAN_EXCLUSIONS and ("CARGO_MANIFEST_DIR" in text or "/Users/k/" in text):
            lines = [
                str(index)
                for index, line in enumerate(text.splitlines(), 1)
                if "CARGO_MANIFEST_DIR" in line or "/Users/k/" in line
            ]
            warnings.append(Finding("path literal", rel, "lines " + ",".join(lines)))
    return failures, warnings


def audit(root: Path) -> tuple[list[Finding], list[Finding]]:
    """Preflight only the private checkout's copyable allowlist."""
    try:
        paths = list(allowed_paths(root))
    except RuntimeError as error:
        return [Finding("allowlist", root, str(error))], []
    return audit_paths(root, paths, require_allowlist=False)


def audit_public_tree(root: Path) -> tuple[list[Finding], list[Finding]]:
    """Audit every exported file and reject anything outside the allowlist."""
    return audit_paths(root, all_public_paths(root), require_allowlist=True)


def print_findings(title: str, findings: list[Finding]) -> None:
    if not findings:
        return
    print(title, file=sys.stderr)
    for finding in findings:
        print(f"  {finding.kind}: {finding.path} ({finding.detail})", file=sys.stderr)


def ensure_output_path(output: Path) -> None:
    if os.path.lexists(output):
        raise RuntimeError(f"output already exists or is a symlink: {output}")
    parent = output.parent
    if not parent.is_dir():
        raise RuntimeError(f"output parent is not a directory: {parent}")
    probe = parent
    while probe != probe.parent:
        if probe.is_symlink():
            raise RuntimeError(f"output parent contains a symlink: {probe}")
        probe = probe.parent
    try:
        output.relative_to(REPO)
    except ValueError:
        return
    raise RuntimeError("output must be outside the source repository")


PUBLIC_GITIGNORE = """# Public source release only. Runtime asset bundles are restored locally.
/target/
/worlds/
/exports/
/work/
/art/
/.claude/
/.unlazy/
__pycache__/
*.py[cod]
*.orig
*.bak
*.backup
*~
.DS_Store

# Never add marketplace, authoring, or licensed runtime payloads here.
/assets/models/
/assets/audio/
/assets/icons/
/assets/textures/handpainted/
/assets/textures/foliage/
/assets/.thumbs/
/assets/.lods/
/assets/.texcache/
/tools/stylepack/
/tools/audio/
/tools/flora/
/tools/dungeon_sheets/
/tools/fish_models/
"""


def copy_file(source: Path, destination_root: Path) -> None:
    target = destination_root / relative(REPO, source)
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target)


def export(output: Path) -> None:
    ensure_output_path(output)
    failures, warnings = audit(REPO)
    print_findings("source audit warnings (review before release):", warnings)
    if failures:
        print_findings("source audit failed; no export was created:", failures)
        raise RuntimeError("fix the listed source audit findings before exporting")

    output.mkdir(mode=0o755)
    try:
        for source in allowed_paths(REPO):
            rel = relative(REPO, source)
            if forbidden(rel):
                continue
            copy_file(source, output)
        (output / ".gitignore").write_text(PUBLIC_GITIGNORE, encoding="utf-8")
        failures, warnings = audit_public_tree(output)
        print_findings("export audit warnings (expected source-path references):", warnings)
        if failures:
            print_findings("export audit failed:", failures)
            raise RuntimeError("export did not satisfy the public allowlist")
    except Exception:
        print(f"Incomplete export retained for inspection: {output}", file=sys.stderr)
        raise
    print(f"prepared public source release: {output}")


def main(argv: list[str]) -> int:
    if argv and argv[0] == "audit":
        parser = argparse.ArgumentParser(description="audit an allowlisted release tree")
        parser.add_argument("tree", nargs="?", type=Path, default=REPO)
        args = parser.parse_args(argv[1:])
        root = args.tree.resolve()
        failures, warnings = audit(root)
        print_findings("source audit warnings:", warnings)
        print_findings("source audit failed:", failures)
        return 1 if failures else 0
    if argv and argv[0] == "audit-public":
        parser = argparse.ArgumentParser(description="audit every file in an exported public release tree")
        parser.add_argument("tree", type=Path)
        args = parser.parse_args(argv[1:])
        root = args.tree.resolve()
        failures, warnings = audit_public_tree(root)
        print_findings("public-tree audit warnings:", warnings)
        print_findings("public-tree audit failed:", failures)
        return 1 if failures else 0
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="new output directory")
    args = parser.parse_args(argv)
    try:
        # abspath normalises a relative destination but deliberately does not
        # resolve symlinks: ensure_output_path must be able to reject them.
        export(Path(os.path.abspath(args.output.expanduser())))
    except RuntimeError as error:
        print(f"prepare_source: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
