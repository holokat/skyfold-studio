//! Writes front and side silhouettes of a model as PGM pictures:
//! `cargo run -p autorig --release --example silhouette -- <model> <out prefix>`.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let m = autorig::import::load(std::path::Path::new(&a[1])).unwrap();
    let (lo, hi) = m.bounds().unwrap();
    for (view, ax) in [("front", 0usize), ("side", 2usize)] {
        let n = 400usize;
        let size = (hi - lo).max_element();
        let mut img = vec![255u8; n * n];
        for mesh in &m.meshes {
            for p in &mesh.positions {
                let u = ((p[ax] - lo[ax]) / size * (n - 1) as f32) as usize;
                let v = ((hi.y - p.y) / size * (n - 1) as f32) as usize;
                let d = ((p[2 - ax] - lo[2 - ax]) / size * 200.0) as u8;
                img[v.min(n - 1) * n + u.min(n - 1)] = img[v.min(n - 1) * n + u.min(n - 1)].min(30 + d);
            }
        }
        let mut out = format!("P5\n{n} {n}\n255\n").into_bytes();
        out.extend(img);
        std::fs::write(format!("{}_{view}.pgm", a[2]), out).unwrap();
    }
}
