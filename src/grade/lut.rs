//! A 3D colour lookup table and the user's LUT as saved with a world or a style.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use rayon::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// `size`³ colours over [0, 1]³, red fastest, then green, then blue (the layout of a `.cube`
/// file and of a 3D texture with x = red, y = green, z = blue).
#[derive(Clone, Debug, PartialEq)]
pub struct Lut3 {
    pub size: usize,
    pub data: Vec<[f32; 3]>,
}

impl Lut3 {
    pub fn from_fn(n: usize, f: impl Fn([f32; 3]) -> [f32; 3] + Sync) -> Lut3 {
        let n = n.max(2);
        let at = |i: usize| i as f32 / (n - 1) as f32;
        let data = (0..n)
            .into_par_iter()
            .flat_map_iter(|b| {
                let f = &f;
                (0..n).flat_map(move |g| (0..n).map(move |r| f([at(r), at(g), at(b)])))
            })
            .collect();
        Lut3 { size: n, data }
    }

    pub fn identity(n: usize) -> Lut3 {
        Lut3::from_fn(n, |c| c)
    }

    fn at(&self, r: usize, g: usize, b: usize) -> [f32; 3] {
        self.data[r + g * self.size + b * self.size * self.size]
    }

    /// Trilinear lookup (input clamped to [0, 1]).
    pub fn sample(&self, c: [f32; 3]) -> [f32; 3] {
        let m = (self.size - 1) as f32;
        let p = c.map(|v| v.clamp(0.0, 1.0) * m);
        let i0 = p.map(|v| (v.floor() as usize).min(self.size - 2));
        let t = [0, 1, 2].map(|k| p[k] - i0[k] as f32);
        let mut out = [0.0; 3];
        for (db, wb) in [(0, 1.0 - t[2]), (1, t[2])] {
            for (dg, wg) in [(0, 1.0 - t[1]), (1, t[1])] {
                for (dr, wr) in [(0, 1.0 - t[0]), (1, t[0])] {
                    let w = wr * wg * wb;
                    if w > 0.0 {
                        let v = self.at(i0[0] + dr, i0[1] + dg, i0[2] + db);
                        for k in 0..3 {
                            out[k] += v[k] * w;
                        }
                    }
                }
            }
        }
        out
    }

    pub fn resample(&self, n: usize) -> Lut3 {
        if n == self.size {
            return self.clone();
        }
        Lut3::from_fn(n, |c| self.sample(c))
    }

    pub fn is_identity(&self, tolerance: f32) -> bool {
        let m = (self.size - 1) as f32;
        self.data.iter().enumerate().all(|(i, v)| {
            let c = [
                (i % self.size) as f32 / m,
                (i / self.size % self.size) as f32 / m,
                (i / (self.size * self.size)) as f32 / m,
            ];
            (0..3).all(|k| (v[k] - c[k]).abs() <= tolerance)
        })
    }

    pub fn content_hash(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.size.hash(&mut h);
        for v in &self.data {
            for k in v {
                k.to_bits().hash(&mut h);
            }
        }
        h.finish()
    }

    /// RGBA 16-bit float texels for a 3D texture.
    pub fn to_rgba16f(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.data.len() * 8);
        for v in &self.data {
            for k in [v[0], v[1], v[2], 1.0] {
                out.extend_from_slice(&half::f16::from_f32(k).to_le_bytes());
            }
        }
        out
    }

    /// Compact text form (half floats, deflated, base64) for saving inside JSON.
    fn encode(&self) -> String {
        let mut raw = Vec::with_capacity(self.data.len() * 6);
        for v in &self.data {
            for k in v {
                raw.extend_from_slice(&half::f16::from_f32(*k).to_le_bytes());
            }
        }
        base64_encode(&miniz_oxide::deflate::compress_to_vec(&raw, 8))
    }

    fn decode(size: usize, text: &str) -> Result<Lut3, String> {
        if !(2..=65).contains(&size) {
            return Err(format!("LUT size {size} is out of range"));
        }
        let packed = base64_decode(text).ok_or("the LUT data is not valid base64")?;
        let raw = miniz_oxide::inflate::decompress_to_vec(&packed)
            .map_err(|e| format!("the LUT data is damaged ({e:?})"))?;
        if raw.len() != size * size * size * 6 {
            return Err("the LUT data has the wrong length".into());
        }
        let f = |i: usize| half::f16::from_le_bytes([raw[i], raw[i + 1]]).to_f32();
        let data = (0..size * size * size)
            .map(|i| [f(i * 6), f(i * 6 + 2), f(i * 6 + 4)])
            .collect();
        Ok(Lut3 { size, data })
    }
}

/// The user's own LUT on a world's look (or in a saved style): applied after the look's grade.
/// Cheap to clone; compared by name and content hash, not cell by cell.
#[derive(Clone)]
pub struct UserLut {
    /// Shown in the UI (the file it came from, without extension).
    pub name: String,
    pub lut: Arc<Lut3>,
    pub hash: u64,
}

impl UserLut {
    pub fn new(name: &str, lut: Lut3) -> UserLut {
        let mut lut = lut.resample(super::RUNTIME_SIZE);
        // stored as half floats: quantise now so a saved and reloaded LUT compares equal
        for v in &mut lut.data {
            *v = v.map(|k| half::f16::from_f32(k).to_f32());
        }
        UserLut {
            name: name.trim().chars().take(80).collect(),
            hash: lut.content_hash(),
            lut: Arc::new(lut),
        }
    }
}

impl PartialEq for UserLut {
    fn eq(&self, o: &UserLut) -> bool {
        self.name == o.name && self.hash == o.hash
    }
}

impl std::fmt::Debug for UserLut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "UserLut({:?}, {}³)", self.name, self.lut.size)
    }
}

#[derive(Serialize, Deserialize)]
struct UserLutFile {
    name: String,
    size: usize,
    data: String,
}

impl Serialize for UserLut {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        UserLutFile {
            name: self.name.clone(),
            size: self.lut.size,
            data: self.lut.encode(),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for UserLut {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<UserLut, D::Error> {
        let f = UserLutFile::deserialize(d)?;
        let lut = Lut3::decode(f.size, &f.data).map_err(serde::de::Error::custom)?;
        Ok(UserLut {
            name: f.name,
            hash: lut.content_hash(),
            lut: Arc::new(lut),
        })
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for n in 0..10 {
            let v: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(base64_decode(&base64_encode(&v)).unwrap(), v);
        }
    }

    #[test]
    fn identity_samples_to_itself_and_resamples() {
        let id = Lut3::identity(5);
        let c = [0.13, 0.57, 0.91];
        let s = id.sample(c);
        for k in 0..3 {
            assert!((s[k] - c[k]).abs() < 1e-5);
        }
        assert!(id.resample(33).is_identity(1e-5));
    }

    #[test]
    fn a_user_lut_survives_json() {
        let warm = Lut3::from_fn(9, |c| [c[0].powf(0.8), c[1], c[2] * 0.9]);
        let u = UserLut::new("Warm", warm);
        let json = serde_json::to_string(&u).unwrap();
        let back: UserLut = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "Warm");
        assert_eq!(back.lut.size, super::super::RUNTIME_SIZE);
        let (a, b) = (u.lut.sample([0.4, 0.4, 0.4]), back.lut.sample([0.4, 0.4, 0.4]));
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 1e-3, "{a:?} vs {b:?}");
        }
    }
}
