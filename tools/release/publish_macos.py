#!/usr/bin/env python3
"""Publish a release built by release_macos.py as the latest GitHub release of the public repo.

    python3 tools/release/publish_macos.py 0.1.3 --notes notes.md

Uploads Skyfold-Studio-macOS.dmg, appcast.xml and SHA256SUMS.txt to a new release v<version>.
The website's download link and every installed app's update check (SUFeedURL) read the latest
release, so this is the step that ships the update. Refuses a DMG that isn't notarized.
"""
from __future__ import annotations

import argparse
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
REPO = "holokat/skyfold-studio"
DMG_NAME = "Skyfold-Studio-macOS.dmg"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("version")
    ap.add_argument("--notes", type=Path, required=True, help="Markdown body of the release")
    args = ap.parse_args()
    out = ROOT / "target/release-mac" / args.version
    files = [out / DMG_NAME, out / "appcast.xml", out / "SHA256SUMS.txt"]
    for f in files:
        if not f.is_file():
            raise SystemExit(f"Missing {f}: run release_macos.py {args.version} first")
    if subprocess.run(["xcrun", "stapler", "validate", str(files[0])],
                      capture_output=True).returncode != 0:
        raise SystemExit(f"{DMG_NAME} isn't notarized and stapled")
    if f"<sparkle:version>{args.version}</sparkle:version>" not in files[1].read_text():
        raise SystemExit("appcast.xml is for another version")
    tag = f"v{args.version}"
    if subprocess.run(["gh", "release", "view", tag, "-R", REPO],
                      capture_output=True).returncode == 0:
        raise SystemExit(f"Release {tag} already exists in {REPO}")
    subprocess.run(["gh", "release", "create", tag, "-R", REPO, *map(str, files),
                    "--title", f"Skyfold Studio {args.version} for macOS",
                    "--notes-file", str(args.notes), "--latest"], check=True)
    print(f"✅ Published {tag}: running apps see it at their next check (daily, or "
          "File › Check for Updates…)")


if __name__ == "__main__":
    main()
