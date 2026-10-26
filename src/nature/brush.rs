//! The Nature tab brush on procedural nature: it paints density, never objects. With an
//! ecosystem preset selected it paints that ecosystem; with species selected, more (or fewer) of
//! them; with nothing selected, the whole category. Grow / Thin / Erase; ⌥ inverts. A stroke is
//! one undo step. Also: picking, deleting and pinning single procedural instances.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;

use super::field::{Chunk, NatureField, NatureUndo, PaintMode, Target};
use super::place::Instance;
use super::render::Near;
use crate::editor::{Editor, ObjectTool, ScatterMode, Tab};
use crate::history::History;
use crate::objects::{Library, ObjectStore, Placed, ScatterSettings};
use crate::tools::Stroke;

/// History key of nature paint steps.
pub const UNDO_KEY: &str = "nature";

#[derive(Resource, Default)]
pub struct NatureBrush {
    /// chunks as they were before the current stroke
    before: HashMap<(i32, i32), Option<Chunk>>,
    /// the procedural instance picked in the Objects tab (Pin / Delete act on it)
    pub selected: Option<Instance>,
}

/// What the Nature tab's selection paints.
pub fn target(settings: &ScatterSettings, lib: &Library, rules: &super::Rules) -> Target {
    if let Some(i) = settings.preset {
        return Target::Preset(i as u8);
    }
    let kinds: Vec<_> = settings
        .selected
        .iter()
        .filter_map(|id| lib.get(id))
        .filter(|e| e.category == settings.category)
        .filter_map(|e| e.kind)
        .collect();
    if !kinds.is_empty() {
        return Target::Species(kinds);
    }
    let _ = rules;
    super::rules::cat_index(settings.category)
        .map(Target::Category)
        .unwrap_or(Target::All)
}

/// Records the chunks a dab will touch (first touch per stroke) for the undo step.
pub fn remember(
    before: &mut HashMap<(i32, i32), Option<Chunk>>,
    field: &NatureField,
    center: Vec2,
    radius: f32,
) {
    for k in NatureField::chunks_in(center, radius) {
        before
            .entry(k)
            .or_insert_with(|| field.chunks.get(&k).cloned());
    }
}

/// Finishes a paint step: pushes the undo entry for the chunks touched.
pub fn commit(
    before: &mut HashMap<(i32, i32), Option<Chunk>>,
    field: &NatureField,
    history: &mut History,
    label: &str,
) {
    if before.is_empty() {
        return;
    }
    let keys: Vec<(i32, i32)> = before.keys().copied().collect();
    let b = NatureUndo {
        chunks: std::mem::take(before).into_iter().collect(),
        ..default()
    };
    let a = field.snapshot(&keys);
    // (the stroke's open step takes the paint, under this name)
    history.set_label(label);
    history.push_ext(label, UNDO_KEY, Arc::new(b), Arc::new(a));
}

#[allow(clippy::too_many_arguments)]
pub fn paint_tool(
    stroke: Res<Stroke>,
    ed: Res<Editor>,
    settings: Res<ScatterSettings>,
    lib: Option<Res<Library>>,
    rules: Option<Res<super::Rules>>,
    mut field: ResMut<NatureField>,
    mut brush: ResMut<NatureBrush>,
    mut history: ResMut<History>,
    mut meta: ResMut<crate::world::WorldMeta>,
    time: Res<Time>,
    terrain: Res<crate::terrain::Terrain>,
    mut selection: Option<ResMut<crate::prompt::selection::Selection>>,
    mut store: ResMut<ObjectStore>,
    fields: Res<crate::fields::ui::FieldTool>,
) {
    // (Nature › Fields draws fields instead)
    if ed.tab != Tab::Nature || fields.on {
        return;
    }
    let (Some(lib), Some(rules)) = (lib, rules) else {
        return;
    };
    if ed.scatter_mode == ScatterMode::Place {
        // one placed object per click: a picked species (or any of the category)
        if stroke.just_started
            && let Some(hit) = stroke.hit
        {
            let picks: Vec<&crate::objects::LibraryEntry> = if settings.selected.is_empty() {
                lib.entries
                    .iter()
                    .filter(|e| e.category == settings.category)
                    .collect()
            } else {
                settings
                    .selected
                    .iter()
                    .filter_map(|id| lib.get(id))
                    .collect()
            };
            if let Some(e) = picks
                .get(fastrand::usize(..picks.len().max(1)))
                .filter(|_| !picks.is_empty())
            {
                let id = store.new_id();
                history.begin(format!("Place {}", e.name));
                history.touch_object(&store, id);
                store.insert(crate::objects::Placed {
                    group: 0,
                    locked: false,
                    glow: None,
                    id,
                    asset: e.id.clone(),
                    variant: fastrand::u32(..e.meshes.len().max(1) as u32),
                    pos: [hit.x, hit.y - 0.05, hit.z],
                    rot: Quat::from_rotation_y(fastrand::f32() * std::f32::consts::TAU).to_array(),
                    scale: e.default_scale * (0.85 + fastrand::f32() * 0.3),
                    stretch: None,
                });
                history.commit(&store);
                meta.dirty = true;
            }
        }
        return;
    }
    if stroke.active
        && let Some(hit) = stroke.hit
    {
        let b = ed.nature_brush;
        let center = Vec2::new(hit.x, hit.z);
        let mode = match (ed.scatter_mode, stroke.inverse) {
            (ScatterMode::Add, false) => PaintMode::Grow,
            (ScatterMode::Thin, _) | (ScatterMode::Add, true) => PaintMode::Thin,
            (ScatterMode::Remove, _) => PaintMode::Erase,
            (ScatterMode::Place, _) => return,
        };
        let target = target(&settings, &lib, &rules);
        let dt = time.delta_secs().min(0.05);
        // "density" sets how far a dab goes; a second of painting reaches the full effect
        let amount = (b.strength * dt * 3.0 * settings.density.max(0.05)).min(1.0);
        remember(&mut brush.before, &field, center, b.size);
        // a selection that masks edits: the dab fades out over its feather
        match selection.as_mut().and_then(|s| s.mask(&terrain)) {
            Some(mask) => {
                field.paint_masked(center, b.size, b.falloff, mode, &target, amount, |p| {
                    mask.at(p)
                })
            }
            None => field.paint(center, b.size, b.falloff, mode, &target, amount),
        }
        meta.dirty = true;
    }
    if stroke.just_ended {
        let label = match ed.scatter_mode {
            ScatterMode::Add => "Paint nature",
            ScatterMode::Thin => "Thin nature",
            ScatterMode::Remove => "Erase nature",
            ScatterMode::Place => "Place nature",
        };
        let NatureBrush { before, .. } = &mut *brush;
        commit(before, &field, &mut history, label);
    }
}

