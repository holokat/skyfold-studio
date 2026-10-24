#!/usr/bin/env python3
"""Package a paid style pack as the files of its skyfoldstudio.com listing.

    python3 tools/release/pack_listing.py skyfold_studio /path/to/new/upload-folder [part MiB]

Writes `skyfold-pack.json` and `part-01.skyfoldpart`, `part-02.skyfoldpart`… (ZIPs, named so
the website's uploader keeps them whole instead of unpacking them) into the new folder: the pack's
folders (its model library `assets/models/stylepacks/<style>` and painted surfaces
`assets/textures/<style>`) split so every part stays under the website's 32 MiB file limit and
the whole under its pack limit. Upload the folder as the
listing's files in Creator studio; the editor installs a bought copy with
`src/market/install.rs`, which expects exactly this layout.
"""

import json
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
# the Collection is the hand-painted buildings, props and scenes; the painted surfaces
# (textures/handpainted) aren't sold in it
PACKS = {"skyfold_studio": ["models/stylepacks/handpainted"]}
PART_LIMIT = 30 * 1024 * 1024  # under the website's 32 MiB per file
PACK_LIMIT = 512 * 1024 * 1024  # docs/website-list-purchases.patch raises the site's to this
MAX_ENTRIES = 64
SKIP = {".DS_Store"}
SKIP_SUFFIXES = {".blend", ".blend1", ".glb"}  # authoring files stay private


def main() -> None:
    if len(sys.argv) not in (3, 4) or sys.argv[1] not in PACKS:
        sys.exit(f"usage: pack_listing.py {{{','.join(PACKS)}}} <new output folder> [part MiB, max 30]")
    pack, out = sys.argv[1], Path(sys.argv[2]).absolute()
    # smaller parts for uploaders that send at most ~10 MB a file (9: about 40 parts)
    part_limit = min(PART_LIMIT, int(float(sys.argv[3]) * 1024 * 1024)) if len(sys.argv) == 4 else PART_LIMIT
    if out.exists():
        sys.exit(f"{out} already exists: give a new folder")
    roots = PACKS[pack]
    assets = ROOT / "assets"
    if not (assets / roots[0] / "models.json").is_file():
        sys.exit(f"{assets / roots[0]} has no models.json")
    files = [
        p for rel in roots for p in (assets / rel).rglob("*")
        if p.is_file() and not p.is_symlink() and p.name not in SKIP and p.suffix not in SKIP_SUFFIXES
    ]
    # in folder order, so each model's (or surface's) files stay together where possible
    files.sort(key=lambda p: str(p.relative_to(assets)))
    out.mkdir(parents=True)
    parts, part, size = [], None, 0
    for f in files:
        n = f.stat().st_size
        if n > part_limit:
            sys.exit(f"{f} is {n} bytes: too big for one part")
        if part is None or size + n > part_limit:
            if part:
                part.close()
            name = f"part-{len(parts) + 1:02d}.skyfoldpart"
            parts.append(out / name)
            part = zipfile.ZipFile(out / name, "w", zipfile.ZIP_DEFLATED, compresslevel=9)
            size = 0
        part.write(f, f.relative_to(assets).as_posix())
        size += n  # uncompressed: a part's ZIP is never bigger
    if part:
        part.close()
    desc = {"format": 1, "pack": pack, "roots": roots, "version": __import__("datetime").date.today().isoformat()}
    (out / "skyfold-pack.json").write_text(json.dumps(desc, indent=1) + "\n")
    sizes = [p.stat().st_size for p in parts]
    total = sum(sizes) + (out / "skyfold-pack.json").stat().st_size
    assert all(s <= 32 * 1024 * 1024 for s in sizes)
    if total > PACK_LIMIT or len(parts) + 1 > MAX_ENTRIES:
        sys.exit(f"{len(parts)} parts, {total} bytes: over the website's pack limits")
    print(json.dumps({"pack": pack, "files": len(files), "parts": len(parts), "total_mib": round(total / 1048576, 1)}))


if __name__ == "__main__":
    main()
