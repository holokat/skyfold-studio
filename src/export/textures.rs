//! One shared texture folder per export: the images every exported GLB would embed are written
//! once (deduplicated by content) and the GLBs refer to them by relative URI. Skyborn and
//! hand-painted buildings share their texture sets, roads and caves reuse the same ground
//! materials, so this saves both disk and engine memory (each engine imports a texture once).
//! `textures.json` lists every file with its role (colour, normal map or data), for importers
//! that need the right texture settings before the models arrive.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{Value, json};

use super::core::{Ctx, safe_name};
use super::glb::{self, ImageOut};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// sRGB colour (base colour, emissive)
    Color,
    /// tangent-space normal map (linear)
    Normal,
    /// linear data (metallic-roughness, occlusion)
    Data,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Color => "color",
            Role::Normal => "normal",
            Role::Data => "data",
        }
    }
}

#[derive(Default)]
pub struct Shared {
    /// folder below the export root (`None`: images stay embedded)
    dir: Mutex<Option<String>>,
    /// content hash → (file name, role)
    files: Mutex<HashMap<u64, (String, Role)>>,
}

/// FNV-1a over the bytes (and their length).
fn hash(b: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ b.len() as u64;
    for x in b {
        h = (h ^ *x as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// `to` relative to the folder `from` (both below the export root, `/`-separated).
pub fn relative(from: &str, to: &str) -> String {
    let a: Vec<&str> = from.split('/').filter(|s| !s.is_empty()).collect();
    let b: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut out: Vec<&str> = vec![".."; a.len() - common];
    out.extend(&b[common..]);
    out.join("/")
}

/// What each image of a GLB is used as (by its materials; unused ones count as colour).
fn roles(doc: &Value) -> HashMap<usize, Role> {
    let textures = doc
        .get("textures")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();
    let image_of = |t: &Value| -> Option<usize> {
        let ti = t.get("index")?.as_u64()? as usize;
        let tex = textures.get(ti)?;
        // texture-format extensions carry their own source
        let ext = tex
            .get("extensions")
            .and_then(|e| e.as_object())
            .and_then(|e| e.values().find_map(|x| x.get("source")?.as_u64()));
        Some(ext.or_else(|| tex.get("source")?.as_u64())? as usize)
    };
    let mut out = HashMap::new();
    for m in doc
        .get("materials")
        .and_then(|m| m.as_array())
        .into_iter()
        .flatten()
    {
        let pbr = m.get("pbrMetallicRoughness");
        for (slot, role) in [
            (pbr.and_then(|p| p.get("baseColorTexture")), Role::Color),
            (m.get("emissiveTexture"), Role::Color),
            (m.get("normalTexture"), Role::Normal),
            (m.get("occlusionTexture"), Role::Data),
            (
                pbr.and_then(|p| p.get("metallicRoughnessTexture")),
                Role::Data,
            ),
        ] {
            if let Some(i) = slot.and_then(image_of) {
                out.entry(i).or_insert(role);
            }
        }
    }
    out
}

impl Shared {
    /// Images go to `dir` (below the export root) from now on.
    pub fn enable(&self, dir: &str) {
        if let Ok(mut d) = self.dir.lock() {
            *d = Some(dir.trim_end_matches('/').to_string());
        }
    }

    pub fn dir(&self) -> Option<String> {
        self.dir.lock().ok().and_then(|d| d.clone())
    }

    /// The GLB to write at `rel` (below the export root): its embedded PNG / JPEG images moved to
    /// the shared folder (each written once) and referred to by URI. Unchanged when sharing is
    /// off or the file can't be rewritten.
    pub fn externalize(&self, ctx: &Ctx, bytes: Vec<u8>, rel: &str) -> Result<Vec<u8>, String> {
        let Some(dir) = self.dir() else {
            return Ok(bytes);
        };
        let Some(doc) = glb::glb_json(&bytes) else {
            return Ok(bytes);
        };
        let roles = roles(&doc);
        let from = rel.rsplit_once('/').map_or("", |(d, _)| d).to_string();
        let mut err = None;
        let out = glb::map_images(&bytes, |k, data, mime, name| {
            let ext = match mime {
                "image/png" => "png",
                "image/jpeg" => "jpg",
                _ => return None,
            };
            let role = roles.get(&k).copied().unwrap_or(Role::Color);
            let h = hash(data);
            let file = {
                let mut files = self.files.lock().ok()?;
                match files.get(&h) {
                    Some((f, _)) => f.clone(),
                    None => {
                        let stem = if name.is_empty() {
                            "texture".to_string()
                        } else {
                            safe_name(name)
                        };
                        let f = format!("{stem}_{:08x}.{ext}", h as u32);
                        if let Err(e) = ctx.write(&format!("{dir}/{f}"), data) {
                            err = Some(e);
                            return None;
                        }
                        files.insert(h, (f.clone(), role));
                        f
                    }
                }
            };
            Some(ImageOut::Uri(relative(&from, &format!("{dir}/{file}"))))
        });
        if let Some(e) = err {
            return Err(e);
        }
        Ok(out.unwrap_or(bytes))
    }

    /// Every shared file with its role, sorted by name.
    pub fn list(&self) -> Vec<(String, Role)> {
        let mut v: Vec<(String, Role)> = self
            .files
            .lock()
            .map(|f| f.values().cloned().collect())
            .unwrap_or_default();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Writes `textures.json` into the shared folder (if any image went there).
    pub fn write_index(&self, ctx: &Ctx) -> Result<Value, String> {
        let Some(dir) = self.dir() else {
            return Ok(Value::Null);
        };
        let list = self.list();
        if list.is_empty() {
            return Ok(Value::Null);
        }
        let v = json!({
            "note": "images shared by the exported GLB files (referenced by relative URI); role: color = sRGB, normal = tangent-space normal map (OpenGL / +Y), data = linear (metallic-roughness, occlusion)",
            "textures": list.iter().map(|(f, r)| json!({ "file": f, "role": r.name() })).collect::<Vec<_>>(),
        });
        ctx.write_json(&format!("{dir}/textures.json"), &v)?;
        Ok(json!({ "folder": dir, "files": list.len() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths() {
        assert_eq!(
            relative(
                "WorldBuilder/Worlds/X/Models",
                "WorldBuilder/Worlds/X/Textures/a.png"
            ),
            "../Textures/a.png"
        );
        assert_eq!(relative("models", "textures/a.png"), "../textures/a.png");
        assert_eq!(relative("", "textures/a.png"), "textures/a.png");
        assert_eq!(relative("a/b", "a/b/c.png"), "c.png");
    }
}
