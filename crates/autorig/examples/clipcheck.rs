//! Imports an animation file and reports its skeleton mapping and clips:
//! `cargo run -p autorig --release --example clipcheck -- <file>…`.
fn main() {
    for f in std::env::args().skip(1) {
        match autorig::import::load(std::path::Path::new(&f)) {
            Ok(m) => {
                let sk = m.skeleton.as_ref().unwrap();
                let map = autorig::humanoid::map_bones(sk);
                println!("{f}: {} bones, {} mapped, humanoid {}, missing {:?}", sk.bones.len(), map.iter().filter(|x| x.is_some()).count(), autorig::humanoid::is_humanoid(&map), autorig::humanoid::missing(&map));
                for c in &m.clips { println!("  clip {} {} frames {:.2}s", c.name, c.frames, c.duration()); }
                let t = autorig::humanoid::template().skeleton();
                let r = autorig::retarget::Retarget::new(sk, &t);
                println!("  retarget onto the standard skeleton: {} of {} bones follow", r.mapped(), t.bones.len());
            }
            Err(e) => println!("{f}: {e}"),
        }
    }
}
