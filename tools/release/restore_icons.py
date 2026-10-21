#!/usr/bin/env python3
"""Copy the UI's named Central SVGs from your separately licensed export."""
from pathlib import Path
import argparse
import re
import shutil

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("licensed_exports", type=Path)
    args = parser.parse_args()
    exports = args.licensed_exports.resolve()
    names = sorted(set(re.findall(r"assets/icons/central/([^\"/]+\.svg)",
                                 (ROOT / "src/ui/icons/svgs.rs").read_text())))
    sources = {path.name: path for path in exports.rglob("*.svg") if not path.is_symlink()}
    missing = [name for name in names if name not in sources]
    if missing:
        parser.error("Missing licensed SVG exports: " + ", ".join(missing))
    destination = ROOT / "assets/icons/central"
    destination.mkdir(parents=True, exist_ok=True)
    for name in names:
        target = destination / name
        if target.is_symlink():
            parser.error(f"Refusing symlink: {target}")
        if target.exists() and target.read_bytes() != sources[name].read_bytes():
            parser.error(f"Existing icon differs; choose which version to keep: {target}")
    for name in names:
        if not (destination / name).exists():
            shutil.copy2(sources[name], destination / name)
    print(f"Restored {len(names)} licensed interface icons. Do not publish assets/icons/central.")


if __name__ == "__main__":
    main()
