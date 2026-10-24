//! Fits the standard skeleton to a model and lists the joints furthest from the template's
//! (for a model that *is* the template, e.g. UAL1_Standard.glb):
//! `cargo run -p autorig --release --example fitdiag -- <model>`.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut m = autorig::import::load(std::path::Path::new(&a[1])).unwrap();
    autorig::markers::normalize(&mut m, true);
    let mk = autorig::markers::guess(&m);
    println!("{mk:?}");
    let j = autorig::fit::joints(&m, &mk);
    let t = autorig::humanoid::template();
    let sk = m.skeleton.as_ref().unwrap();
    let w = sk.rest_world();
    let mut e: Vec<(f32, &str, glam::Vec3)> = t.bones.iter().enumerate().map(|(i, b)| {
        let o = w[sk.find(b.name).unwrap()].w_axis.truncate();
        ((j[i] - o).length(), b.name, j[i] - o)
    }).collect();
    e.sort_by(|a, b| b.0.total_cmp(&a.0));
    for x in e.iter().take(10) { println!("{:>16} {:.3} {:?}", x.1, x.0, x.2); }
}
