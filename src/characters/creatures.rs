//! Creatures in their spawn areas while walking: every creature type with a model
//! (`CreatureType::character`) roams the Gameplay spawn areas that list it, a few at a time
//! near the player. Hostile ones come for the player and attack when close, neutral ones keep
//! to themselves, passive ones shy away. The player's attacks (`Strikes`) hit them: they flinch,
//! fall after a few hits and come back after the area's respawn time.

use std::collections::HashMap;

use bevy::prelude::*;

use super::moves::{self, Move};
use super::puppet::{self, Puppet};
use super::{Characters, labs_enabled};
use crate::gameplay::{Aggression, Gameplay};
use crate::terrain::Terrain;

/// The player's swings this frame: (where, facing direction on the ground).
#[derive(Resource, Default)]
pub struct Strikes(pub Vec<(Vec3, Vec2)>);

struct Mob {
    entity: Entity,
    creature: u32,
    rig: String,
    pos: Vec3,
    yaw: f32,
    goal: Option<Vec2>,
    wait: f32,
    hp: i32,
    /// fell at this time (taken away a while later)
    dead: Option<f32>,
    /// attacking / flinching: back to roaming when the clip ends
    busy: bool,
}

#[derive(Default)]
struct Areas {
    /// spawn region id → its creatures
    mobs: HashMap<u32, Vec<Mob>>,
    /// region id → when its fallen come back
    respawn: HashMap<u32, f32>,
    seed: u32,
}

pub struct CreaturesPlugin;

impl Plugin for CreaturesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Strikes>()
            .add_systems(Update, roam.after(super::player::PlayerSystems));
    }
}

fn rand(seed: &mut u32) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    (*seed & 0xFFFF) as f32 / 65535.0
}

fn inside(poly: &[[f32; 2]], p: Vec2) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a[1] > p.y) != (b[1] > p.y) && p.x < (b[0] - a[0]) * (p.y - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
    }
    c
}

