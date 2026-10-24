#!/usr/bin/env python3
"""Build a signed, notarized Skyfold Studio release for macOS with a Sparkle update feed.

    python3 tools/release/release_macos.py 0.1.3 --notes notes.md
    python3 tools/release/publish_macos.py 0.1.3 --notes notes.md   # then publish it

Steps: release build -> stage the app (package_macos.py) with Sparkle.framework and the feed
settings -> Developer ID signing, inside-out, hardened runtime -> notarize + staple the app ->
DMG -> sign, notarize + staple the DMG -> appcast.xml (EdDSA-signed by Sparkle's
generate_appcast with the key in the login keychain; the DMG itself is the update) ->
SHA256SUMS.txt. Everything lands in target/release-mac/<version>/; nothing is uploaded.

The feed (SUFeedURL) is the latest GitHub release's appcast.xml, so publishing a release is
what makes running apps see it. It lists only the new version: the apps before Sparkle can't
read it and every later one updates straight to the newest.

One-time setup (already done on the studio Mac): a Developer ID Application certificate, a
notarytool keychain profile (SKYFOLD_NOTARY_PROFILE, default moonshot-notary: same team) and
Sparkle's EdDSA key in the keychain (generate_keys; SKYFOLD_SPARKLE_ACCOUNT, default ed25519).
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
REPO = "holokat/skyfold-studio"
FEED_URL = f"https://github.com/{REPO}/releases/latest/download/appcast.xml"
DMG_NAME = "Skyfold-Studio-macOS.dmg"
APP_NAME = "Skyfold Studio.app"
SPARKLE_VERSION = "2.9.3"
SPARKLE_URL = (f"https://github.com/sparkle-project/Sparkle/releases/download/"
               f"{SPARKLE_VERSION}/Sparkle-{SPARKLE_VERSION}.tar.xz")
SPARKLE_SHA256 = "74a07da821f92b79310009954c0e15f350173374a3abe39095b4fc5096916be6"
CACHE = Path.home() / "Library/Caches/skyfold-release"
# GitHub refuses release assets of 2 GiB or more
ASSET_LIMIT = 2 * 1024**3


def run(*cmd, capture=False, **kw) -> str:
    print("  $", " ".join(str(c) for c in cmd), flush=True)
    r = subprocess.run([str(c) for c in cmd], check=True, text=True,
                       stdout=subprocess.PIPE if capture else None, **kw)
    return r.stdout if capture else ""


def step(text: str) -> None:
    print(f"\n→ {text}", flush=True)


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def sparkle() -> Path:
    """Sparkle's release (framework + tools), downloaded once and checked against the pinned hash."""
    home = CACHE / f"Sparkle-{SPARKLE_VERSION}"
    if (home / "Sparkle.framework").is_dir() and (home / "bin/generate_appcast").is_file():
        return home
    CACHE.mkdir(parents=True, exist_ok=True)
    archive = CACHE / f"Sparkle-{SPARKLE_VERSION}.tar.xz"
    if not archive.is_file():
        print(f"  downloading {SPARKLE_URL}")
        urllib.request.urlretrieve(SPARKLE_URL, archive)
    if sha256(archive) != SPARKLE_SHA256:
        archive.unlink()
        raise SystemExit(f"Sparkle {SPARKLE_VERSION} download doesn't match the pinned SHA-256")
    home.mkdir(exist_ok=True)
    run("tar", "-xJf", archive, "-C", home)
    return home


