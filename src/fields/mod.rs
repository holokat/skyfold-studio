//! Farm fields: draw a field's outline (like a lake or a set piece) and it fills with a crop in
//! rows — wheat, corn, sunflowers, a vineyard, pumpkins… — or let the editor lay out patchworks of
//! fields on the flat lowland round the world (`auto`).
//!
//! A field is its outline, its crop and its row direction (along the field's long side unless
//! set). Its plants are procedural (`plants`), drawn in 8 m tiles of rows turned to the field and
//! clipped to its outline, seated on the ground and bending in the same gusting wind as the grass
//! and trees (`render`, `shaders/crops.wesl`); far away the field reads as its painted ground
//! (soil, stubble). Planting a field paints its ground, clears wild nature out of it and is one
//! undo step. Fields are saved with the world. MCP: `fields_list`, `fields_add`, `fields_remove`,
//! `fields_auto` (`mcp`).

pub mod assets;
pub mod auto;
pub(crate) mod mcp;
pub mod plants;
pub mod render;
pub mod ui;

use std::sync::Arc;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::terrain::Terrain;
use crate::terrain::layers::{LAYER_COUNT, Layer};

/// History key of field edits.
pub const UNDO_KEY: &str = "fields";

/// The crops.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Crop {
    Wheat,
    Barley,
    Rye,
    Rice,
    Corn,
    Sunflowers,
    Lavender,
    Tulips,
    Hay,
    Pumpkins,
    Cabbages,
    Potatoes,
    Carrots,
    Strawberries,
    Tomatoes,
    Vineyard,
    Hops,
    Cotton,
    Tea,
    Plowed,
}

impl Crop {
    pub const ALL: [Crop; 20] = [
        Crop::Wheat,
        Crop::Barley,
        Crop::Rye,
        Crop::Corn,
        Crop::Sunflowers,
        Crop::Hay,
        Crop::Potatoes,
        Crop::Cabbages,
        Crop::Carrots,
        Crop::Pumpkins,
        Crop::Strawberries,
        Crop::Tomatoes,
        Crop::Lavender,
        Crop::Tulips,
        Crop::Vineyard,
        Crop::Hops,
        Crop::Cotton,
        Crop::Rice,
        Crop::Tea,
        Crop::Plowed,
    ];

    pub fn id(self) -> &'static str {
        use Crop::*;
        match self {
            Wheat => "wheat",
            Barley => "barley",
            Rye => "rye",
            Rice => "rice",
            Corn => "corn",
            Sunflowers => "sunflowers",
            Lavender => "lavender",
            Tulips => "tulips",
            Hay => "hay",
            Pumpkins => "pumpkins",
            Cabbages => "cabbages",
            Potatoes => "potatoes",
            Carrots => "carrots",
            Strawberries => "strawberries",
            Tomatoes => "tomatoes",
            Vineyard => "vineyard",
            Hops => "hops",
            Cotton => "cotton",
            Tea => "tea",
            Plowed => "plowed",
        }
    }

    pub fn name(self) -> &'static str {
        use Crop::*;
        match self {
            Wheat => "Wheat",
            Barley => "Barley",
            Rye => "Rye",
            Rice => "Rice paddy",
            Corn => "Corn",
            Sunflowers => "Sunflowers",
            Lavender => "Lavender",
            Tulips => "Tulips",
            Hay => "Hay & bales",
            Pumpkins => "Pumpkins",
            Cabbages => "Cabbages",
            Potatoes => "Potatoes",
            Carrots => "Carrots",
            Strawberries => "Strawberries",
            Tomatoes => "Tomatoes",
            Vineyard => "Vineyard",
            Hops => "Hop garden",
            Cotton => "Cotton",
            Tea => "Tea rows",
            Plowed => "Plowed soil",
        }
    }

    pub fn from_id(s: &str) -> Option<Crop> {
        let s = s.trim().to_lowercase().replace([' ', '-'], "_");
        Crop::ALL.into_iter().find(|c| {
            c.id() == s
                || c.name().to_lowercase().replace([' ', '-'], "_") == s
                || format!("{}s", c.id()) == s
                || c.id().trim_end_matches('s') == s
        })
    }

    /// The ground between the plants (and what the field looks like from afar).
    pub fn ground(self) -> Layer {
        use Crop::*;
        match self {
            Wheat | Barley | Rye => Layer::DryGrass,
            // a mown meadow: green regrowth under the windrows
            Hay => Layer::Grass,
            Rice => Layer::Mud,
            _ => Layer::Dirt,
        }
    }

    /// How far the plants are drawn (m) at each level of detail: tall crops further.
    pub fn ranges(self) -> [f32; 3] {
        use Crop::*;
        match self {
            Corn | Sunflowers | Hops | Vineyard => [45.0, 120.0, 260.0],
            Wheat | Barley | Rye | Rice | Lavender | Tea | Cotton | Tomatoes => [30.0, 80.0, 170.0],
            Plowed => [40.0, 100.0, 200.0],
            _ => [25.0, 60.0, 130.0],
        }
    }

    /// Where its far canopy starts rising (m), given its draw distances: low crops whose plants
    /// turn sub-pixel early get it soon after their close range, tall ones only past half way.
    pub fn canopy_from(self, ranges: [f32; 3]) -> f32 {
        use Crop::*;
        match self {
            Corn | Sunflowers | Hops | Vineyard | Tea | Plowed => ranges[2] * 0.55,
            // tulip heads stay apart, the soil showing between them, until well into their mid
            // range: a carpet nearer than that is brighter than the plants in front of it
            Tulips => ranges[1] * 0.75,
            // thin stalks and small leaves turn sub-pixel early: the canopy carries them from
            // just past their close range (the plants' tops stand out of it)
            _ => ranges[0],
        }
    }

    /// A swatch colour for the picker (sRGB).
    pub fn swatch(self) -> [u8; 3] {
        use Crop::*;
        match self {
            Wheat => [222, 182, 92],
            Barley => [214, 196, 128],
            Rye => [190, 172, 116],
            Rice => [96, 160, 60],
            Corn => [92, 140, 48],
            Sunflowers => [246, 192, 30],
            Lavender => [138, 108, 186],
            Tulips => [222, 60, 90],
            Hay => [206, 176, 104],
            Pumpkins => [232, 116, 24],
            Cabbages => [140, 184, 130],
            Potatoes => [76, 120, 50],
            Carrots => [236, 128, 40],
            Strawberries => [210, 40, 50],
            Tomatoes => [214, 60, 40],
            Vineyard => [100, 50, 110],
            Hops => [150, 196, 96],
            Cotton => [240, 240, 232],
            Tea => [60, 124, 52],
            Plowed => [110, 82, 58],
        }
    }
}

