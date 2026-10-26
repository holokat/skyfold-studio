//! Photoscanned props: CC0 scans from Poly Haven (see `assets/models/photoscan/CREDITS.md`) for
//! boulders, mossy rocks, rock clusters, pebbles, stumps, fallen logs, driftwood, roots, ferns,
//! meadow flowers, tall grass tufts, branch piles and desert scrub, and the tall rock formations
//! (sea stacks, hoodoos, rock spires) assembled from cliff and boulder scans.
//!
//! `tools/process_photoscan.py` prepares them offline: Blender simplifies each scan into levels of
//! detail, the scan's detail is re-baked into a normal map for the simplified LOD 0, and the scans
//! of one kind share one texture atlas (one material, so all its variants batch), block-compressed
//! with its mips. At startup the compact `<name>.lods.bin` meshes and the KTX2 atlases are read in
//! parallel (no glTF parsing, no asset-server round trip, no mip generation) and handed to the
//! library as `TreeParts`: they render through the same LOD + visibility-range path as the ez-tree
//! trees. Tall grass is crossed photo cards cut from the grass scan's own pictures of whole tufts.
//! The formations come from `tools/process_formations.py`, in the same format. Cliff rocks keep
//! their procedural shape but wear a tiling photoscanned rock texture (see `Dressed`). A kind whose
//! files are missing or unreadable keeps its procedural mesh and vertex colours.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::asset::RenderAssetUsages;
use bevy::image::{
    CompressedImageFormatSupport, CompressedImageFormats, ImageAddressMode, ImageFilterMode,
    ImageSampler, ImageSamplerDescriptor,
};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::objects::{FoliageExt, FoliageMaterial};
use crate::procgen::meshes::{AssetKind, MeshData, build_asset};
use crate::trees::{TreeParts, TreeVariant};

/// A library kind drawn from photoscans.
struct Prop {
    kind: AssetKind,
    /// `<file>.lods.bin`, `<file>_albedo.ktx2` and `<file>_normal.ktx2` in `assets/models/photoscan`
    file: &'static str,
    /// Where LOD 0→1 and 1→2 switch and where the prop disappears (m), as in `TreeParts::ranges`.
    ranges: [f32; 4],
}

const PROPS: [Prop; 18] = [
    // rocks keep a far LOD out to the horizon, like the procedural ones
    Prop {
        kind: AssetKind::Boulder,
        file: "boulder",
        ranges: [25.0, 110.0, 4000.0, 4000.0],
    },
    Prop {
        kind: AssetKind::MossyRock,
        file: "mossy_rock",
        ranges: [25.0, 110.0, 4000.0, 4000.0],
    },
    Prop {
        kind: AssetKind::RockCluster,
        file: "rock_cluster",
        ranges: [25.0, 110.0, 4000.0, 4000.0],
    },
    // a patch of stones is sub-pixel long before the horizon
    Prop {
        kind: AssetKind::Pebbles,
        file: "pebbles",
        ranges: [12.0, 40.0, 250.0, 250.0],
    },
    Prop {
        kind: AssetKind::Driftwood,
        file: "driftwood",
        ranges: [25.0, 90.0, 450.0, 450.0],
    },
    Prop {
        kind: AssetKind::Stump,
        file: "stump",
        ranges: [18.0, 80.0, 450.0, 450.0],
    },
    Prop {
        kind: AssetKind::FallenLog,
        file: "fallen_log",
        ranges: [25.0, 90.0, 450.0, 450.0],
    },
    Prop {
        kind: AssetKind::Roots,
        file: "roots",
        ranges: [25.0, 90.0, 450.0, 450.0],
    },
    Prop {
        kind: AssetKind::Branches,
        file: "branches",
        ranges: [12.0, 45.0, 450.0, 450.0],
    },
    // small plants: one LOD, near only and (objects.rs) no shadows, like the procedural ones
    Prop {
        kind: AssetKind::Fern,
        file: "fern",
        ranges: [95.0, 95.0, 95.0, 95.0],
    },
    Prop {
        kind: AssetKind::Flowers,
        file: "flowers",
        ranges: [75.0, 75.0, 75.0, 75.0],
    },
    Prop {
        kind: AssetKind::TallGrass,
        file: "grass_tuft",
        ranges: [75.0, 75.0, 75.0, 75.0],
    },
    // dry desert scrub (Poly Haven's wild rooibos): two LODs, gone where it would be sub-pixel
    Prop {
        kind: AssetKind::DesertShrub,
        file: "desert_shrub",
        ranges: [30.0, 110.0, 400.0, 400.0],
    },
    // alien flora (sci-fi template, recoloured by `nature::alien`): the ~5 m quiver tree holds
    // out like a small tree; the knee-high succulent clumps and rosettes like the desert scrub
    Prop {
        kind: AssetKind::QuiverTree,
        file: "quiver_tree",
        ranges: [40.0, 140.0, 900.0, 900.0],
    },
    Prop {
        kind: AssetKind::Succulent,
        file: "succulents",
        ranges: [20.0, 60.0, 220.0, 220.0],
    },
    // tall formations (tools/process_formations.py): big, so each level holds out longer
    Prop {
        kind: AssetKind::SeaStack,
        file: "sea_stack",
        ranges: [70.0, 260.0, 4000.0, 4000.0],
    },
    Prop {
        kind: AssetKind::Hoodoo,
        file: "hoodoo",
        ranges: [40.0, 150.0, 4000.0, 4000.0],
    },
    Prop {
        kind: AssetKind::RockSpire,
        file: "rock_spire",
        ranges: [50.0, 180.0, 4000.0, 4000.0],
    },
];