def identity() -> str:
    """The Developer ID Application certificate (SHA-1) that's valid the longest: the keychain
    can hold several with the same name, which makes signing by name ambiguous."""
    if os.environ.get("SKYFOLD_CODESIGN_IDENTITY"):
        return os.environ["SKYFOLD_CODESIGN_IDENTITY"]
    out = run("security", "find-identity", "-v", "-p", "codesigning", capture=True)
    hashes = re.findall(r"\) ([0-9A-F]{40}) \"Developer ID Application", out)
    if not hashes:
        raise SystemExit("No valid 'Developer ID Application' certificate in the keychain")
    pems = run("security", "find-certificate", "-a", "-Z", "-p", "-c", "Developer ID Application",
               capture=True)
    best, best_end = hashes[0], ""
    for block in re.findall(r"SHA-1 hash: ([0-9A-F]{40})\s+(-----BEGIN CERTIFICATE-----.*?"
                            r"-----END CERTIFICATE-----)", pems, re.S):
        sha, pem = block
        if sha not in hashes:
            continue
        end = subprocess.run(["openssl", "x509", "-noout", "-enddate"], input=pem, text=True,
                             capture_output=True).stdout
        # notAfter=Sep 17 00:00:00 2031 GMT -> compare as (year, month, day)
        m = re.search(r"notAfter=(\w+)\s+(\d+) [\d:]+ (\d+)", end)
        if m:
            months = "Jan Feb Mar Apr May Jun Jul Aug Sep Oct Nov Dec".split()
            key = f"{m[3]}{months.index(m[1]) + 1:02d}{int(m[2]):02d}"
            if key > best_end:
                best, best_end = sha, key
    return best


def is_macho(path: Path) -> bool:
    try:
        with path.open("rb") as f:
            magic = f.read(4)
    except OSError:
        return False
    return magic in (b"\xcf\xfa\xed\xfe", b"\xce\xfa\xed\xfe", b"\xca\xfe\xba\xbe",
                     b"\xfe\xed\xfa\xcf", b"\xfe\xed\xfa\xce", b"\xbe\xba\xfe\xca")


def sign_app(app: Path, ident: str) -> None:
    """Inside-out: Sparkle's helpers, then the framework, any other code, the app last."""
    sign = ["/usr/bin/codesign", "--force", "--options", "runtime", "--timestamp",
            "--sign", ident]
    fw = app / "Contents/Frameworks/Sparkle.framework"
    v = fw / "Versions/B"
    for part in ("XPCServices/Downloader.xpc", "XPCServices/Installer.xpc", "Updater.app",
                 "Autoupdate"):
        if (v / part).exists():
            run(*sign, v / part)
    run(*sign, fw)
    main = app / "Contents/MacOS"
    for f in sorted((app / "Contents").rglob("*")):
        if (f.is_file() and not f.is_symlink() and fw not in f.parents
                and main not in f.parents and is_macho(f)):
            run(*sign, f)
    run(*sign, app)
    run("/usr/bin/codesign", "--verify", "--deep", "--strict", "--verbose=2", app)