fn point_in(poly: &[[f32; 2]], seed: &mut u32) -> Option<Vec2> {
    let (mut lo, mut hi) = (Vec2::MAX, Vec2::MIN);
    for p in poly {
        lo = lo.min(Vec2::from(*p));
        hi = hi.max(Vec2::from(*p));
    }
    for _ in 0..40 {
        let q = lo + (hi - lo) * Vec2::new(rand(seed), rand(seed));
        if inside(poly, q) {
            return Some(q);
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn roam(
    gp: Res<Gameplay>,
    chars: Res<Characters>,
    terrain: Res<Terrain>,
    walk: Res<crate::walk::WalkState>,
    mut strikes: ResMut<Strikes>,
    time: Res<Time>,
    server: Res<AssetServer>,
    mut st: Local<Areas>,
    mut puppets: Query<(&mut Puppet, &mut Transform)>,
    mut commands: Commands,
) {
    let swings = std::mem::take(&mut strikes.0);
    let on = labs_enabled() && walk.active;
    let player = walk.position;
    let now = time.elapsed_secs();
    let dt = time.delta_secs().min(0.1);
    if st.seed == 0 {
        st.seed = 0x1234_5678;
    }
    // take down areas far away (or everything, not walking)
    let far: Vec<u32> = st
        .mobs
        .keys()
        .copied()
        .filter(|id| {
            !on || gp
                .spawns
                .iter()
                .find(|s| s.id == *id)
                .and_then(|s| s.centroid())
                .is_none_or(|c| c.distance(player.xz()) > 260.0)
        })
        .collect();
    for id in far {
        for m in st.mobs.remove(&id).unwrap_or_default() {
            commands.entity(m.entity).despawn();
        }
    }
    if !on {
        return;
    }
    for s in &gp.spawns {
        let Some(c) = s.centroid() else { continue };
        if c.distance(player.xz()) > 200.0 || s.polygon.len() < 3 {
            continue;
        }
        // its creature types with a model
        let kinds: Vec<(u32, &crate::gameplay::CreatureType)> = s
            .entries
            .iter()
            .filter_map(|e| gp.creatures.iter().find(|c| c.id == e.creature))
            .filter(|c| c.character.as_ref().is_some_and(|ch| chars.get(ch).is_some_and(|e| e.rigged())))
            .map(|c| (c.id, c))
            .collect();
        if kinds.is_empty() {
            continue;
        }
        let km2 = s.area() / 1e6;
        let mut want = ((s.density * km2).round() as usize).clamp(1, 6);
        if s.max_alive > 0 {
            want = want.min(s.max_alive as usize);
        }
        let mut seed = st.seed;
        let ready = st.respawn.get(&s.id).is_none_or(|t| now >= *t);
        let mobs = st.mobs.entry(s.id).or_default();
        // fallen ones are taken away after a while; the area refills after its respawn time
        let before = mobs.len();
        mobs.retain(|m| {
            let keep = m.dead.is_none_or(|t| now - t < 6.0);
            if !keep {
                commands.entity(m.entity).despawn();
            }
            keep
        });
        let respawn_due = mobs.len() < before;
        if ready && mobs.len() < want {
            let (cid, kind) = kinds[(rand(&mut seed) * kinds.len() as f32) as usize % kinds.len()];
            if let Some(p) = point_in(&s.polygon, &mut seed)
                && let Some(entry) = kind.character.as_ref().and_then(|c| chars.get(c))
            {
                let pos = Vec3::new(p.x, terrain.height_at(p.x, p.y), p.y);
                let yaw = rand(&mut seed) * std::f32::consts::TAU;
                let idle = moves::anim_for(&entry.meta.moves, Move::Idle);
                if let Some(e) = puppet::spawn(
                    &mut commands,
                    &server,
                    entry,
                    Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw)),
                    Some(idle),
                    None,
                ) {
                    commands.entity(e).insert(Name::new(format!("Creature {}", kind.name)));
                    mobs.push(Mob {
                        entity: e,
                        creature: cid,
                        rig: entry.rig_key(),
                        pos,
                        yaw,
                        goal: None,
                        wait: now + rand(&mut seed) * 3.0,
                        hp: if kind.elite { 6 } else { 3 },
                        dead: None,
                        busy: false,
                    });
                }
            }
        }
        if respawn_due {
            st.respawn.insert(s.id, now + s.respawn_secs.max(5.0));
        }
        let mobs = st.mobs.get_mut(&s.id).unwrap();
        for m in mobs.iter_mut() {
            let Some(kind) = gp.creatures.iter().find(|c| c.id == m.creature) else {
                continue;
            };
            let Some(entry) = kind.character.as_ref().and_then(|c| chars.get(c)) else {
                continue;
            };
            if entry.rig_key() != m.rig {
                continue;
            }
            let Ok((mut p, mut tf)) = puppets.get_mut(m.entity) else {
                continue;
            };
            let anim = |mv: Move| moves::anim_for(&entry.meta.moves, mv);
            if m.dead.is_some() {
                continue;
            }
            // hit by the player's swing: in reach and in front of them
            for (at, dir) in &swings {
                let d = m.pos.xz() - at.xz();
                if d.length() < 2.4 && d.normalize_or_zero().dot(*dir) > 0.2 {
                    m.hp -= 1;
                    m.busy = true;
                    if m.hp <= 0 {
                        m.dead = Some(now);
                        p.play(&anim(Move::Death), true);
                    } else {
                        p.restart(&anim(Move::Hit), true);
                    }
                }
            }
            if m.dead.is_some() {
                continue;
            }
            if m.busy {
                if p.finished {
                    m.busy = false;
                } else {
                    continue;
                }
            }
            let to_player = player.xz() - m.pos.xz();
            let dist = to_player.length();
            let (goal, speed, mv) = match kind.aggression {
                Aggression::Hostile if dist < 16.0 => {
                    if dist < 1.7 {
                        // in reach: face and strike
                        m.yaw = turn(m.yaw, to_player.x.atan2(to_player.y), dt * 8.0);
                        m.busy = true;
                        p.speed = 1.0;
                        p.restart(&anim(Move::LightAttack), true);
                        tf.rotation = Quat::from_rotation_y(m.yaw);
                        continue;
                    }
                    (Some(player.xz()), 3.6, Move::Run)
                }
                Aggression::Passive if dist < 7.0 => {
                    (Some(m.pos.xz() - to_player.normalize_or_zero() * 6.0), 3.6, Move::Run)
                }
                _ => {
                    if m.goal.is_none() && now >= m.wait {
                        m.goal = point_in(&s.polygon, &mut seed);
                    }
                    (m.goal, 1.4, Move::Walk)
                }
            };
            match goal {
                Some(g) if g.distance(m.pos.xz()) > 0.5 => {
                    let d = g - m.pos.xz();
                    let step = d.normalize() * (speed * dt).min(d.length());
                    let np = m.pos.xz() + step;
                    // (creatures keep to their area unless chasing)
                    if mv == Move::Run || inside(&s.polygon, np) {
                        m.pos = Vec3::new(np.x, terrain.height_at(np.x, np.y), np.y);
                    } else {
                        m.goal = None;
                    }
                    m.yaw = turn(m.yaw, d.x.atan2(d.y), dt * 6.0);
                    p.play(&anim(mv), false);
                    p.speed = speed / mv.speed().unwrap_or(1.6);
                }
                _ => {
                    if m.goal.is_some() {
                        m.goal = None;
                        m.wait = now + 2.0 + rand(&mut seed) * 6.0;
                    }
                    p.speed = 1.0;
                    p.play(&anim(Move::Idle), false);
                }
            }
            tf.translation = m.pos;
            tf.rotation = Quat::from_rotation_y(m.yaw);
        }
        st.seed = seed;
    }
}

fn turn(from: f32, to: f32, max: f32) -> f32 {
    let mut d = (to - from) % std::f32::consts::TAU;
    if d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    } else if d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    from + d.clamp(-max, max)
}