/// Kinds that keep their procedural shape, dressed in a tiling photoscanned texture (UV-mapped, with
/// its normal map) instead of flat vertex colours: the CC0 cliff scans are open-backed facades.
struct Dressed {
    kind: AssetKind,
    variants: u32,
    ranges: [f32; 4],
}

const DRESSED: [Dressed; 3] = [
    Dressed {
        kind: AssetKind::CliffRock,
        variants: 3,
        ranges: [110.0, 4000.0, 4000.0, 4000.0],
    },
    // the alien-world rocks (visuals::floating, visuals::monoliths); their tiles keep their size
    // however far they're scaled (visuals::rock_material)
    Dressed {
        kind: AssetKind::FloatingRock,
        variants: crate::visuals::floating::VARIANTS,
        ranges: [120.0, 4000.0, 4000.0, 4000.0],
    },
    Dressed {
        kind: AssetKind::Monolith,
        variants: crate::visuals::monoliths::VARIANTS,
        ranges: [90.0, 3000.0, 3000.0, 3000.0],
    },
];
/// `<file>_albedo.ktx2` / `_normal.ktx2`: Poly Haven's rock_face_03, a 2.7 m tiling scan.
const TILE_FILE: &str = "tile_rock";
const TILE_M: f32 = 2.7;

fn dir() -> PathBuf {
    PathBuf::from(crate::assets_path()).join("models/photoscan")
}

/// Everything read from disk for one prop (built off the main thread).
struct Loaded {
    variants: Vec<(Vec<Mesh>, f32, f32, f32)>,
    roughness: f32,
    cutout: bool,
    /// crossed photo cards: no normal map, normals lean up and stay unflipped on back faces
    cards: bool,
    albedo: Image,
    normal: Option<Image>,
}

