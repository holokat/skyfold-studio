#!/usr/bin/env python3
"""Publish a release built by release_macos.py as the latest GitHub release of the public repo.

    python3 tools/release/publish_macos.py 0.1.3 --notes notes.md
    python3 tools/release/publish_macos.py --seed 0.1.6 <dmg>   # put an existing DMG on R2

Default (DMG_HOST "github"): uploads Skyfold-Studio-macOS.dmg, appcast.xml and SHA256SUMS.txt to
a new release v<version>. The website's download link and every installed app's update check
(SUFeedURL) read the latest release, so this is the step that ships the update.

With SKYFOLD_DMG_HOST=r2 (for both scripts; for DMGs over GitHub's 2 GiB asset limit):
1. uploads the DMG to the public R2 bucket (DOWNLOAD_BASE/v<version>/, kept forever) and checks
   the public URL serves exactly that file;
2. copies it to DOWNLOAD_BASE/latest/ (then point the website's and README's Download link there);
3. creates the GitHub release with appcast.xml and SHA256SUMS.txt only: the appcast's enclosure
   points at the R2 copy.
Refuses a DMG that isn't notarized.

R2 is written through its S3 API (curl's built-in SigV4; one PUT, R2 takes up to 5 GiB). The
credentials are an R2 API token with Object Read & Write on the bucket: R2_ACCESS_KEY_ID and
R2_SECRET_ACCESS_KEY in the environment, else the login keychain item "skyfold-r2-downloads"
(account = access key ID, password = secret). They go to curl on stdin, never in argv.
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import subprocess
import sys
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(Path(__file__).resolve().parent))
from release_macos import DMG_HOST, DMG_NAME, DOWNLOAD_BASE, REPO, sha256  # noqa: E402

ACCOUNT_ID = "0b55a6c39a979a58efbf40875ac376d6"
BUCKET = "skyfold-studio-downloads"
ENDPOINT = f"https://{ACCOUNT_ID}.r2.cloudflarestorage.com/{BUCKET}"
KEYCHAIN_ITEM = "skyfold-r2-downloads"


def credentials() -> tuple[str, str]:
    key, secret = os.environ.get("R2_ACCESS_KEY_ID"), os.environ.get("R2_SECRET_ACCESS_KEY")
    if key and secret:
        return key, secret
    r = subprocess.run(["security", "find-generic-password", "-s", KEYCHAIN_ITEM, "-g"],
                       capture_output=True, text=True)
    acct = next((l.split('"acct"<blob>="', 1)[1].rstrip('"') for l in r.stdout.splitlines()
                 if '"acct"<blob>="' in l), None)
    pw = subprocess.run(["security", "find-generic-password", "-s", KEYCHAIN_ITEM, "-w"],
                        capture_output=True, text=True).stdout.strip()
    if r.returncode != 0 or not acct or not pw:
        raise SystemExit(f"No R2 credentials: set R2_ACCESS_KEY_ID / R2_SECRET_ACCESS_KEY or add the "
                         f"keychain item:\n  security add-generic-password -s {KEYCHAIN_ITEM} "
                         "-a <access key id> -w")
    return acct, pw


def s3(method: str, key: str, headers: dict[str, str], upload: Path | None = None) -> None:
    """One signed request to the bucket (credentials on curl's stdin config)."""
    acct, secret = credentials()
    cmd = ["curl", "-sS", "--fail-with-body", "-K", "-", "--aws-sigv4", "aws:amz:auto:s3",
           "-X", method, "-H", "x-amz-content-sha256: UNSIGNED-PAYLOAD"]
    for k, v in headers.items():
        cmd += ["-H", f"{k}: {v}"]
    if upload is not None:
        cmd += ["-T", str(upload), "--progress-bar", "-o", "/dev/null"]
    cmd.append(f"{ENDPOINT}/{key}")
    r = subprocess.run(cmd, input=f'user = "{acct}:{secret}"\n', text=True,
                       stdout=None if upload else subprocess.PIPE)
    if r.returncode != 0:
        raise SystemExit(f"R2 {method} {key} failed (curl {r.returncode}) {r.stdout or ''}")


def public_size(url: str) -> int | None:
    req = urllib.request.Request(url, method="HEAD", headers={"Cache-Control": "no-cache"})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return int(r.headers.get("Content-Length", -1))
    except Exception as e:  # noqa: BLE001
        print(f"  {url}: {e}")
        return None


def put_dmg(version: str, dmg: Path) -> str:
    """Uploads v<version>/ and copies it to latest/; returns the versioned public URL."""
    key = f"v{version}/{DMG_NAME}"
    common = {"Content-Type": "application/x-apple-diskimage",
              "Content-Disposition": f'attachment; filename="{DMG_NAME}"'}
    size = dmg.stat().st_size
    print(f"→ Uploading {dmg.name} ({size / 1024**3:.2f} GiB) to R2 {BUCKET}/{key}", flush=True)
    s3("PUT", key, {**common, "Cache-Control": "public, max-age=31536000, immutable",
                    "x-amz-meta-sha256": sha256(dmg)}, upload=dmg)
    url = f"{DOWNLOAD_BASE}/{key}"
    got = public_size(url)
    if got != size:
        raise SystemExit(f"{url} serves {got} bytes, expected {size}: is the bucket's public "
                         "domain connected? Nothing was published to GitHub.")
    print(f"→ Copying to {BUCKET}/latest/{DMG_NAME}", flush=True)
    s3("PUT", f"latest/{DMG_NAME}", {**common, "Cache-Control": "public, max-age=300",
                                      "x-amz-copy-source": f"/{BUCKET}/{key}",
                                      "x-amz-metadata-directive": "REPLACE"})
    latest = f"{DOWNLOAD_BASE}/latest/{DMG_NAME}"
    if public_size(latest) != size:
        raise SystemExit(f"{latest} doesn't serve the new DMG yet")
    return url


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("version")
    ap.add_argument("dmg", nargs="?", type=Path, help="with --seed: the DMG to upload")
    ap.add_argument("--notes", type=Path, help="Markdown body of the release")
    ap.add_argument("--seed", action="store_true",
                    help="only put an already-published DMG on R2 (v<version>/ and latest/)")
    args = ap.parse_args()
    if args.seed:
        if not args.dmg or not args.dmg.is_file():
            raise SystemExit("--seed needs the DMG path")
        print(f"✅ {put_dmg(args.version, args.dmg)}")
        return
    if not args.notes:
        raise SystemExit("--notes is required")
    out = ROOT / "target/release-mac" / args.version
    dmg, appcast, sums = out / DMG_NAME, out / "appcast.xml", out / "SHA256SUMS.txt"
    for f in (dmg, appcast, sums):
        if not f.is_file():
            raise SystemExit(f"Missing {f}: run release_macos.py {args.version} first")
    if subprocess.run(["xcrun", "stapler", "validate", str(dmg)],
                      capture_output=True).returncode != 0:
        raise SystemExit(f"{DMG_NAME} isn't notarized and stapled")
    feed = appcast.read_text()
    if f"<sparkle:version>{args.version}</sparkle:version>" not in feed:
        raise SystemExit("appcast.xml is for another version")
    tag = f"v{args.version}"
    url = (f"{DOWNLOAD_BASE}/{tag}/{DMG_NAME}" if DMG_HOST == "r2"
           else f"https://github.com/{REPO}/releases/download/{tag}/{DMG_NAME}")
    if url not in feed:
        raise SystemExit(f"appcast.xml doesn't point at {url}: build and publish with the same "
                         "SKYFOLD_DMG_HOST")
    if subprocess.run(["gh", "release", "view", tag, "-R", REPO],
                      capture_output=True).returncode == 0:
        raise SystemExit(f"Release {tag} already exists in {REPO}")
    notes, assets = args.notes, [dmg, appcast, sums]
    if DMG_HOST == "r2":
        put_dmg(args.version, dmg)
        notes = out / "release-notes.md"
        notes.write_text(args.notes.read_text().rstrip() + f"\n\n**Download:** [{DMG_NAME}]({url})\n")
        assets = [appcast, sums]
    subprocess.run(["gh", "release", "create", tag, "-R", REPO, *map(str, assets),
                    "--title", f"Skyfold Studio {args.version} for macOS",
                    "--notes-file", str(notes), "--latest"], check=True)
    print(f"✅ Published {tag}: running apps see it at their next check (daily, or "
          f"File › Check for Updates…); download {url}")


if __name__ == "__main__":
    main()
