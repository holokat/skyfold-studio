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

Paid marketplace packs, including the painted surface texture pack, are not bundled. A world referencing an uninstalled pack needs that pack installed separately. Release builds always disable developer unlock, including environment variables and previously saved developer preferences.

## Maintainer packaging

`tools/release/package_macos.py` stages an app using an explicit resource allowlist. It excludes paid pack directories, authoring files, raw Central SVGs, imported user files and the removed zone-entry sound. Its manifest records a SHA-256 for every included resource. Include owner-provided fish and audio only when their redistribution rights are confirmed.

`tools/release/prepare_source.py` exports a clean source tree to a new directory outside the development checkout. It copies only approved source inputs and refuses symlinks, private asset paths, oversized source files and recognized credential patterns. It never copies development Git history.

```sh
python3 tools/release/prepare_source.py audit
python3 tools/release/prepare_source.py /absolute/path/to/new-source-tree
python3 tools/release/prepare_source.py audit-public /absolute/path/to/new-source-tree
```

The exporter retains opaque legacy metadata solely so existing worlds can load and save without losing unrelated data. Effects Studio is not part of the editor.