/// Loads every photoscanned prop and adds it to `parts` (kinds already there are left alone).
/// Called from `objects::build_library`; `formats` tells whether the GPU samples BC textures.
pub fn build(
    meshes: &mut Assets<Mesh>,
    foliage: &mut Assets<FoliageMaterial>,
    images: &mut Assets<Image>,
    parts: &mut HashMap<AssetKind, TreeParts>,
    formats: Option<&CompressedImageFormatSupport>,
) {
    let t0 = std::time::Instant::now();
    let dir = dir();
    let bc = formats.is_some_and(|f| f.0.contains(CompressedImageFormats::BC));
    let (loaded, dressed): (Vec<Option<Loaded>>, Vec<Vec<(Vec<Mesh>, f32, f32, f32)>>) =
        std::thread::scope(|s| {
            let handles: Vec<_> = PROPS
                .iter()
                .map(|p| {
                    s.spawn(|| {
                        load(&dir, p.file, bc)
                            .map_err(|e| warn!("photoscan {}: {e}", p.file))
                            .ok()
                    })
                })
                .collect();
            let dressed: Vec<_> = DRESSED
                .iter()
                .map(|d| s.spawn(|| dressed_variants(d)))
                .collect();
            (
                handles
                    .into_iter()
                    .map(|h| h.join().ok().flatten())
                    .collect(),
                dressed
                    .into_iter()
                    .map(|h| h.join().unwrap_or_default())
                    .collect(),
            )
        });
    let mut tris = 0;
    let mut texels = 0;
    for (p, l) in PROPS.iter().zip(loaded) {
        let Some(l) = l else { continue };
        if parts.contains_key(&p.kind) {
            continue;
        }
        texels += l.albedo.width() * l.albedo.height();
        parts.insert(p.kind, parts_of(l, p.ranges, meshes, foliage, images, &mut tris));
    }
    let tile = (|| {
        Ok::<_, String>((
            ktx2_image(&dir.join(format!("{TILE_FILE}_albedo.ktx2")), bc, true)?,
            ktx2_image(&dir.join(format!("{TILE_FILE}_normal.ktx2")), bc, true)?,
        ))
    })();
    match tile {
        Ok((albedo, normal)) => {
            let mat = foliage.add(FoliageMaterial {
                base: StandardMaterial {
                    base_color: Color::WHITE,
                    base_color_texture: Some(images.add(albedo)),
                    normal_map_texture: Some(images.add(normal)),
                    perceptual_roughness: 0.82,
                    reflectance: 0.3,
                    ..default()
                },
                extension: FoliageExt::default(),
            });
            for (d, variants) in DRESSED.iter().zip(dressed) {
                if parts.contains_key(&d.kind) || variants.is_empty() {
                    continue;
                }
                let variants = variants
                    .into_iter()
                    .map(|(lods, height, width, min_y)| TreeVariant {
                        lods: lods.into_iter().map(|m| (meshes.add(m), None)).collect(),
                        cell: None,
                        height,
                        width,
                        min_y,
                        trunk: None,
                    })
                    .collect();
                parts.insert(
                    d.kind,
                    TreeParts {
                        variants,
                        bark: mat.clone(),
                        leaves: mat.clone(),
                        ranges: d.ranges,
                    },
                );
            }
        }
        Err(e) => warn!("photoscan {TILE_FILE}: {e}"),
    }
    let kinds = PROPS
        .iter()
        .map(|p| p.kind)
        .chain(DRESSED.iter().map(|d| d.kind))
        .filter(|k| parts.contains_key(k))
        .count();
    let tex = if bc {
        "BC-compressed"
    } else {
        "decoded (no BC support)"
    };
    info!(
        "photoscan props: {kinds} kinds, {tris} triangles over all LODs, {:.1} M texels {tex}, loaded in {:?}",
        texels as f32 / 1e6,
        t0.elapsed()
    );
}

/// The library-facing shape of one loaded prop: its meshes and its one material.
fn parts_of(
    l: Loaded,
    ranges: [f32; 4],
    meshes: &mut Assets<Mesh>,
    foliage: &mut Assets<FoliageMaterial>,
    images: &mut Assets<Image>,
    tris: &mut usize,
) -> TreeParts {
    let mat = foliage.add(FoliageMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(images.add(l.albedo)),
            normal_map_texture: l.normal.map(|n| images.add(n)),
            perceptual_roughness: l.roughness,
            reflectance: 0.3,
            // fronds, petals and blades are single cut-out sheets seen from both sides
            alpha_mode: if l.cutout {
                AlphaMode::Mask(0.5)
            } else {
                AlphaMode::Opaque
            },
            cull_mode: if l.cutout {
                None
            } else {
                Some(bevy::render::render_resource::Face::Back)
            },
            double_sided: l.cutout && !l.cards,
            diffuse_transmission: if l.cutout { 0.3 } else { 0.0 },
            ..default()
        },
        extension: FoliageExt::default(),
    });
    let variants = l
        .variants
        .into_iter()
        .map(|(lods, height, width, min_y)| TreeVariant {
            lods: lods
                .into_iter()
                .map(|m| {
                    *tris += m.indices().map_or(0, |i| i.len() / 3);
                    (meshes.add(m), None)
                })
                .collect(),
            cell: None,
            height,
            width,
            min_y,
            trunk: None,
        })
        .collect();
    TreeParts {
        variants,
        bark: mat.clone(),
        leaves: mat,
        ranges,
    }
}

