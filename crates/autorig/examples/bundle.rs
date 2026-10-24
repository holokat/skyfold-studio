//! Builds Skyfold's bundled character assets from the Quaternius Universal Animation Library
//! (CC0): `cargo run -p autorig --release --example bundle -- <UAL folder> <assets/characters>`.
//! The folder holds UAL1_Standard.glb, UAL2_Standard.glb and Mannequin_F.glb (from the
//! "Standard" zips' Unreal-Godot folders).

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (src, out) = (Path::new(&args[1]), Path::new(&args[2]));
    std::fs::create_dir_all(out.join("animations")).unwrap();
    std::fs::create_dir_all(out.join("builtin")).unwrap();
    for f in ["UAL1_Standard.glb", "UAL2_Standard.glb"] {
        let to = out.join("animations").join(f.replace("_Standard", "").to_lowercase());
        std::fs::copy(src.join(f), &to).unwrap();
        println!("{}", to.display());
    }
    for (f, name) in [("UAL1_Standard.glb", "mannequin_m.glb"), ("Mannequin_F.glb", "mannequin_f.glb")] {
        let mut m = autorig::import::load(&src.join(f)).unwrap();
        autorig::markers::normalize(&mut m, true);
        m.clips.clear();
        let bytes = autorig::glb::write(&m, &[]);
        let to = out.join("builtin").join(name);
        std::fs::write(&to, bytes).unwrap();
        println!("{} ({} vertices)", to.display(), m.vertex_count());
    }
}