/// A planted field.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Field {
    pub id: u64,
    pub crop: Crop,
    /// the outline [x, z] (world)
    pub outline: Vec<[f32; 2]>,
    /// the rows' direction (radians, as `Quat::from_rotation_y`: 0 = along x)
    pub rows: f32,
    /// tulips only: all one colour (`plants::TULIP_COLOUR_NAMES` index); none = bands of colours
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<u8>,
    /// outlines inside it left unplanted (for other fields: a disc, a stripe, a letter)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holes: Vec<Vec<[f32; 2]>>,
}

impl Field {
    pub fn poly(&self) -> Vec<Vec2> {
        self.outline.iter().map(|p| Vec2::from_array(*p)).collect()
    }
    pub fn hole_polys(&self) -> Vec<Vec<Vec2>> {
        self.holes
            .iter()
            .map(|h| h.iter().map(|p| Vec2::from_array(*p)).collect())
            .collect()
    }
    /// Signed distance to the planted ground: < 0 inside the outline and outside every hole.
    pub fn sdf(&self, p: Vec2) -> f32 {
        sdf_with_holes(&self.poly(), &self.hole_polys(), p)
    }
    pub fn area(&self) -> f32 {
        poly_area(&self.poly()) - self.hole_polys().iter().map(|h| poly_area(h)).sum::<f32>()
    }
    pub fn centre(&self) -> Vec2 {
        let p = self.poly();
        p.iter().copied().sum::<Vec2>() / p.len().max(1) as f32
    }
    pub fn contains(&self, p: Vec2) -> bool {
        self.sdf(p) < 0.0
    }
}

/// Signed distance to the inside of `poly` less its `holes`.
pub fn sdf_with_holes(poly: &[Vec2], holes: &[Vec<Vec2>], p: Vec2) -> f32 {
    holes.iter().fold(crate::setpieces::poly_sdf(poly, p), |d, h| {
        d.max(-crate::setpieces::poly_sdf(h, p))
    })
}

pub fn poly_area(poly: &[Vec2]) -> f32 {
    let n = poly.len();
    (0..n)
        .map(|i| poly[i].perp_dot(poly[(i + 1) % n]))
        .sum::<f32>()
        .abs()
        * 0.5
}