/// One photoscanned set outside the library (`assets/models/photoscan/<file>.*`): the ground
/// cover's herbs and grass tufts (`crate::groundcover`). `ranges` as in `TreeParts`.
pub fn load_set(
    file: &str,
    ranges: [f32; 4],
    meshes: &mut Assets<Mesh>,
    foliage: &mut Assets<FoliageMaterial>,
    images: &mut Assets<Image>,
    formats: Option<&CompressedImageFormatSupport>,
) -> Option<TreeParts> {
    let bc = formats.is_some_and(|f| f.0.contains(CompressedImageFormats::BC));
    let l = load(&dir(), file, bc)
        .map_err(|e| warn!("photoscan {file}: {e}"))
        .ok()?;
    Some(parts_of(l, ranges, meshes, foliage, images, &mut 0))
}

fn load(dir: &Path, file: &str, bc: bool) -> Result<Loaded, String> {
    // swaying props are the plants, and every plant here is an alpha-tested cut-out
    let (variants, roughness, cutout, cards) = read_lods(&dir.join(format!("{file}.lods.bin")))?;
    let albedo = ktx2_image(&dir.join(format!("{file}_albedo.ktx2")), bc, false)?;
    let normal = if cards {
        None
    } else {
        Some(ktx2_image(
            &dir.join(format!("{file}_normal.ktx2")),
            bc,
            false,
        )?)
    };
    Ok(Loaded {
        variants,
        roughness,
        cutout,
        cards,
        albedo,
        normal,
    })
}

/// The procedural variants of a dressed kind, both LODs each, with UVs for the tiling texture.
fn dressed_variants(d: &Dressed) -> Vec<(Vec<Mesh>, f32, f32, f32)> {
    (0..d.variants)
        .map(|v| {
            let lods: Vec<MeshData> = (0..2).map(|lod| build_asset(d.kind, v, lod)).collect();
            let (mn, mx) = lods[0].bounds();
            let width = (mx[0] - mn[0]).max(mx[2] - mn[2]);
            (
                lods.iter().map(|m| dressed_mesh(m, TILE_M)).collect(),
                mx[1] - mn[1],
                width,
                mn[1],
            )
        })
        .collect()
}

/// Unwelds a procedural mesh (one vertex per triangle corner, so neighbouring triangles may project
/// differently) and adds UVs in tiles of `tile` metres (planar, along each triangle's dominant axis)
/// plus mikktspace tangents. The vertex colours are dropped: the texture carries the colour, and the
/// foliage shader would bend coloured meshes.
fn dressed_mesh(m: &MeshData, tile: f32) -> Mesh {
    let p = |i: u32| Vec3::from_array(m.positions[i as usize]);
    let (mut pos, mut nrm, mut uv) = (vec![], vec![], vec![]);
    for t in m.indices.chunks_exact(3) {
        let c = [p(t[0]), p(t[1]), p(t[2])];
        let n = (c[1] - c[0]).cross(c[2] - c[0]).normalize_or_zero();
        let a = n.abs();
        let uvs = c
            .map(|q| {
                if a.y >= a.x && a.y >= a.z {
                    [q.x, q.z]
                } else if a.x >= a.z {
                    [q.z + 0.37 * tile, -q.y]
                } else {
                    [q.x + 0.71 * tile, -q.y]
                }
            })
            .map(|[u, v]| [u / tile, v / tile]);
        for (k, &i) in t.iter().enumerate() {
            pos.push(m.positions[i as usize]);
            nrm.push(m.normals[i as usize]);
            uv.push(uvs[k]);
        }
    }
    let n = pos.len() as u32;
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    mesh.insert_indices(Indices::U32((0..n).collect()));
    if let Err(e) = mesh.generate_tangents() {
        warn!("photoscan tangents: {e}");
    }
    mesh
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let s = self.b.get(self.at..self.at + n).ok_or("truncated file")?;
        self.at += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}

/// One level of detail as read from a `.lods.bin` file.
#[derive(Clone, Debug, Default)]
pub struct RawMesh {
    pub pos: Vec<[f32; 3]>,
    pub nrm: Vec<[f32; 3]>,
    pub tan: Vec<[f32; 4]>,
    pub uv: Vec<[f32; 2]>,
    /// sway weight (0 rigid … 1 the freest tip)
    pub sway: Vec<f32>,
    pub idx: Vec<u16>,
}

/// A variant: its levels of detail, height, width and lowest point (m).
#[derive(Clone, Debug, Default)]
pub struct RawVariant {
    pub lods: Vec<RawMesh>,
    pub height: f32,
    pub width: f32,
    pub min_y: f32,
}