def notarize(path: Path, profile: str) -> None:
    out = run("xcrun", "notarytool", "submit", path, "--keychain-profile", profile, "--wait",
              "--output-format", "json", capture=True)
    result = json.loads(out)
    print(f"  notarization {result.get('id')}: {result.get('status')}")
    if result.get("status") != "Accepted":
        run("xcrun", "notarytool", "log", result.get("id", ""), "--keychain-profile", profile)
        raise SystemExit(f"Notarization of {path.name} was not accepted")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("version", help="e.g. 0.1.3 (higher than the last release)")
    ap.add_argument("--notes", type=Path, required=True,
                    help="Markdown release notes: Sparkle's update window and the GitHub release")
    ap.add_argument("--include-owner-media", action="store_true",
                    help="Passed to package_macos.py (the fish models)")
    ap.add_argument("--skip-build", action="store_true",
                    help="Use the existing target/release/world_builder")
    ap.add_argument("--skip-notarize", action="store_true",
                    help="Local test only: sign but don't notarize (never publish such a build)")
    args = ap.parse_args()
    if sys.platform != "darwin":
        raise SystemExit("Releases build only on macOS")
    if not re.fullmatch(r"\d+\.\d+\.\d+", args.version):
        raise SystemExit("Version must look like 0.1.3")
    notes = args.notes.resolve()
    if not notes.is_file():
        raise SystemExit(f"No release notes at {notes}")
    out = ROOT / "target/release-mac" / args.version
    if out.exists():
        raise SystemExit(f"{out} exists: remove it or pick another version")
    profile = os.environ.get("SKYFOLD_NOTARY_PROFILE", "moonshot-notary")
    account = os.environ.get("SKYFOLD_SPARKLE_ACCOUNT", "ed25519")
    ident = identity()
    print(f"Signing identity {ident}, notary profile {profile}")

    sp = sparkle()
    public_key = run(sp / "bin/generate_keys", "--account", account, "-p", capture=True).strip()
    if not public_key:
        raise SystemExit("No Sparkle EdDSA key in the keychain (run generate_keys)")

    binary = ROOT / "target/release/world_builder"
    if not args.skip_build:
        step("Release build")
        run("cargo", "build", "--release", "--locked", cwd=ROOT)
    out.mkdir(parents=True)
    meta = out / "cargo-metadata.json"
    meta.write_text(run("cargo", "metadata", "--format-version", "1", "--locked", capture=True,
                        cwd=ROOT))

    step("Staging the app")
    stage = out / "dmg"
    run(sys.executable, ROOT / "tools/release/package_macos.py", "--output", stage,
        "--binary", binary, "--version", args.version, "--cargo-metadata", meta,
        "--sparkle-framework", sp / "Sparkle.framework", "--feed-url", FEED_URL,
        "--ed-public-key", public_key,
        *(["--include-owner-media"] if args.include_owner_media else []))
    meta.unlink()
    app = stage / APP_NAME

    step("Signing")
    sign_app(app, ident)

    if not args.skip_notarize:
        step("Notarizing the app (the upload takes a while)")
        upload = out / "app-notarize.zip"
        run("/usr/bin/ditto", "-c", "-k", "--keepParent", app, upload)
        notarize(upload, profile)
        upload.unlink()
        run("xcrun", "stapler", "staple", app)

    step("DMG")
    dmg = out / DMG_NAME
    run("hdiutil", "create", "-volname", "Skyfold Studio", "-srcfolder", stage, "-fs", "HFS+",
        "-format", "ULFO", dmg)
    run("/usr/bin/codesign", "--force", "--timestamp", "--sign", ident, dmg)
    if dmg.stat().st_size >= ASSET_LIMIT:
        raise SystemExit(f"{dmg.name} is {dmg.stat().st_size / 1024**3:.2f} GiB: GitHub's "
                         "release asset limit is 2 GiB")
    if not args.skip_notarize:
        step("Notarizing the DMG")
        notarize(dmg, profile)
        run("xcrun", "stapler", "staple", dmg)
        run("xcrun", "stapler", "validate", dmg)

    step("Sparkle feed")
    feed = out / "sparkle"
    feed.mkdir()
    # a versioned name for generate_appcast (it caches by archive), the stable asset name in the URL
    archive = feed / f"Skyfold-Studio-{args.version}.dmg"
    run("/bin/cp", "-c", dmg, archive)
    shutil.copy(notes, feed / f"Skyfold-Studio-{args.version}.md")
    prefix = f"https://github.com/{REPO}/releases/download/v{args.version}/"
    run(sp / "bin/generate_appcast", "--account", account, "--download-url-prefix", prefix,
        "--embed-release-notes", "--link", "https://skyfoldstudio.com", feed)
    appcast = (feed / "appcast.xml").read_text()
    appcast = appcast.replace(prefix + archive.name, prefix + DMG_NAME)
    if (prefix + DMG_NAME not in appcast or "sparkle:edSignature" not in appcast
            or f"<sparkle:version>{args.version}</sparkle:version>" not in appcast):
        raise SystemExit("appcast.xml is missing the new version, its URL or its signature")
    (out / "appcast.xml").write_text(appcast)
    shutil.rmtree(feed)
    # generate_appcast keeps an unpacked copy of every archive: gigabytes
    shutil.rmtree(Path.home() / "Library/Caches/Sparkle_generate_appcast", ignore_errors=True)

    (out / "SHA256SUMS.txt").write_text(f"{sha256(dmg)}  {DMG_NAME}\n")
    shutil.rmtree(stage)
    print(f"\n✅ Skyfold Studio {args.version}: {out}")
    print(f"   {DMG_NAME} ({dmg.stat().st_size / 1024**3:.2f} GiB), appcast.xml, SHA256SUMS.txt")
    if args.skip_notarize:
        print("   NOT notarized: a local test build, don't publish it")
    else:
        print(f"   Publish: python3 tools/release/publish_macos.py {args.version} "
              f"--notes {args.notes}")


if __name__ == "__main__":
    main()