/// The direction of a shape's long axis (radians, as `Quat::from_rotation_y`).
pub fn long_axis(poly: &[Vec2]) -> f32 {
    let c = poly.iter().copied().sum::<Vec2>() / poly.len().max(1) as f32;
    let (mut xx, mut xz, mut zz) = (0.0f32, 0.0f32, 0.0f32);
    for p in poly {
        let d = *p - c;
        xx += d.x * d.x;
        xz += d.x * d.y;
        zz += d.y * d.y;
    }
    // (rotation_y turns +x towards -z: the angle of (x, -z))
    let a = 0.5 * (2.0 * xz).atan2(xx - zz);
    -a
}

/// The fields of the world.
#[derive(Resource, Default, Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Fields {
    pub list: Vec<Field>,
    #[serde(default)]
    pub next_id: u64,
    /// bumped on every change (the renderer rebuilds what changed)
    #[serde(skip)]
    pub revision: u64,
    /// File › New asked for fields: laid out once the world made after this terrain generation
    /// has settled (`auto_after_new_world`)
    #[serde(skip)]
    pub auto_pending: Option<u64>,
}

impl Fields {
    pub fn get(&self, id: u64) -> Option<&Field> {
        self.list.iter().find(|f| f.id == id)
    }
    pub fn at(&self, p: Vec2) -> Option<&Field> {
        self.list.iter().rev().find(|f| f.contains(p))
    }
    fn new_id(&mut self) -> u64 {
        self.next_id = self
            .next_id
            .max(self.list.iter().map(|f| f.id).max().unwrap_or(0))
            + 1;
        self.next_id
    }
    pub fn changed(&mut self) {
        self.revision += 1;
    }
}

/// An undo snapshot of the fields.
#[derive(Clone)]
struct Snapshot(Vec<Field>, u64);

/// What planting needs from the world.
pub struct PlantWorld<'a> {
    pub fields: &'a mut Fields,
    pub terrain: &'a mut Terrain,
    pub history: &'a mut crate::history::History,
    pub nature: &'a mut crate::nature::NatureField,
    pub water: &'a crate::water::WaterBodies,
}

/// Plants a field into the open undo step (the caller begins and commits it): its ground
/// painted, wild nature cleared out of it. `rows`: the rows' direction, default along its long
/// side. Returns its id.
pub fn add(
    w: &mut PlantWorld,
    crop: Crop,
    outline: &[Vec2],
    rows: Option<f32>,
    label: &str,
) -> Result<u64, String> {
    add_with_holes(w, crop, outline, &[], rows, label)
}

/// `add`, leaving `holes` (outlines inside it) unplanted.
pub fn add_with_holes(
    w: &mut PlantWorld,
    crop: Crop,
    outline: &[Vec2],
    holes: &[Vec<Vec2>],
    rows: Option<f32>,
    label: &str,
) -> Result<u64, String> {
    if holes
        .iter()
        .any(|h| h.len() < 3 || h.iter().any(|p| crate::setpieces::poly_sdf(outline, *p) > 0.0))
    {
        return Err("every hole needs at least 3 corners, all inside the field".into());
    }
    if outline.len() < 3 {
        return Err("a field needs at least 3 corners".into());
    }
    let area = poly_area(outline) - holes.iter().map(|h| poly_area(h)).sum::<f32>();
    if area < 30.0 {
        return Err(format!(
            "that's only {area:.0} m² — draw a field of at least 30 m²"
        ));
    }
    if area > 600_000.0 {
        return Err(format!(
            "that's {:.0} ha — fields are at most 60 ha (draw several)",
            area / 10_000.0
        ));
    }
    if outline.iter().any(|p| !w.terrain.in_bounds(p.x, p.y)) {
        return Err("part of that field is outside the world".into());
    }
    // not in a lake, river or the sea
    let (lo, hi) = bounds(outline);
    let step = ((hi - lo).max_element() / 12.0).max(2.0);
    let (mut inside, mut wet) = (0, 0);
    let mut z = lo.y + step * 0.5;
    while z < hi.y {
        let mut x = lo.x + step * 0.5;
        while x < hi.x {
            let p = Vec2::new(x, z);
            if sdf_with_holes(outline, holes, p) < 0.0 {
                inside += 1;
                let h = w.terrain.height_at(x, z);
                if h < w.terrain.sea_level + 0.2 || w.water.is_underwater(Vec3::new(x, h, z), 0.2) {
                    wet += 1;
                }
            }
            x += step;
        }
        z += step;
    }
    if inside > 0 && wet * 3 > inside {
        return Err("that's mostly water — draw the field on dry land".into());
    }
    let before = Snapshot(w.fields.list.clone(), w.fields.next_id);
    w.history.begin(label);
    w.history.set_label(label);
    let id = w.fields.new_id();
    let f = Field {
        id,
        crop,
        outline: outline.iter().map(|p| p.to_array()).collect(),
        rows: rows.unwrap_or_else(|| long_axis(outline)),
        colour: None,
        holes: holes
            .iter()
            .map(|h| h.iter().map(|p| p.to_array()).collect())
            .collect(),
    };
    paint_ground(w.terrain, w.history, &f);
    let poly: Vec<Vec2> = outline.to_vec();
    let holes = holes.to_vec();
    let (lo, hi) = bounds(&poly);
    crate::nature::paint_region(
        w.nature,
        w.history,
        label,
        lo - 3.0,
        hi + 3.0,
        move |q| sdf_with_holes(&poly, &holes, q) < 2.5,
        &crate::nature::field::Target::All,
        crate::nature::AreaOp::Keep(0.0),
    );
    w.fields.list.push(f);
    w.fields.changed();
    record(w.history, label, before, w.fields);
    Ok(id)
}