/// A `.lods.bin` file: its variants, roughness and flags.
#[derive(Clone, Debug, Default)]
pub struct RawLods {
    pub variants: Vec<RawVariant>,
    pub roughness: f32,
    pub sway: bool,
    pub cards: bool,
}

/// `<name>.lods.bin` (little endian, written by tools/process_photoscan.py and tools/crops):
/// "WBPS", version 1, flags (1 = vertex sway weights, 2 = photo cards), roughness, variant count; per variant its
/// height, width and lowest point (m) and LOD count; per LOD the vertex and index counts, 36-byte
/// vertices (position f32x3, normal i16x3, sway u16, tangent i16x4 with handedness, uv f32x2) and
/// u16 triangle indices padded to 4 bytes.
pub(crate) fn read_lods_raw(path: &Path) -> Result<RawLods, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut r = Reader { b: &bytes, at: 0 };
    if r.take(4)? != b"WBPS" || r.u32()? != 1 {
        return Err(format!(
            "{}: not a version 1 photoscan LOD file",
            path.display()
        ));
    }
    let flags = r.u32()?;
    let (sway, cards) = (flags & 1 != 0, flags & 2 != 0);
    let roughness = r.f32()?.clamp(0.05, 1.0);
    let n_variants = r.u32()?;
    let mut variants = vec![];
    for _ in 0..n_variants {
        let (height, width, min_y) = (r.f32()?, r.f32()?, r.f32()?);
        let n_lods = r.u32()?;
        let mut lods = vec![];
        for _ in 0..n_lods {
            let (nv, ni) = (r.u32()? as usize, r.u32()? as usize);
            let v = r.take(nv * 36)?;
            let f = |o: usize| f32::from_le_bytes(v[o..o + 4].try_into().unwrap());
            let sn = |o: usize| (i16::from_le_bytes([v[o], v[o + 1]]) as f32 / 32767.0).max(-1.0);
            let mut m = RawMesh::default();
            for i in 0..nv {
                let o = i * 36;
                m.pos.push([f(o), f(o + 4), f(o + 8)]);
                m.nrm.push(
                    Vec3::new(sn(o + 12), sn(o + 14), sn(o + 16))
                        .normalize_or(Vec3::Y)
                        .to_array(),
                );
                m.sway
                    .push(u16::from_le_bytes([v[o + 18], v[o + 19]]) as f32 / 65535.0);
                let t = Vec3::new(sn(o + 20), sn(o + 22), sn(o + 24)).normalize_or(Vec3::X);
                m.tan
                    .push([t.x, t.y, t.z, if sn(o + 26) < 0.0 { -1.0 } else { 1.0 }]);
                m.uv.push([f(o + 28), f(o + 32)]);
            }
            m.idx = r
                .take(ni * 2)?
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            if ni % 2 == 1 {
                r.take(2)?;
            }
            if m.idx.iter().any(|&i| i as usize >= nv) {
                return Err(format!("{}: index out of range", path.display()));
            }
            lods.push(m);
        }
        if !lods.is_empty() {
            variants.push(RawVariant {
                lods,
                height,
                width,
                min_y,
            });
        }
    }
    if variants.is_empty() {
        return Err(format!("{}: no variants", path.display()));
    }
    Ok(RawLods {
        variants,
        roughness,
        sway,
        cards,
    })
}

fn read_lods(path: &Path) -> Result<(Vec<(Vec<Mesh>, f32, f32, f32)>, f32, bool, bool), String> {
    let raw = read_lods_raw(path)?;
    let sway = raw.sway;
    let variants = raw
        .variants
        .into_iter()
        .map(|v| {
            let lods = v
                .lods
                .into_iter()
                .map(|l| {
                    let mut m = Mesh::new(
                        PrimitiveTopology::TriangleList,
                        RenderAssetUsages::RENDER_WORLD,
                    );
                    let col: Vec<[f32; 4]> = l.sway.iter().map(|s| [1.0, 1.0, 1.0, *s]).collect();
                    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, l.pos);
                    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, l.nrm);
                    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, l.uv);
                    m.insert_attribute(Mesh::ATTRIBUTE_TANGENT, l.tan);
                    // the foliage shader bends vertex colour alpha (sway weight); rocks and wood stay still
                    if sway {
                        m.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
                    }
                    m.insert_indices(Indices::U16(l.idx));
                    m
                })
                .collect();
            (lods, v.height, v.width, v.min_y)
        })
        .collect();
    Ok((variants, raw.roughness, sway, raw.cards))
}

