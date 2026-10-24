#!/usr/bin/env python3
"""Stage a macOS app from an explicit runtime-asset allowlist. Never publishes."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import struct
import tempfile

ROOT = Path(__file__).resolve().parents[2]
# (audio is the open sound library: every file CC0 or synthesised, see assets/audio/CREDITS.md)
RUNTIME_DIRS = (
    "audio", "branding", "dungeon_sheets", "fonts", "market", "onboarding", "shaders",
    "textures", "models/crops", "models/flora", "models/food",
    "models/photoscan", "models/polyhaven", "models/skyborn",
)
EXTENSIONS = {".png", ".jpg", ".jpeg", ".ktx2", ".json", ".md", ".txt",
              ".ttf", ".otf", ".icns", ".gltf", ".glb", ".bin", ".wesl", ".wgsl", ".mp3"}
# (the painted surfaces, textures/handpainted, are part of the free studio and ship with it)
PRIVATE_PREFIXES = ("models/stylepacks/", "icons/", "imported/")
# A paid style pack the app doesn't ship is previewed instead (src/objects.rs `PREVIEWS`):
# models/stylepack_previews/<style>/ holds its whole models.json, the thumbnail studio's picture
# of every model (thumbs/<id>.png) and the files of its catalog hero and free samples only.
PREVIEWS = "models/stylepack_previews"
# src/thumbs.rs / src/thumbs/cache.rs: an asset's thumbnail size and the studio's VERSION
THUMB_SIZE = 192
THUMB_VERSION = 1


def runtime_files(root: Path, include_owner_media: bool) -> list[Path]:
    dirs = RUNTIME_DIRS + (("models/fish",) if include_owner_media else ())
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


def pack_prefix(style: str) -> str:
    """src/objects.rs `pack_prefix`."""
    return "hp" if style == "handpainted" else style


def pack_id(style: str, manifest: dict) -> str:
    """src/objects.rs: a manifest's marketplace pack id (`default_pack_id` without one)."""
    if manifest.get("style") and manifest.get("pack"):
        return manifest["pack"]
    return {"handpainted": "skyfold_studio"}.get(style, style)