/// Objects tab, Select: a click that hits no placed object picks the nearest procedural
/// instance under the cursor (Delete removes it, the inspector's Pin makes it a placed object).
#[allow(clippy::too_many_arguments)]
pub fn pick_tool(
    stroke: Res<Stroke>,
    ed: Res<Editor>,
    pointer: Res<crate::camera::Pointer>,
    keys: Res<ButtonInput<KeyCode>>,
    near: Res<Near>,
    lib: Option<Res<Library>>,
    mut store: ResMut<ObjectStore>,
    mut brush: ResMut<NatureBrush>,
    mut field: ResMut<NatureField>,
    mut history: ResMut<History>,
) {
    let in_select = ed.tab == Tab::Objects && ed.object_tool == ObjectTool::Select;
    if !in_select {
        brush.selected = None;
        return;
    }
    let Some(lib) = lib else { return };
    if stroke.just_started {
        brush.selected = if store.selected.is_none() {
            pick(&near, &lib, pointer.ray)
        } else {
            None
        };
    }
    if store.selected.is_some() {
        brush.selected = None;
    }
    if let Some(sel) = brush.selected
        && !pointer.keyboard_captured
        && (keys.just_pressed(KeyCode::Delete) || keys.just_pressed(KeyCode::Backspace))
    {
        remove(&mut field, &mut history, &[sel.id]);
        brush.selected = None;
        store.selected = None;
    }
}

/// Removes procedural instances by hand (one undo step).
pub fn remove(field: &mut NatureField, history: &mut History, ids: &[u64]) {
    let before = NatureUndo {
        excluded: Some(field.excluded.clone()),
        ..default()
    };
    field.excluded.extend(ids.iter().copied());
    let after = NatureUndo {
        excluded: Some(field.excluded.clone()),
        ..default()
    };
    field.touch(Rect::from_center_half_size(Vec2::ZERO, Vec2::splat(1.0e6)));
    history.push_ext("Remove nature", UNDO_KEY, Arc::new(before), Arc::new(after));
}

/// Turns procedural instances into placed objects at the same spot (they then take the slot:
/// placed objects keep procedural nature off their footprint). One undo step; returns the new ids.
pub fn pin(store: &mut ObjectStore, history: &mut History, instances: &[Instance]) -> Vec<u64> {
    history.begin("Pin nature");
    let mut ids = vec![];
    for i in instances {
        let id = store.new_id();
        history.touch_object(store, id);
        let mut p: Placed = i.placed();
        p.id = id;
        store.insert(p);
        ids.push(id);
    }
    history.commit(store);
    ids
}

/// Ray-sphere pick against the evaluated near instances.
pub fn pick(near: &Near, lib: &Library, ray: Option<Ray3d>) -> Option<Instance> {
    let ray = ray?;
    let mut best: Option<(f32, Instance)> = None;
    let mut t = 0.0;
    let mut buf = vec![];
    while t < 1500.0 {
        let p = ray.origin + *ray.direction * t;
        buf.clear();
        near.query(p.xz(), 30.0, &mut buf);
        for i in &buf {
            let e = &lib.entries[i.entry as usize];
            let r = (e.radius * i.scale).max(0.8);
            let c = i.pos + Vec3::Y * (e.height * i.scale * 0.4).min(r * 2.5);
            let oc = ray.origin - c;
            let b = oc.dot(*ray.direction);
            let disc = b * b - (oc.length_squared() - r * r);
            if disc >= 0.0 {
                let hit = -b - disc.sqrt();
                if hit > 0.0 && best.is_none_or(|(bt, _)| hit < bt) {
                    best = Some((hit, *i));
                }
            }
        }
        if best.is_some() {
            break;
        }
        t += 20.0;
    }
    best.map(|b| b.1)
}