/// The block-compressed formats written by tools/process_photoscan.py, by KTX2 `vkFormat`.
fn bc_format(vk: u32) -> Option<(TextureFormat, usize)> {
    Some(match vk {
        131 | 133 => (TextureFormat::Bc1RgbaUnorm, 8),
        132 | 134 => (TextureFormat::Bc1RgbaUnormSrgb, 8),
        137 => (TextureFormat::Bc3RgbaUnorm, 16),
        138 => (TextureFormat::Bc3RgbaUnormSrgb, 16),
        141 => (TextureFormat::Bc5RgUnorm, 16),
        _ => return None,
    })
}

/// A KTX2 texture (BC1 / BC3 / BC5 blocks, every mip level, no supercompression). Uploaded as it is
/// when the GPU samples BC formats (desktop GPUs, Apple silicon), otherwise decoded to RGBA8 / RG8.
/// Atlases clamp; tiling textures repeat.
pub(crate) fn ktx2_image(path: &Path, bc: bool, repeat: bool) -> Result<Image, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let bad = |what: &str| format!("{}: {what}", path.display());
    if b.len() < 80 || b[..12] != *b"\xabKTX 20\xbb\r\n\x1a\n" {
        return Err(bad("not a KTX2 file"));
    }
    let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) as usize;
    let (vk, w, h, levels) = (u32_at(12), u32_at(20), u32_at(24), u32_at(40).max(1));
    let Some((format, block)) = bc_format(vk) else {
        return Err(bad("unsupported texture format"));
    };
    if u32_at(44) != 0 || w == 0 || h == 0 || b.len() < 80 + levels as usize * 24 {
        return Err(bad("unsupported KTX2 layout"));
    }
    let mut data = vec![];
    for l in 0..levels {
        let (lw, lh) = ((w >> l).max(1), (h >> l).max(1));
        let (off, len) = (u64_at(80 + l as usize * 24), u64_at(88 + l as usize * 24));
        let blocks = b
            .get(off..off + len)
            .filter(|s| s.len() == lw.div_ceil(4) as usize * lh.div_ceil(4) as usize * block)
            .ok_or_else(|| bad("bad mip level"))?;
        if bc {
            data.extend_from_slice(blocks);
        } else {
            decode_bc(blocks, lw as usize, lh as usize, format, &mut data);
        }
    }
    let format = match format {
        _ if bc => format,
        TextureFormat::Bc5RgUnorm => TextureFormat::Rg8Unorm,
        TextureFormat::Bc1RgbaUnorm | TextureFormat::Bc3RgbaUnorm => TextureFormat::Rgba8Unorm,
        _ => TextureFormat::Rgba8UnormSrgb,
    };
    let mut img = Image::new_uninit(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.data = Some(data);
    img.texture_descriptor.mip_level_count = levels;
    let mode = if repeat {
        ImageAddressMode::Repeat
    } else {
        ImageAddressMode::ClampToEdge
    };
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: mode,
        address_mode_v: mode,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    Ok(img)
}

/// CPU fallback: one mip level of BC1 / BC3 (to RGBA8) or BC5 (to RG8) blocks.
pub(crate) fn decode_bc(
    blocks: &[u8],
    w: usize,
    h: usize,
    format: TextureFormat,
    out: &mut Vec<u8>,
) {
    let (texel, block) = match format {
        TextureFormat::Bc5RgUnorm => (2, 16),
        TextureFormat::Bc1RgbaUnorm | TextureFormat::Bc1RgbaUnormSrgb => (4, 8),
        _ => (4, 16),
    };
    let start = out.len();
    out.resize(start + w * h * texel, 0);
    let px = &mut out[start..];
    for (i, b) in blocks.chunks_exact(block).enumerate() {
        let (bx, by) = (i % w.div_ceil(4) * 4, i / w.div_ceil(4) * 4);
        let mut texels = [[0u8, 0, 0, 255]; 16];
        match texel {
            2 => {
                let (r, g) = (bc4(&b[..8]), bc4(&b[8..]));
                for k in 0..16 {
                    texels[k] = [r[k], g[k], 0, 0];
                }
            }
            _ if block == 8 => texels = bc1(b, true),
            _ => {
                let a = bc4(&b[..8]);
                texels = bc1(&b[8..], false);
                for k in 0..16 {
                    texels[k][3] = a[k];
                }
            }
        }
        for k in 0..16 {
            let (x, y) = (bx + k % 4, by + k / 4);
            if x < w && y < h {
                px[(y * w + x) * texel..][..texel].copy_from_slice(&texels[k][..texel]);
            }
        }
    }
}

