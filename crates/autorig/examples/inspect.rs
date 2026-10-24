//! Prints a model's skeleton, bounds and how it deforms under a library clip:
//! `cargo run -p autorig --release --example inspect -- <model> [<clip source glb> <clip name>]`.
use std::path::Path;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let m = autorig::import::load(Path::new(&a[1])).unwrap();
    println!("bounds {:?} meshes {} skinned {}", m.bounds(), m.meshes.len(), m.meshes.iter().filter(|x| x.skin.is_some()).count());
    let sk = m.skeleton.clone().unwrap();
    let w = sk.rest_world();
    for (b, wm) in sk.bones.iter().zip(&w).take(12) {
        println!("  {} parent {:?} pos {:?} scale {:?}", b.name, b.parent, wm.w_axis.truncate(), b.rest.s);
    }
    let map = autorig::humanoid::map_bones(&sk);
    println!("mapped {} humanoid {}", map.iter().filter(|x| x.is_some()).count(), autorig::humanoid::is_humanoid(&map));
    if a.len() > 3 {
        let src = autorig::import::load(Path::new(&a[2])).unwrap();
        let clip = src.clips.iter().find(|c| c.name == a[3]).unwrap();
        let r = autorig::retarget::Retarget::new(src.skeleton.as_ref().unwrap(), &sk);
        let pose = r.pose(clip, 0.5, Default::default());
        let d = autorig::rig::deform(&m, &pose);
        let (mut lo, mut hi) = (glam::Vec3::MAX, glam::Vec3::MIN);
        for p in d.iter().flatten() { lo = lo.min(*p); hi = hi.max(*p); }
        println!("posed bounds {lo:?} {hi:?}");
        let rest: Vec<_> = sk.bones.iter().map(|b| b.rest).collect();
        let d = autorig::rig::deform(&m, &rest);
        let (mut lo, mut hi) = (glam::Vec3::MAX, glam::Vec3::MIN);
        for p in d.iter().flatten() { lo = lo.min(*p); hi = hi.max(*p); }
        println!("rest-pose bounds {lo:?} {hi:?}");
    }
}