def fnv(parts: list[bytes]) -> int:
    """src/thumbs/cache.rs `hash` (FNV-1a, 0xff after each part)."""
    h = 0xCBF29CE484222325
    for part in parts:
        for b in part:
            h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
        h = ((h ^ 0xFF) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def thumb_name(asset: str, model_file: Path) -> tuple[str, str]:
    """The cached thumbnail of a glTF library asset (src/thumbs/cache.rs `stamp` and `path`):
    (its file name, the prefix every version of it shares)."""
    stat = model_file.stat()
    key = f"asset:{asset}"
    stamp = fnv([key.encode(), struct.pack("<QQ", stat.st_size, int(stat.st_mtime)),
                 struct.pack("<Q", THUMB_VERSION), struct.pack("<I", THUMB_SIZE), struct.pack("<I", THUMB_SIZE)])
    prefix = re.sub(r"[^A-Za-z0-9_.]", "-", key) + "-"
    return f"{prefix}{stamp:016x}.png", prefix


def model_files(gltf: Path, library: Path) -> list[Path]:
    """A model's files: the glTF and every buffer and image it references (inside its library)."""
    files = [gltf]
    if gltf.suffix.lower() == ".gltf":
        doc = json.loads(gltf.read_text())
        for item in doc.get("buffers", []) + doc.get("images", []):
            uri = item.get("uri")
            if not uri or uri.startswith("data:"):
                continue
            path = (gltf.parent / uri).resolve()
            if library.resolve() not in path.parents or not path.is_file():
                raise RuntimeError(f"{gltf.relative_to(ROOT)}: `{uri}` isn't a file in its library")
            files.append(path)
    return files


def preview_sets(root: Path, work: Path, thumbs: Path) -> tuple[list[tuple[Path, str]], set[str]]:
    """The previews of the paid style packs the app doesn't ship: (source, assets-relative
    destination) pairs (models.json is written to `work`), and the allowed model files."""
    catalog = {p["id"]: p for p in json.loads((root / "assets/market/catalog.json").read_text())["packs"]}
    out: list[tuple[Path, str]] = []
    allowed: set[str] = set()
    styles = root / "assets/models/stylepacks"
    for library in sorted(p for p in styles.iterdir() if (p / "models.json").is_file()):
        style = library.name
        manifest = json.loads((library / "models.json").read_text())
        pack = catalog.get(pack_id(style, manifest))
        if pack is None or pack.get("price") is None:
            print(f"(no preview for {style}: not a paid pack in the catalog)")
            continue
        prefix = pack_prefix(style)
        dest = f"{PREVIEWS}/{style}"
        by_id = {m["id"]: m for m in manifest["models"]}
        samples = [a.split(":", 1)[1] for a in [pack.get("hero_asset")] + pack.get("free_assets", [])
                   if a and a.startswith(prefix + ":")]
        missing = [m for m in samples if m not in by_id]
        if missing:
            raise RuntimeError(f"{style}: catalog samples missing from its models.json: {missing}")
        # the samples' files (and the textures they share), at the same relative paths
        files: dict[str, Path] = {}
        for model in dict.fromkeys(samples):
            for f in model_files(library / by_id[model]["gltf"], library):
                rel = f.relative_to(library.resolve()).as_posix()
                files[rel] = f
                allowed.add(f"{dest}/{rel}")
        # a picture of every model: the thumbnail cache's current one
        stale, absent = [], []
        for model in manifest["models"]:
            asset = f"{prefix}:{model['id']}"
            name, same = thumb_name(asset, library / model["gltf"])
            picture = thumbs / name
            if not picture.is_file():
                older = sorted((p for p in thumbs.glob(same + "*.png")
                                if re.fullmatch(re.escape(same) + r"[0-9a-f]{16}\.png", p.name)),
                               key=lambda p: p.stat().st_mtime)
                if not older:
                    absent.append(asset)
                    continue
                stale.append(asset)
                picture = older[-1]
            files[f"thumbs/{model['id']}.png"] = picture
            allowed.add(f"{dest}/thumbs/{model['id']}.png")
        if absent:
            raise RuntimeError(
                f"{len(absent)} {style} models have no thumbnail in {thumbs} (e.g. {absent[:5]}): "
                "show them in a developer build's Objects tab (or objects_preview over MCP) first")
        if stale:
            print(f"warning: {len(stale)} {style} thumbnails are older than their model (e.g. {stale[:5]})")
        if (library / "CREDITS.md").is_file():
            files["CREDITS.md"] = library / "CREDITS.md"
            allowed.add(f"{dest}/CREDITS.md")
        doc = dict(manifest)
        doc.update({"style": style, "pack": pack["id"], "preview": True,
                    "samples": list(dict.fromkeys(samples))})
        written = work / dest / "models.json"
        written.parent.mkdir(parents=True, exist_ok=True)
        written.write_text(json.dumps(doc, indent=1, sort_keys=True) + "\n")
        out.append((written, f"{dest}/models.json"))
        allowed.add(f"{dest}/models.json")
        out.extend((src, f"{dest}/{rel}") for rel, src in sorted(files.items()))
    return out, allowed


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


def audit_resources(assets: Path, expected: dict[str, dict], previews: set[str]) -> None:
    actual = {p.relative_to(assets).as_posix() for p in assets.rglob("*") if p.is_file()}
    if actual != set(expected):
        raise RuntimeError("Staged resource set differs from the approved manifest")
    for relative, entry in expected.items():
        if relative.startswith(PRIVATE_PREFIXES):
            raise RuntimeError(f"Private resource found in app: {relative}")
        # a paid pack's preview: its list, pictures and free samples only
        if relative.startswith(PREVIEWS + "/") and relative not in previews:
            raise RuntimeError(f"Paid pack file found in its preview: {relative}")
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
                        help="Include the fish models only after their owner confirms redistribution rights")
    parser.add_argument("--inspect", action="store_true")
    parser.add_argument("--thumbs", type=Path, default=ROOT / "cache/thumbs",
                        help="The thumbnail cache the paid packs' preview pictures come from")
    parser.add_argument("--previews-only", type=Path, metavar="DIR",
                        help="Only write the paid packs' preview set into DIR (to look at or measure)")
    parser.add_argument("--sparkle-framework", type=Path,
                        help="Embed this Sparkle.framework (auto-updates; release_macos.py passes it)")
    parser.add_argument("--feed-url", help="Sparkle appcast URL (SUFeedURL)")
    parser.add_argument("--ed-public-key", help="Sparkle EdDSA public key (SUPublicEDKey)")
    args = parser.parse_args()
    sparkle = (args.sparkle_framework, args.feed_url, args.ed_public_key)
    if any(sparkle) and not all(sparkle):
        parser.error("--sparkle-framework, --feed-url and --ed-public-key go together")
    work = tempfile.TemporaryDirectory(prefix="skyfold-previews-")
    previews, allowed = preview_sets(ROOT, Path(work.name), args.thumbs)
    if args.previews_only:
        for source, relative in previews:
            clone_copy(source, args.previews_only / relative)
        print(json.dumps({"preview_files": len(previews),
                          "preview_bytes": sum(p.stat().st_size for p, _ in previews),
                          "written_to": str(args.previews_only)}))
        return
    files = [(p, p.relative_to(ROOT / "assets").as_posix())
             for p in runtime_files(ROOT, args.include_owner_media)] + previews
    total = sum(p.stat().st_size for p, _ in files)
    print(json.dumps({"resource_files": len(files), "resource_bytes": total,
                      "preview_files": len(previews),
                      "preview_bytes": sum(p.stat().st_size for p, _ in previews),
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
    if b"SKYFOLD_STUDIO_DEV_BUILD" in args.binary.read_bytes():
        raise RuntimeError("That binary is a studio_dev build (developer unlock): ship target/release")
    clone_copy(args.binary, executable)
    executable.chmod(0o755)
    manifest = {}
    for source, relative in files:
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
    if args.sparkle_framework:
        # ditto keeps the framework's Versions/Current symlinks
        frameworks = contents / "Frameworks"
        frameworks.mkdir(parents=True)
        subprocess.run(["/usr/bin/ditto", str(args.sparkle_framework),
                        str(frameworks / "Sparkle.framework")], check=True)
        info.update({
            "SUFeedURL": args.feed_url, "SUPublicEDKey": args.ed_public_key,
            "SUEnableAutomaticChecks": True, "SUScheduledCheckInterval": 86400,
            "SUAllowsAutomaticUpdates": True,
        })
    (contents / "Info.plist").write_bytes(plistlib.dumps(info))
    audit_resources(resources / "assets", manifest, allowed)
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