/// Removes a field (its ground stays as painted: paint over it, or undo).
pub fn remove(
    fields: &mut Fields,
    history: &mut crate::history::History,
    id: u64,
) -> Result<Field, String> {
    let i = fields
        .list
        .iter()
        .position(|f| f.id == id)
        .ok_or_else(|| format!("no field {id}"))?;
    let before = Snapshot(fields.list.clone(), fields.next_id);
    let f = fields.list.remove(i);
    fields.changed();
    // (its own undo step, or part of the one open)
    record(history, "Remove field", before, fields);
    Ok(f)
}

/// Changes a field's crop or row direction.
pub fn edit(
    fields: &mut Fields,
    history: &mut crate::history::History,
    id: u64,
    crop: Option<Crop>,
    rows: Option<f32>,
) -> Result<(), String> {
    let before = Snapshot(fields.list.clone(), fields.next_id);
    let f = fields
        .list
        .iter_mut()
        .find(|f| f.id == id)
        .ok_or_else(|| format!("no field {id}"))?;
    if let Some(c) = crop {
        f.crop = c;
        if c != Crop::Tulips {
            f.colour = None;
        }
    }
    if let Some(r) = rows {
        f.rows = r;
    }
    fields.changed();
    record(history, "Change field", before, fields);
    Ok(())
}

/// The tulip colour index of `name` (red, yellow, pink, white, purple, orange).
pub fn tulip_colour(name: &str) -> Option<u8> {
    let n = name.trim().to_lowercase();
    plants::TULIP_COLOUR_NAMES
        .iter()
        .position(|c| *c == n)
        .map(|i| i as u8)
}

/// Sets a tulip field's colour (`None`: bands of every colour).
pub fn set_colour(
    fields: &mut Fields,
    history: &mut crate::history::History,
    id: u64,
    colour: Option<u8>,
) -> Result<(), String> {
    let before = Snapshot(fields.list.clone(), fields.next_id);
    let f = fields
        .list
        .iter_mut()
        .find(|f| f.id == id)
        .ok_or_else(|| format!("no field {id}"))?;
    if colour.is_some() && f.crop != Crop::Tulips {
        return Err(format!(
            "only tulip fields have a colour (field {id} is {})",
            f.crop.id()
        ));
    }
    if f.colour == colour {
        return Ok(());
    }
    f.colour = colour;
    fields.changed();
    record(history, "Change field", before, fields);
    Ok(())
}

fn record(history: &mut crate::history::History, label: &str, before: Snapshot, fields: &Fields) {
    let after = Snapshot(fields.list.clone(), fields.next_id);
    history.push_ext(
        label.to_string(),
        UNDO_KEY,
        Arc::new(before),
        Arc::new(after),
    );
}

