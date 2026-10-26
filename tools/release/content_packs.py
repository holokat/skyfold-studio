#!/usr/bin/env python3
"""Free content packs that download the first time they're used (`src/content.rs`), so the app
download stays small.

    python3 tools/release/content_packs.py build OUT   # the ZIPs into OUT + assets/content/packs.json
    python3 tools/release/content_packs.py upload OUT  # attach OUT's ZIPs to the GitHub release
                                                       # `content-packs`
    python3 tools/release/content_packs.py list        # the packs, their folders and sizes

Packs:
- city_surfaces: the City & sci-fi surfaces (the terrain library's `city` grounds and the object
  materials `tools/build_material_library.py` builds). Each material's swatch.png stays in the
  app (`keep`), so pickers show it before the download.
- handpainted_surfaces: textures/handpainted, the painted twin of every texture (`art_style`).

`build` writes each pack as one ZIP named after its content (`<id>-<sha256[:12]>.zip`, so a
changed pack is a new file and old apps keep finding theirs) laid out like `assets/`, and
records its file, size and SHA-256 in assets/content/packs.json, which ships with the app.

Hosting: assets of the GitHub release `content-packs` (never marked Latest: every installed
app's update check reads the Latest release's appcast.xml). Each ZIP is under GitHub's 2 GiB
asset limit.
`package_macos.py` leaves every pack's files (except `keep`) out of the app.

Release order: `build` (commit packs.json), `upload`, then package the app.
"""
from __future__ import annotations

import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[2]
ASSETS = ROOT / "assets"
MANIFEST = ASSETS / "content/packs.json"
sys.path.insert(0, str(Path(__file__).resolve().parent))
from release_macos import REPO  # noqa: E402

RELEASE_TAG = "content-packs"

STORED = {".png", ".jpg", ".jpeg", ".ktx2", ".mp3", ".webp"}
FIXED_TIME = (2026, 1, 1, 0, 0, 0)


def city_folders() -> list[str]:
    lib = json.loads((ASSETS / "textures/terrain_library/library.json").read_text())
    grounds = [f"textures/terrain_library/{m['id']}" for m in lib["materials"]
               if m.get("category") == "city"]
    # the object materials the city & sci-fi builder makes (its MATERIALS dict's keys)
    src = (ROOT / "tools/build_material_library.py").read_text()
    ids: list[str] = []
    for node in ast.parse(src).body:
        if isinstance(node, ast.Assign) and any(getattr(t, "id", "") == "MATERIALS"
                                                for t in node.targets):
            ids = [k.value for k in node.value.keys if isinstance(k, ast.Constant)]
    if not ids:
        raise SystemExit("no MATERIALS in tools/build_material_library.py")
    mats = [f"textures/materials/{i}" for i in ids]
    return sorted(grounds) + sorted(mats)


def packs() -> list[dict]:
    return [
        {"id": "city_surfaces", "name": "City & sci-fi surfaces",
         "blurb": "Asphalt, pavers, terrazzo, deck plating, chrome, concrete, tech panels and more.",
         "folders": city_folders(), "keep": ["swatch.png"]},
        {"id": "handpainted_surfaces", "name": "Hand-painted surfaces",
         "blurb": "The painted ground, rock, bark, leaves and materials of a Hand-painted world.",
         "folders": ["textures/handpainted"], "keep": []},
    ]


def files_of(pack: dict) -> list[Path]:
    out = []
    for folder in pack["folders"]:
        d = ASSETS / folder
        if not d.is_dir():
            raise SystemExit(f"{pack['id']}: missing assets/{folder}")
        for f in sorted(d.rglob("*")):
            if f.is_file() and f.name not in pack["keep"] and not f.name.startswith("."):
                out.append(f)
    return out


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def build(out: Path) -> None:
    out.mkdir(parents=True, exist_ok=True)
    manifest = {"format": 1,
                "base_url": f"https://github.com/{REPO}/releases/download/{RELEASE_TAG}",
                "packs": []}
    for p in packs():
        files = files_of(p)
        tmp = out / f"{p['id']}.zip.part"
        with zipfile.ZipFile(tmp, "w") as z:
            for f in files:
                info = zipfile.ZipInfo(f.relative_to(ASSETS).as_posix(), FIXED_TIME)
                info.external_attr = 0o644 << 16
                info.compress_type = (zipfile.ZIP_STORED if f.suffix.lower() in STORED
                                      else zipfile.ZIP_DEFLATED)
                z.writestr(info, f.read_bytes())
        digest = sha256(tmp)
        name = f"{p['id']}-{digest[:12]}.zip"
        final = out / name
        tmp.replace(final)
        size = final.stat().st_size
        print(f"{name}: {len(files)} files, {size / 1e6:.0f} MB")
        manifest["packs"].append({**p, "version": digest[:12], "file": name, "bytes": size,
                                  "sha256": digest, "files": len(files),
                                  "unpacked_bytes": sum(f.stat().st_size for f in files)})
    MANIFEST.parent.mkdir(parents=True, exist_ok=True)
    MANIFEST.write_text(json.dumps(manifest, indent=1) + "\n")
    print(f"wrote {MANIFEST.relative_to(ROOT)}")


def upload(out: Path) -> None:
    manifest = json.loads(MANIFEST.read_text())
    files = []
    for p in manifest["packs"]:
        f = out / p["file"]
        if not f.is_file() or sha256(f) != p["sha256"]:
            raise SystemExit(f"{f} is missing or isn't the one in packs.json: run build first")
        if p["bytes"] >= 2 << 30:
            raise SystemExit(f"{p['file']} is over GitHub's 2 GiB asset limit")
        files.append(str(f))
    gh = ["gh", "--repo", REPO]
    if subprocess.run(gh + ["release", "view", RELEASE_TAG], capture_output=True).returncode:
        subprocess.run(gh + ["release", "create", RELEASE_TAG, "--latest=false",
                             "--title", "Content packs",
                             "--notes", "Free content Skyfold Studio downloads the first time it "
                                        "is used (City & sci-fi surfaces, Hand-painted "
                                        "surfaces). Not an app release."], check=True)
    print(f"→ {len(files)} ZIPs to the {RELEASE_TAG} release", flush=True)
    subprocess.run(gh + ["release", "upload", RELEASE_TAG, "--clobber", *files], check=True)


def main() -> None:
    args = sys.argv[1:]
    if args[:1] == ["build"] and len(args) == 2:
        build(Path(args[1]).resolve())
    elif args[:1] == ["upload"] and len(args) == 2:
        upload(Path(args[1]).resolve())
    elif args == ["list"]:
        for p in packs():
            files = files_of(p)
            print(f"{p['id']}: {len(p['folders'])} folders, {len(files)} files, "
                  f"{sum(f.stat().st_size for f in files) / 1e6:.0f} MB")
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