fn bc1(b: &[u8], punch_through: bool) -> [[u8; 4]; 16] {
    let (c0, c1) = (
        u16::from_le_bytes([b[0], b[1]]),
        u16::from_le_bytes([b[2], b[3]]),
    );
    let rgb = |c: u16| {
        [
            ((c >> 11) & 31) as u32 * 255 / 31,
            ((c >> 5) & 63) as u32 * 255 / 63,
            (c & 31) as u32 * 255 / 31,
        ]
    };
    let (p0, p1) = (rgb(c0), rgb(c1));
    let mix = |a: u32, b: u32, d: u32| {
        [
            (p0[0] * a + p1[0] * b) / d,
            (p0[1] * a + p1[1] * b) / d,
            (p0[2] * a + p1[2] * b) / d,
        ]
    };
    let four = c0 > c1 || !punch_through;
    let pal = if four {
        [p0, p1, mix(2, 1, 3), mix(1, 2, 3)]
    } else {
        [p0, p1, mix(1, 1, 2), [0, 0, 0]]
    };
    let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    std::array::from_fn(|k| {
        let i = (bits >> (2 * k)) as usize & 3;
        let c = pal[i];
        [
            c[0] as u8,
            c[1] as u8,
            c[2] as u8,
            if !four && i == 3 { 0 } else { 255 },
        ]
    })
}

fn bc4(b: &[u8]) -> [u8; 16] {
    let (a0, a1) = (b[0] as u32, b[1] as u32);
    let pal: [u32; 8] = if a0 > a1 {
        std::array::from_fn(|i| match i {
            0 => a0,
            1 => a1,
            _ => (a0 * (8 - i as u32) + a1 * (i as u32 - 1)) / 7,
        })
    } else {
        std::array::from_fn(|i| match i {
            0 => a0,
            1 => a1,
            6 => 0,
            7 => 255,
            _ => (a0 * (6 - i as u32) + a1 * (i as u32 - 1)) / 5,
        })
    };
    let bits = u64::from_le_bytes([b[2], b[3], b[4], b[5], b[6], b[7], 0, 0]);
    std::array::from_fn(|k| pal[(bits >> (3 * k)) as usize & 7] as u8)
}

/// A photoscanned (or photoscan-dressed) kind as plain CPU data for the engine exporters
/// (`crate::export`): uploaded meshes and textures keep no main-world copy.
pub struct PropExport {
    /// Variants in library order, each its levels of detail.
    pub variants: Vec<Vec<Mesh>>,
    /// Base level of the albedo (RGBA8) and normal map (RG8 = tangent XY) textures.
    pub albedo: (u32, u32, Vec<u8>),
    pub normal: Option<(u32, u32, Vec<u8>)>,
    /// A tiling texture on a procedural shape (repeat) rather than a scan atlas (clamp).
    pub tiling: bool,
}