pub fn bounds(poly: &[Vec2]) -> (Vec2, Vec2) {
    poly.iter().fold(
        (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
        |(lo, hi), p| (lo.min(*p), hi.max(*p)),
    )
}

/// Paints the field's ground layer inside its outline (soft at the edge).
fn paint_ground(t: &mut Terrain, history: &mut crate::history::History, f: &Field) {
    let poly = f.poly();
    let (lo, hi) = bounds(&poly);
    let c = (lo + hi) * 0.5;
    let Some(rect) = t.rect_for_circle(c, (hi - lo).length() * 0.5 + 3.0) else {
        return;
    };
    history.touch_terrain(t, rect);
    let layer = f.crop.ground() as usize;
    for j in rect.z0..=rect.z1 {
        for i in rect.x0..=rect.x1 {
            let w = t.grid_to_world(i, j);
            let d = sdf_with_holes(&poly, &f.hole_polys(), w);
            let k = ((1.5 - d) / 3.0).clamp(0.0, 1.0);
            if k <= 0.004 {
                continue;
            }
            let idx = t.idx(i, j);
            let s = &mut t.splat[idx];
            for (l, v) in s.iter_mut().enumerate().take(LAYER_COUNT) {
                let target = if l == layer { 255.0 } else { 0.0 };
                *v = (*v as f32 + (target - *v as f32) * k)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
            let sum: u32 = s.iter().map(|v| *v as u32).sum();
            if sum > 255 {
                let f = 255.0 / sum as f32;
                s.iter_mut()
                    .for_each(|v| *v = (*v as f32 * f).floor() as u8);
            }
        }
    }
    t.mark_splat_dirty(rect);
}

/// Restores fields after undo / redo.
fn apply_undo(mut history: ResMut<crate::history::History>, mut fields: ResMut<Fields>) {
    if !history.ext_apply.iter().any(|(k, _)| *k == UNDO_KEY) {
        return;
    }
    for (key, snap) in std::mem::take(&mut history.ext_apply) {
        if key == UNDO_KEY {
            if let Some(s) = snap.downcast_ref::<Snapshot>() {
                fields.list = s.0.clone();
                fields.next_id = s.1;
                fields.changed();
            }
        } else {
            history.ext_apply.push((key, snap));
        }
    }
}

/// Fields for a new world (File › New › Farm fields), once its terrain has settled.
fn auto_after_new_world(mut d: crate::io::WorldData, mut stable: Local<(u64, u32)>) {
    let Some(g0) = d.fields.auto_pending else {
        return;
    };
    let g = d.terrain.generation;
    if g == g0 || d.lib.is_none() {
        return;
    }
    if stable.0 != g {
        *stable = (g, 0);
        return;
    }
    stable.1 += 1;
    if stable.1 < 45 {
        return;
    }
    d.fields.auto_pending = None;
    let opts = auto::Auto {
        clusters: ((d.terrain.size() / 900.0).powi(2) as usize).clamp(3, 24),
        area: None,
        crops: vec![],
        seed: d.meta.seed,
    };
    d.history.begin("Plant fields");
    let crate::io::WorldData {
        fields,
        terrain,
        history,
        nature,
        water,
        store,
        ..
    } = &mut d;
    let r = auto::auto(
        &mut PlantWorld {
            fields,
            terrain,
            history,
            nature,
            water,
        },
        store,
        &opts,
    );
    d.history.commit(&d.store);
    if let Ok(ids) = r {
        info!("FIELDS planted {} for the new world", ids.len());
        d.meta.dirty = true;
    }
}

pub struct FieldsPlugin;

impl Plugin for FieldsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Fields>()
            .init_resource::<ui::FieldTool>()
            .add_systems(
                Update,
                (apply_undo, ui::cursor, ui::sync_mode, auto_after_new_world),
            )
            .add_systems(
                Update,
                ui::draw_field.in_set(crate::tools::ToolSystems::Apply),
            )
            .init_resource::<assets::CropAssets>()
            .add_systems(Update, assets::watch)
            .add_plugins(render::CropRenderPlugin);
        mcp::register(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_have_unique_ids_and_parse_back() {
        let mut ids = std::collections::HashSet::new();
        for c in Crop::ALL {
            assert!(ids.insert(c.id()));
            assert_eq!(Crop::from_id(c.id()), Some(c));
            assert_eq!(Crop::from_id(c.name()), Some(c), "{}", c.name());
        }
        assert_eq!(Crop::from_id("Pumpkin"), Some(Crop::Pumpkins));
        assert_eq!(Crop::from_id("grapes"), None);
    }

    #[test]
    fn rows_follow_the_long_side() {
        // a field long in x: rows along x (angle 0); long in z: rows along z (±90°)
        let wide = [
            Vec2::new(0.0, 0.0),
            Vec2::new(100.0, 0.0),
            Vec2::new(100.0, 30.0),
            Vec2::new(0.0, 30.0),
        ];
        assert!(long_axis(&wide).abs() < 1e-3);
        let tall = [
            Vec2::new(0.0, 0.0),
            Vec2::new(30.0, 0.0),
            Vec2::new(30.0, 100.0),
            Vec2::new(0.0, 100.0),
        ];
        let a = long_axis(&tall);
        let d = (Quat::from_rotation_y(a) * Vec3::X).xz();
        assert!(d.x.abs() < 1e-3 && d.y.abs() > 0.999, "{d:?}");
        assert!((poly_area(&wide) - 3000.0).abs() < 1e-2);
    }
}
