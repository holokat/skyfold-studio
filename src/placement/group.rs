//! Groups: the objects of one set piece or town share `Placed::group`, so the set behaves as one
//! object. Clicking any of its pieces selects the set; the handles move, turn and resize it as a
//! whole around its centre (each piece is moved in place and set back on the ground at its own
//! spot, so a moved graveyard's stones sit on the new ground); Delete removes it, ⌘D copies it,
//! Z / X turn it; Ungroup (Objects panel) frees the pieces. MCP: `objects_group`.

use bevy::prelude::*;

use super::footprint::{Footprint, ModelBounds};
use crate::objects::{Library, ObjectStore, Placed};
use crate::terrain::Terrain;

/// Unit bounds the group frame is drawn with: the frame's `scale3` is the set's size.
pub const FRAME_BOUNDS: (Vec3, Vec3) = (Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5));

/// The group's handles work on a stand-in object: centred on the set, sized to it, unturned.
pub fn frame(store: &ObjectStore, lib: &Library, bounds: &ModelBounds, g: u64) -> Option<Placed> {
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    let (mut base, mut top) = (f32::MAX, f32::MIN);
    let mut n = 0;
    for p in store.items.values().filter(|p| p.group == g) {
        let Some(e) = lib.get(&p.asset) else { continue };
        let b = bounds.of(e);
        let (a, z) = Footprint::of(p, b).aabb(0.0);
        lo = lo.min(a);
        hi = hi.max(z);
        base = base.min(p.pos[1]);
        top = top.max(p.pos[1] + b.1.y * p.scale3().y);
        n += 1;
    }
    if n == 0 {
        return None;
    }
    let c = (lo + hi) * 0.5;
    let mut f = Placed {
        id: 0,
        asset: String::new(),
        variant: 0,
        pos: [c.x, base, c.y],
        rot: Quat::IDENTITY.to_array(),
        scale: 1.0,
        stretch: None,
        group: g,
        locked: false,
        glow: None,
    };
    f.set_scale3(Vec3::new(
        (hi.x - lo.x).max(0.5),
        (top - base).max(0.5),
        (hi.y - lo.y).max(0.5),
    ));
    Some(f)
}

/// A rigid change of a set: turn by `yaw` (as `Quat::from_rotation_y`) and resize by `k` about
/// `pivot`, then move by `shift` (x, z) and lift by `lift`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Change {
    pub pivot: Vec3,
    pub shift: Vec2,
    pub yaw: f32,
    pub k: f32,
    pub lift: f32,
}

impl Change {
    pub fn none(pivot: Vec3) -> Change {
        Change {
            pivot,
            shift: Vec2::ZERO,
            yaw: 0.0,
            k: 1.0,
            lift: 0.0,
        }
    }
}

/// The pieces after `ch`, from their states `starts`: each keeps its height above the ground
/// (sunk bones stay sunk, lifted lamps stay lifted), scaled with the set.
pub fn apply(starts: &[Placed], ch: Change, terrain: &Terrain) -> Vec<Placed> {
    let q = Quat::from_rotation_y(ch.yaw);
    starts
        .iter()
        .map(|m| {
            let rel = m.translation() - ch.pivot;
            let r = q * Vec3::new(rel.x * ch.k, 0.0, rel.z * ch.k);
            let (x, z) = (ch.pivot.x + ch.shift.x + r.x, ch.pivot.z + ch.shift.y + r.z);
            let above = m.pos[1] - terrain.height_at(m.pos[0], m.pos[2]);
            let y = terrain.height_at(x, z) + above * ch.k + ch.lift;
            let mut p = m.clone();
            p.pos = [x, y, z];
            p.rot = (q * m.rotation()).to_array();
            if (ch.k - 1.0).abs() > 1e-6 {
                p.set_scale3(m.scale3() * ch.k);
            }
            p
        })
        .collect()
}

/// Applies `ch` to group `g` in the store (moved in place), recording into the open undo step.
pub fn transform(
    store: &mut ObjectStore,
    history: &mut crate::history::History,
    terrain: &Terrain,
    g: u64,
    starts: &[Placed],
    ch: Change,
) {
    for p in apply(starts, ch, terrain) {
        history.touch_object(store, p.id);
        store.move_to(p);
    }
    let _ = g;
}

/// The set's pieces as they are now.
pub fn snapshot(store: &ObjectStore, g: u64) -> Vec<Placed> {
    store
        .group_members(g)
        .into_iter()
        .filter_map(|id| store.items.get(&id).cloned())
        .collect()
}

/// A copy of group `g` beside it (new ids, a new group id), not yet inserted.
pub fn duplicate(
    store: &mut ObjectStore,
    lib: &Library,
    bounds: &ModelBounds,
    terrain: &Terrain,
    g: u64,
) -> Vec<Placed> {
    let Some(f) = frame(store, lib, bounds, g) else {
        return vec![];
    };
    let starts = snapshot(store, g);
    if starts.iter().any(|p| !crate::market::allow(&p.asset)) {
        return vec![];
    }
    let shift = Vec2::new(f.scale3().x + 4.0, 0.0);
    let ng = store.new_id();
    apply(
        &starts,
        Change {
            shift,
            ..Change::none(f.translation())
        },
        terrain,
    )
    .into_iter()
    .map(|mut p| {
        p.id = store.new_id();
        p.group = ng;
        p.locked = false;
        p
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(id: u64, x: f32, z: f32) -> Placed {
        Placed {
            id,
            asset: "a".into(),
            variant: 0,
            pos: [x, 0.0, z],
            rot: Quat::IDENTITY.to_array(),
            scale: 1.0,
            stretch: None,
            group: 7,
            locked: false,
            glow: None,
        }
    }

    #[test]
    fn a_set_turns_and_resizes_as_one() {
        let t = Terrain::new(64, 4.0, -10.0);
        let starts = [piece(1, 2.0, 0.0), piece(2, -2.0, 0.0)];
        // a quarter turn about the middle swaps x into -z (Quat::from_rotation_y)
        let out = apply(
            &starts,
            Change {
                yaw: std::f32::consts::FRAC_PI_2,
                k: 2.0,
                ..Change::none(Vec3::ZERO)
            },
            &t,
        );
        assert!(
            (out[0].translation().xz() - Vec2::new(0.0, -4.0)).length() < 1e-4,
            "{:?}",
            out[0].pos
        );
        assert!((out[1].translation().xz() - Vec2::new(0.0, 4.0)).length() < 1e-4);
        // (each piece keeps its height above the ground, scaled with the set)
        let h = t.height_at(0.0, 0.0);
        assert!((out[0].pos[1] - (h + (0.0 - h) * 2.0)).abs() < 1e-3);
        assert!((out[0].scale - 2.0).abs() < 1e-5);
        // a move keeps the shape
        let moved = apply(
            &starts,
            Change {
                shift: Vec2::new(10.0, 5.0),
                ..Change::none(Vec3::ZERO)
            },
            &t,
        );
        assert_eq!(moved[0].translation().xz(), Vec2::new(12.0, 5.0));
        assert_eq!(moved[1].translation().xz(), Vec2::new(8.0, 5.0));
    }
}