/// Reads the given photoscanned kinds from disk for export (BC textures decoded on the CPU).
pub fn export_props(kinds: &[AssetKind]) -> Vec<PropExport> {
    let dir = dir();
    let base = |img: Image, texel: usize| {
        let (w, h) = (img.width(), img.height());
        let mut data = img.data.unwrap_or_default();
        data.truncate(w as usize * h as usize * texel);
        (w, h, data)
    };
    let mut out = vec![];
    for p in PROPS.iter().filter(|p| kinds.contains(&p.kind)) {
        let read = || -> Result<PropExport, String> {
            let (variants, _, _, cards) = read_lods(&dir.join(format!("{}.lods.bin", p.file)))?;
            let albedo = base(
                ktx2_image(&dir.join(format!("{}_albedo.ktx2", p.file)), false, false)?,
                4,
            );
            let normal = if cards {
                None
            } else {
                Some(base(
                    ktx2_image(&dir.join(format!("{}_normal.ktx2", p.file)), false, false)?,
                    2,
                ))
            };
            Ok(PropExport {
                variants: variants.into_iter().map(|v| v.0).collect(),
                albedo,
                normal,
                tiling: false,
            })
        };
        match read() {
            Ok(e) => out.push(e),
            Err(e) => warn!("photoscan export {}: {e}", p.file),
        }
    }
    let dressed: Vec<&Dressed> = DRESSED.iter().filter(|d| kinds.contains(&d.kind)).collect();
    if !dressed.is_empty() {
        let tile = (|| {
            Ok::<_, String>((
                ktx2_image(&dir.join(format!("{TILE_FILE}_albedo.ktx2")), false, true)?,
                ktx2_image(&dir.join(format!("{TILE_FILE}_normal.ktx2")), false, true)?,
            ))
        })();
        match tile {
            Ok((a, n)) => {
                let (albedo, normal) = (base(a, 4), base(n, 2));
                for d in dressed {
                    let variants = dressed_variants(d).into_iter().map(|v| v.0).collect();
                    out.push(PropExport {
                        variants,
                        albedo: albedo.clone(),
                        normal: Some(normal.clone()),
                        tiling: true,
                    });
                }
            }
            Err(e) => warn!("photoscan export {TILE_FILE}: {e}"),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped files parse, and the CPU fallback decodes every mip level to the right size.
    #[test]
    fn shipped_props_load() {
        let dir = dir();
        for p in &PROPS {
            let l = load(&dir, p.file, true).unwrap_or_else(|e| panic!("{e}"));
            assert!(
                l.variants
                    .iter()
                    .all(|(lods, h, w, _)| !lods.is_empty() && *h > 0.0 && *w > 0.0),
                "{}",
                p.file
            );
            for kind in ["albedo", "normal"] {
                if kind == "normal" && l.cards {
                    continue;
                }
                let img =
                    ktx2_image(&dir.join(format!("{}_{kind}.ktx2", p.file)), false, false).unwrap();
                let texel = if kind == "normal" { 2 } else { 4 };
                let (w, h) = (img.width() as usize, img.height() as usize);
                let expected: usize = (0..img.texture_descriptor.mip_level_count)
                    .map(|l| (w >> l).max(1) * (h >> l).max(1) * texel)
                    .sum();
                assert_eq!(
                    img.data.as_ref().map(Vec::len),
                    Some(expected),
                    "{} {kind}",
                    p.file
                );
            }
        }
    }

    /// The tall formations (tools/process_formations.py) stand at their real size, sunk into the
    /// ground, each level of detail much lighter than the one before.
    #[test]
    fn formations_load() {
        let dir = dir();
        for (kind, lo, hi) in [
            (AssetKind::SeaStack, 20.0, 45.0),
            (AssetKind::Hoodoo, 7.0, 16.0),
            (AssetKind::RockSpire, 10.0, 22.0),
        ] {
            let p = PROPS.iter().find(|p| p.kind == kind).unwrap();
            let l = load(&dir, p.file, true).unwrap_or_else(|e| panic!("{e}"));
            assert!(
                l.variants.len() >= 3,
                "{kind:?}: {} variants",
                l.variants.len()
            );
            for (lods, h, _, min_y) in &l.variants {
                assert!(*h > lo && *h < hi, "{kind:?}: {h} m tall");
                assert!(*min_y < -0.8, "{kind:?}: not sunk into the ground");
                let tris: Vec<usize> = lods
                    .iter()
                    .map(|m| m.indices().map_or(0, |i| i.len() / 3))
                    .collect();
                assert!(
                    tris.len() == 3 && tris.windows(2).all(|w| w[1] * 2 < w[0]),
                    "{kind:?}: LOD triangles {tris:?}"
                );
            }
        }
    }

    #[test]
    fn bc_blocks_decode() {
        // BC1: white / black endpoints, texel k uses index k % 4 (white, black, 2/3 white, 1/3 white)
        let px = bc1(&[0xff, 0xff, 0, 0, 0xe4, 0xe4, 0xe4, 0xe4], true);
        assert_eq!(px[0], [255, 255, 255, 255]);
        assert_eq!(px[1], [0, 0, 0, 255]);
        assert_eq!(px[2], [170, 170, 170, 255]);
        // BC4: 8-value mode, all texels code 2 = (6 * 200 + 100) / 7
        let a = bc4(&[200, 100, 146, 36, 73, 146, 36, 73]);
        assert!(a.iter().all(|&v| v as u32 == (6 * 200 + 100) / 7));
    }
}
