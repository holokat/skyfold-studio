# Build Skyfold Studio

The public repository contains the MIT source. The macOS app includes the free runtime resources and interface icons needed to run it. Users downloading the app do not need a separate license or developer setup.

## Build from source

Use the Rust version in `rust-toolchain.toml`. On macOS, install Xcode Command Line Tools and work in a normal writable checkout.

1. [Download the macOS app](https://github.com/holokat/skyfold-studio/releases/latest/download/Skyfold-Studio-macOS.dmg) and place it in Applications. Restore its free runtime resources into your checkout:

   ```sh
   cp -cR '/Applications/Skyfold Studio.app/Contents/Resources/assets/.' assets/
   ```

2. Supply your licensed Central SVG exports for the editor interface:

   ```sh
   python3 tools/release/restore_icons.py /path/to/your/central-svg-exports
   ```

   Central Icons are licensed separately. The helper lists missing filenames and never requests or stores a license key. Keep these files private. They are interface artwork, not world assets.

3. Build and run:

   ```sh
   cargo run --release --locked
   ```

The available DMG is for Apple silicon. Other platforms do not yet have packaged releases.

## Saved worlds and imports

The packaged app reads resources from `Contents/Resources/assets` and writes worlds, settings, imports and caches under `~/Library/Application Support/Skyfold Studio`. It can run from a read-only DMG. Development builds keep their data in the checkout. Existing development worlds can be opened through File without moving or replacing them.

Paid marketplace packs (the hand-painted Skyfold Studio Collection models) are not bundled; the painted surfaces for hand-painted worlds are free and ship with the app. A world referencing an uninstalled pack needs that pack installed separately. Release builds always disable developer unlock, including environment variables and previously saved developer preferences. The studio's own screenshot build (`cargo build --profile studio --features studio_dev`, in `target/studio`) keeps it; the packager refuses that binary.

## Maintainer packaging

`tools/release/package_macos.py` stages an app using an explicit resource allowlist. It excludes paid pack directories, authoring files, raw Central SVGs, imported user files and the removed zone-entry sound. Its manifest records a SHA-256 for every included resource. Include owner-provided fish and audio only when their redistribution rights are confirmed.

### Releases and auto-updates

The app updates itself with [Sparkle 2](https://sparkle-project.org). Each release embeds `Sparkle.framework`, and its Info.plist points `SUFeedURL` at the latest GitHub release's `appcast.xml`. The app checks once a day and from File › Check for Updates…. Sparkle then shows the release notes and offers Install Update, then Install and Relaunch. The editor saves the open world (as closing the window does) before it relaunches. Development builds have no framework and skip all of this (`src/updates.rs`).

```sh
python3 tools/release/release_macos.py 0.1.3 --notes notes.md   # build, sign, notarize, DMG, appcast
python3 tools/release/publish_macos.py 0.1.3 --notes notes.md   # GitHub release v0.1.3 = the update
```

`release_macos.py` builds the release binary and stages it with `package_macos.py`. It then signs inside-out with the longest-valid Developer ID Application certificate and the hardened runtime, notarizes and staples the app and the DMG, and writes an EdDSA-signed `appcast.xml` whose update is the DMG itself. Output goes to `target/release-mac/<version>/` and nothing is uploaded. It needs a notarytool keychain profile (`SKYFOLD_NOTARY_PROFILE`, default `moonshot-notary`) and Sparkle's EdDSA key in the login keychain (`SKYFOLD_SPARKLE_ACCOUNT`, default `ed25519`). It downloads Sparkle once and checks it against a pinned SHA-256. Versions must increase. Release notes are Markdown and show in Sparkle's update window and on GitHub. `--skip-notarize` makes a local test build that must not be published.

`tools/release/prepare_source.py` exports a clean source tree to a new directory outside the development checkout. It copies only approved source inputs and refuses symlinks, private asset paths, oversized source files and recognized credential patterns. It never copies development Git history.

```sh
python3 tools/release/prepare_source.py audit
python3 tools/release/prepare_source.py /absolute/path/to/new-source-tree
python3 tools/release/prepare_source.py audit-public /absolute/path/to/new-source-tree
```

The exporter retains opaque legacy metadata solely so existing worlds can load and save without losing unrelated data. Effects Studio is not part of the editor.
