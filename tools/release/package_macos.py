#!/usr/bin/env python3
"""Stage a macOS app from an explicit runtime-asset allowlist. Never publishes."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]
RUNTIME_DIRS = (
    "branding", "dungeon_sheets", "fonts", "market", "onboarding", "shaders",
    "textures", "models/crops", "models/flora", "models/food",
    "models/photoscan", "models/polyhaven", "models/skyborn",
)
EXTENSIONS = {".png", ".jpg", ".jpeg", ".ktx2", ".json", ".md", ".txt",
              ".ttf", ".otf", ".icns", ".gltf", ".glb", ".bin", ".wesl", ".wgsl", ".mp3"}
PRIVATE_PREFIXES = ("models/stylepacks/", "textures/handpainted/", "icons/", "imported/")


def runtime_files(root: Path, include_owner_media: bool) -> list[Path]:
    dirs = RUNTIME_DIRS + (("audio", "models/fish") if include_owner_media else ())
    files = []
    for name in dirs:
        directory = root / "assets" / name
        if not directory.is_dir():
            raise RuntimeError(f"Missing runtime resource directory: assets/{name}")
        for file in sorted(directory.rglob("*")):
            if file.relative_to(root / "assets").as_posix().startswith(PRIVATE_PREFIXES):
                continue
            if file.is_symlink():
                raise RuntimeError(f"Resource symlink is not allowed: {file.relative_to(root)}")
            if file.is_file() and file.suffix.lower() in EXTENSIONS and file.name != "zone_enter_0.mp3":
                if any(part.startswith(".") or part in {"src", "source", "sources"}
                       for part in file.relative_to(directory).parts):
                    continue
                files.append(file)
    return files


def clone_copy(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    if os.uname().sysname == "Darwin":
        subprocess.run(["/bin/cp", "-c", str(source), str(destination)], check=True)
    else:
        shutil.copy2(source, destination)


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        result = hashlib.sha256()
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(block)
        return result.hexdigest()


def audit_resources(assets: Path, expected: dict[str, dict]) -> None:
    actual = {p.relative_to(assets).as_posix() for p in assets.rglob("*") if p.is_file()}
    if actual != set(expected):
        raise RuntimeError("Staged resource set differs from the approved manifest")
    for relative, entry in expected.items():
        if relative.startswith(PRIVATE_PREFIXES):
            raise RuntimeError(f"Private resource found in app: {relative}")
        if digest(assets / relative) != entry["sha256"]:
            raise RuntimeError(f"Resource checksum differs: {relative}")


def dependency_notices(metadata_file: Path, destination: Path) -> None:
    metadata = json.loads(metadata_file.read_text())
    lines = ["Skyfold Studio dependency notices", ""]
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        lines.append(f'{package["name"]} {package["version"]}: {package.get("license") or "See package license"}')
        directory = Path(package["manifest_path"]).parent
        candidates = set()
        for pattern in ("LICENSE*", "LICENCE*", "COPYING*", "NOTICE*", "UNLICENSE*"):
            candidates.update(p for p in directory.glob(pattern) if p.is_file())
        if package.get("license_file"):
            candidates.add(directory / package["license_file"])
        for file in sorted(candidates):
            if file.is_file() and not file.is_symlink():
                clone_copy(file, destination / f'{package["name"]}-{package["version"]}' / file.name)
    destination.mkdir(parents=True, exist_ok=True)
    (destination / "DEPENDENCIES.txt").write_text("\n".join(lines) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/world_builder")
    parser.add_argument("--version", default="0.1.0")
    parser.add_argument("--cargo-metadata", type=Path)
    parser.add_argument("--include-owner-media", action="store_true",
                        help="Include fish/audio only after their owner confirms redistribution rights")
    parser.add_argument("--inspect", action="store_true")
    args = parser.parse_args()
    files = runtime_files(ROOT, args.include_owner_media)
    total = sum(p.stat().st_size for p in files)
    print(json.dumps({"resource_files": len(files), "resource_bytes": total,
                      "paid_pack_files": 0, "raw_icon_files": 0}))
    if args.inspect:
        return
    if not args.output or not args.cargo_metadata:
        parser.error("--output and --cargo-metadata are required for staging")
    output = args.output.absolute()
    if output.exists() or output.is_symlink():
        raise RuntimeError("Output must be a new directory")
    if not output.parent.is_dir() or output.parent.is_symlink():
        raise RuntimeError("Output parent must be an existing real directory")
    app = output / "Skyfold Studio.app"
    contents = app / "Contents"
    resources = contents / "Resources"
    executable = contents / "MacOS/Skyfold Studio"
    clone_copy(args.binary, executable)
    executable.chmod(0o755)
    manifest = {}
    for source in files:
        relative = source.relative_to(ROOT / "assets").as_posix()
        destination = resources / "assets" / relative
        clone_copy(source, destination)
        manifest[relative] = {"bytes": source.stat().st_size, "sha256": digest(source)}
    clone_copy(ROOT / "assets/branding/Skyfold.icns", resources / "Skyfold.icns")
    for name in ("LICENSE", "THIRD_PARTY_NOTICES.md"):
        clone_copy(ROOT / name, resources / name)
    dependency_notices(args.cargo_metadata, resources / "licenses")
    info = {
        "CFBundleDisplayName": "Skyfold Studio", "CFBundleName": "Skyfold Studio",
        "CFBundleExecutable": "Skyfold Studio", "CFBundleIdentifier": "com.skyfoldstudio.editor",
        "CFBundlePackageType": "APPL", "CFBundleIconFile": "Skyfold.icns",
        "CFBundleShortVersionString": args.version, "CFBundleVersion": args.version,
        "LSMinimumSystemVersion": "11.0", "NSHighResolutionCapable": True,
        "LSApplicationCategoryType": "public.app-category.graphics-design",
    }
    (contents / "Info.plist").write_bytes(plistlib.dumps(info))
    audit_resources(resources / "assets", manifest)
    (resources / "runtime-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (output / "Applications").symlink_to("/Applications")
    (output / "Install.txt").write_text(
        "Skyfold Studio\n\nDrag Skyfold Studio into Applications, then open it.\n"
        "This build is for Apple silicon Macs.\n\n"
        "Worlds and settings are stored in ~/Library/Application Support/Skyfold Studio.\n"
        "Paid asset packs are downloaded separately through the marketplace.\n"
        "https://skyfoldstudio.com\n")
    print(f"Staged and audited {app}")


if __name__ == "__main__":
    main()
