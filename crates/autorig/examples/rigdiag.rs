//! Rigs a prepared model with its guessed markers and reports where vertices went unreached:
//! `cargo run -p autorig --release --example rigdiag -- <prepared.glb>`.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut m = autorig::import::load(std::path::Path::new(&a[1])).unwrap();
    let (lo, hi) = m.bounds().unwrap();
    println!("{} meshes, {} vertices, {} triangles, bounds {lo:?} {hi:?}", m.meshes.len(), m.vertex_count(), m.triangle_count());
    for mesh in &m.meshes { println!("  mesh {} {} verts", mesh.name, mesh.positions.len()); }
    let sil = autorig::markers::Silhouette::new(&m, 240).unwrap();
    let h = hi.y - lo.y;
    let mut y = hi.y - sil.cell * 0.5;
    while y > lo.y + 0.3 * h {
        let w = sil.run_containing(y, 0.0).map(|(a, b)| b - a);
        println!("  y {:.3} ({:.2}h) width {:?} runs {}", y, (y - lo.y) / h, w, sil.runs(y).len());
        y -= sil.cell * 4.0;
    }
    let mk = autorig::markers::guess(&m);
    println!("markers {mk:?}");
    let (_, rep) = autorig::rig::auto_rig(&mut m, Some(mk), Default::default());
    println!("{rep:?}");
}
