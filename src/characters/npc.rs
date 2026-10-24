//! NPCs with a body: a Gameplay NPC (`crate::gameplay::Npc`) gets a character and a life
//! (`NpcLife`): what it does (stand, wander, patrol a route, or follow a day schedule), how it
//! treats the player, what it says and which moves it uses. Its character stands where the NPC
//! is; while walking (or with "Preview behaviors" on) it lives out its behavior: walking
//! between spots on the ground, sitting, working, sleeping, turning to greet the player.

use std::collections::{BTreeMap, HashMap};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::moves::{self, Move};
use super::puppet::{self, Puppet};
use super::{Characters, labs_enabled};
use crate::gameplay::Gameplay;
use crate::terrain::Terrain;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Behavior {
    /// stays on its spot (idle, now and then a look round)
    #[default]
    Stand,
    /// roams within `wander_radius` of its spot
    Wander,
    /// walks its route, waypoint to waypoint
    Patrol,
    /// does what its schedule says for the time of day
    Schedule,
}

impl Behavior {
    pub const ALL: [Behavior; 4] = [
        Behavior::Stand,
        Behavior::Wander,
        Behavior::Patrol,
        Behavior::Schedule,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Behavior::Stand => "Stand",
            Behavior::Wander => "Wander",
            Behavior::Patrol => "Patrol",
            Behavior::Schedule => "Schedule",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Behavior::Stand => "Stays on its spot, idling",
            Behavior::Wander => "Roams round its spot and stops now and then",
            Behavior::Patrol => "Walks a route you click on the ground",
            Behavior::Schedule => "Does different things at different times of day",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Activity {
    #[default]
    Stand,
    Wander,
    Patrol,
    Sit,
    Work,
    Talk,
    Sleep,
}

impl Activity {
    pub const ALL: [Activity; 7] = [
        Activity::Stand,
        Activity::Wander,
        Activity::Patrol,
        Activity::Sit,
        Activity::Work,
        Activity::Talk,
        Activity::Sleep,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Activity::Stand => "Stand",
            Activity::Wander => "Walk around",
            Activity::Patrol => "Patrol its route",
            Activity::Sit => "Sit",
            Activity::Work => "Work",
            Activity::Talk => "Chat",
            Activity::Sleep => "Sleep",
        }
    }
    fn anim(self) -> Move {
        match self {
            Activity::Stand | Activity::Wander | Activity::Patrol => Move::Idle,
            Activity::Sit => Move::Sit,
            Activity::Work => Move::Work,
            Activity::Talk => Move::Talk,
            Activity::Sleep => Move::Sleep,
        }
    }
}

/// One line of a schedule: from `hour` on, do `activity` at `at` (its own spot when `None`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    pub hour: f32,
    pub activity: Activity,
    #[serde(default)]
    pub at: Option<[f32; 2]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Attitude {
    #[default]
    Friendly,
    Neutral,
    Hostile,
}

impl Attitude {
    pub const ALL: [Attitude; 3] = [Attitude::Friendly, Attitude::Neutral, Attitude::Hostile];
    pub fn name(self) -> &'static str {
        match self {
            Attitude::Friendly => "Friendly",
            Attitude::Neutral => "Neutral",
            Attitude::Hostile => "Hostile",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Lines {
    #[serde(default)]
    pub greet: String,
    #[serde(default)]
    pub idle: String,
    #[serde(default)]
    pub farewell: String,
}

fn yes() -> bool {
    true
}
fn twelve() -> f32 {
    12.0
}
fn thirty() -> f32 {
    30.0
}
fn one() -> f32 {
    1.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NpcLife {
    /// the library character standing in for it (`None`: just the marker)
    #[serde(default)]
    pub character: Option<String>,
    #[serde(default)]
    pub behavior: Behavior,
    #[serde(default = "twelve")]
    pub wander_radius: f32,
    /// waypoints [x, z]
    #[serde(default)]
    pub route: Vec<[f32; 2]>,
    /// round and round (else there and back)
    #[serde(default = "yes")]
    pub route_loop: bool,
    #[serde(default)]
    pub schedule: Vec<Slot>,
    /// never further than this from its spot (m), whatever it's doing
    #[serde(default = "thirty")]
    pub leash: f32,
    /// walking pace (× a normal walk)
    #[serde(default = "one")]
    pub speed: f32,
    #[serde(default)]
    pub attitude: Attitude,
    #[serde(default)]
    pub faction: String,
    #[serde(default = "yes")]
    pub can_talk: bool,
    #[serde(default)]
    pub can_trade: bool,
    #[serde(default)]
    pub gives_quests: bool,
    #[serde(default)]
    pub attackable: bool,
    #[serde(default)]
    pub can_join: bool,
    #[serde(default)]
    pub lines: Lines,
    /// its own choice of clip per move (else the character's, else the library's)
    #[serde(default)]
    pub moves: BTreeMap<String, String>,
}

impl Default for NpcLife {
    fn default() -> Self {
        Self {
            character: None,
            behavior: Behavior::Stand,
            wander_radius: 12.0,
            route: vec![],
            route_loop: true,
            schedule: vec![],
            leash: 30.0,
            speed: 1.0,
            attitude: Attitude::Friendly,
            faction: String::new(),
            can_talk: true,
            can_trade: false,
            gives_quests: false,
            attackable: false,
            can_join: false,
            lines: Lines::default(),
            moves: BTreeMap::new(),
        }
    }
}

impl NpcLife {
    /// Nothing set (not written to world.json).
    pub fn is_plain(&self) -> bool {
        *self == Self::default()
    }
    /// A sensible day for a townsperson, round its spot.
    pub fn typical_day(&mut self) {
        self.schedule = vec![
            Slot { hour: 6.0, activity: Activity::Wander, at: None },
            Slot { hour: 10.0, activity: Activity::Work, at: None },
            Slot { hour: 13.0, activity: Activity::Sit, at: None },
            Slot { hour: 15.0, activity: Activity::Talk, at: None },
            Slot { hour: 19.0, activity: Activity::Wander, at: None },
            Slot { hour: 22.0, activity: Activity::Sleep, at: None },
        ];
        self.behavior = Behavior::Schedule;
    }
    /// The schedule line in force at `hour` (the last one started, wrapping past midnight).
    pub fn slot_at(&self, hour: f32) -> Option<&Slot> {
        let mut s: Vec<&Slot> = self.schedule.iter().collect();
        s.sort_by(|a, b| a.hour.total_cmp(&b.hour));
        s.iter()
            .rev()
            .find(|x| x.hour <= hour)
            .or(s.last())
            .copied()
    }
}

/// The clip an NPC plays for `m`: its own, its character's, the library's.
pub fn anim_of(life: &NpcLife, chars: &Characters, m: Move) -> String {
    life.moves.get(m.id()).cloned().unwrap_or_else(|| {
        let own = life
            .character
            .as_ref()
            .and_then(|c| chars.get(c))
            .map(|e| e.meta.moves.clone())
            .unwrap_or_default();
        moves::anim_for(&own, m)
    })
}

/// Editing NPC lives in the world (shared with the Gameplay tab's NPC panel).
#[derive(Resource, Default)]
pub struct NpcEdit {
    /// NPCs act out their behavior in the editor too (always while walking)
    pub preview: bool,
    /// clicks on the ground add waypoints to this NPC's route
    pub drawing_route: Option<u32>,
    /// the next click on the ground is this NPC's schedule line's spot
    pub picking_spot: Option<(u32, usize)>,
    /// clicks on the ground stand this NPC there (facing the camera), until Esc
    pub moving: Option<u32>,
}

impl NpcEdit {
    /// Clicks on the ground belong to an edit (the window is put away for it).
    pub fn busy(&self) -> bool {
        self.drawing_route.is_some() || self.picking_spot.is_some() || self.moving.is_some()
    }
    pub fn stop(&mut self) {
        self.drawing_route = None;
        self.picking_spot = None;
        self.moving = None;
    }
}

// ------------------------------------------------------------------ living

/// What a living NPC is doing right now (not saved).
#[derive(Clone, Debug)]
struct Run {
    pos: Vec3,
    yaw: f32,
    /// walking to: (point, then do)
    goal: Option<Vec2>,
    /// idling / doing an activity until this time
    wait: f32,
    activity: Activity,
    /// route waypoint (and direction, there and back)
    leg: usize,
    back: bool,
    /// the schedule line it acted on last (a new one starts a new activity)
    slot: Option<(u32, usize)>,
    /// greeted the player this visit
    greeted: bool,
}

#[derive(Default)]
struct Living {
    spawned: HashMap<u32, (Entity, String, String)>,
    runs: HashMap<u32, Run>,
}

/// Lines said over NPCs' heads (the walk view draws them).
#[derive(Resource, Default)]
pub struct Bubbles(pub Vec<(Vec3, String)>);

pub struct NpcPlugin;

impl Plugin for NpcPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NpcEdit>()
            .init_resource::<Bubbles>()
            .add_systems(Update, (live, route_clicks, draw_routes))
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                bubbles.after(crate::ui::editor_ui),
            );
    }
}

fn rng(seed: u32, k: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ k.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

#[allow(clippy::too_many_arguments)]
fn live(
    gp: Res<Gameplay>,
    chars: Res<Characters>,
    terrain: Res<Terrain>,
    lighting: Res<crate::sky::Lighting>,
    walk: Res<crate::walk::WalkState>,
    edit: Res<NpcEdit>,
    time: Res<Time>,
    server: Res<AssetServer>,
    mut state: Local<Living>,
    mut puppets: Query<(&mut Puppet, &mut Transform)>,
    mut bubbles: ResMut<Bubbles>,
    mut commands: Commands,
) {
    bubbles.0.clear();
    let on = labs_enabled();
    let living = walk.active || edit.preview;
    let st = &mut *state;
    // take down the gone (or changed) ones
    st.spawned.retain(|id, (e, character, rig)| {
        let keep = on
            && gp.npcs.iter().any(|n| {
                n.id == *id
                    && n.life.character.as_deref() == Some(character.as_str())
                    && chars.get(character).map(|c| c.rig_key()).as_deref() == Some(rig.as_str())
            });
        if !keep {
            commands.entity(*e).despawn();
        }
        keep
    });
    st.runs.retain(|id, _| st.spawned.contains_key(id) || living);
    if !on {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    let now = time.elapsed_secs();
    for n in &gp.npcs {
        let Some(cid) = &n.life.character else {
            continue;
        };
        let Some(entry) = chars.get(cid) else {
            continue;
        };
        if !entry.rigged() {
            continue;
        }
        let home = Vec3::from(n.pos);
        // gameplay facing: 0 = +x, counter-clockwise from above (towards -z); a character
        // turned by `a` about +y faces (sin a, cos a)
        let home_yaw = n.yaw + std::f32::consts::FRAC_PI_2;
        let ent = match st.spawned.get(&n.id) {
            Some((e, _, _)) => *e,
            None => {
                let tf = Transform::from_translation(home).with_rotation(Quat::from_rotation_y(home_yaw));
                let Some(e) = puppet::spawn(&mut commands, &server, entry, tf, None, None) else {
                    continue;
                };
                commands.entity(e).insert(Name::new(format!("NPC {}", n.name)));
                st.spawned.insert(n.id, (e, cid.clone(), entry.rig_key()));
                continue;
            }
        };
        let Ok((mut p, mut tf)) = puppets.get_mut(ent) else {
            continue;
        };
        let life = &n.life;
        if !living {
            // in the editor: on its spot, idling
            st.runs.remove(&n.id);
            *tf = Transform::from_translation(home).with_rotation(Quat::from_rotation_y(home_yaw));
            p.speed = 1.0;
            p.play(&anim_of(life, &chars, Move::Idle), false);
            continue;
        }
        let run = st.runs.entry(n.id).or_insert_with(|| Run {
            pos: home,
            yaw: home_yaw,
            goal: None,
            wait: now + 1.0 + rng(n.id, 1) * 3.0,
            activity: Activity::Stand,
            leg: 0,
            back: false,
            slot: None,
            greeted: false,
        });
        // what it should be doing
        let (activity, centre) = match life.behavior {
            Behavior::Stand => (Activity::Stand, home.xz()),
            Behavior::Wander => (Activity::Wander, home.xz()),
            Behavior::Patrol => (Activity::Patrol, home.xz()),
            Behavior::Schedule => match life.slot_at(lighting.hour) {
                Some(s) => {
                    let idx = life.schedule.iter().position(|x| x == s).unwrap_or(0);
                    if run.slot != Some((n.id, idx)) {
                        run.slot = Some((n.id, idx));
                        run.goal = None;
                        run.wait = 0.0;
                    }
                    (s.activity, s.at.map(Vec2::from).unwrap_or(home.xz()))
                }
                None => (Activity::Stand, home.xz()),
            },
        };
        run.activity = activity;
        // the player close by: turn to them and greet
        let player = walk.active.then_some(walk.position);
        let near_player = player.filter(|q| q.xz().distance(run.pos.xz()) < 3.5 && life.can_talk);
        if let Some(q) = near_player {
            let d = q.xz() - run.pos.xz();
            let want = d.x.atan2(d.y);
            run.yaw = turn(run.yaw, want, dt * 5.0);
            if !run.greeted {
                run.greeted = true;
                p.play(&anim_of(life, &chars, Move::Wave), true);
            } else if p.finished || p.anim.as_deref() != Some(&anim_of(life, &chars, Move::Wave)) {
                p.play(&anim_of(life, &chars, Move::Talk), false);
            }
            let line = if life.lines.greet.is_empty() {
                "Hello there!".to_string()
            } else {
                life.lines.greet.clone()
            };
            bubbles.0.push((run.pos + Vec3::Y * (entry.meta.height.max(1.0) + 0.35), line));
            tf.translation = run.pos;
            tf.rotation = Quat::from_rotation_y(run.yaw);
            continue;
        }
        if player.is_some_and(|q| q.xz().distance(run.pos.xz()) > 6.0) {
            run.greeted = false;
        }
        // walking somewhere
        if let Some(goal) = run.goal {
            let d = goal - run.pos.xz();
            let dist = d.length();
            if dist < 0.4 {
                run.goal = None;
                run.wait = now + match activity {
                    Activity::Wander => 3.0 + rng(n.id, now as u32) * 6.0,
                    Activity::Patrol => 1.0 + rng(n.id, now as u32) * 2.0,
                    _ => 1e9,
                };
            } else {
                let speed = 1.4 * life.speed.clamp(0.3, 3.0);
                let step = d / dist * (speed * dt).min(dist);
                let mut np = run.pos.xz() + step;
                // (on its leash)
                if np.distance(home.xz()) > life.leash.max(2.0) {
                    np = home.xz() + (np - home.xz()).normalize() * life.leash.max(2.0);
                    run.goal = None;
                }
                run.pos = Vec3::new(np.x, terrain.height_at(np.x, np.y), np.y);
                run.yaw = turn(run.yaw, d.x.atan2(d.y), dt * 6.0);
                p.play(&anim_of(life, &chars, Move::Walk), false);
                p.speed = speed / Move::Walk.speed().unwrap_or(1.6);
                tf.translation = run.pos;
                tf.rotation = Quat::from_rotation_y(run.yaw);
                continue;
            }
        }
        // there: do the activity, or set off for the next spot
        p.speed = 1.0;
        let at_spot = run.pos.xz().distance(centre) < 1.0;
        match activity {
            Activity::Wander if now >= run.wait => {
                let a = rng(n.id, (now * 7.0) as u32) * std::f32::consts::TAU;
                let r = life.wander_radius.max(1.0) * rng(n.id, (now * 13.0) as u32).sqrt();
                run.goal = Some(centre + Vec2::new(a.cos(), a.sin()) * r);
            }
            Activity::Patrol if now >= run.wait && !life.route.is_empty() => {
                let k = life.route.len();
                run.goal = Some(Vec2::from(life.route[run.leg.min(k - 1)]));
                if life.route_loop {
                    run.leg = (run.leg + 1) % k;
                } else {
                    if run.back {
                        if run.leg == 0 {
                            run.back = false;
                            run.leg = 1.min(k - 1);
                        } else {
                            run.leg -= 1;
                        }
                    } else if run.leg + 1 >= k {
                        run.back = true;
                        run.leg = k.saturating_sub(2);
                    } else {
                        run.leg += 1;
                    }
                }
            }
            Activity::Stand | Activity::Sit | Activity::Work | Activity::Talk | Activity::Sleep
                if !at_spot =>
            {
                run.goal = Some(centre);
            }
            _ => {
                if matches!(activity, Activity::Stand | Activity::Sit | Activity::Work | Activity::Talk | Activity::Sleep) {
                    run.yaw = turn(run.yaw, home_yaw, dt * 3.0);
                }
                let m = activity.anim();
                p.play(&anim_of(life, &chars, m), m == Move::Sleep);
                if m == Move::Sleep {
                    // (lying down: the clip's first frame)
                    p.time = 0.0;
                }
                if activity == Activity::Talk && !life.lines.idle.is_empty() {
                    bubbles.0.push((run.pos + Vec3::Y * (entry.meta.height.max(1.0) + 0.35), life.lines.idle.clone()));
                }
            }
        }
        tf.translation = run.pos;
        tf.rotation = Quat::from_rotation_y(run.yaw);
    }
}

/// `from` turned towards `to` by at most `max` radians.
fn turn(from: f32, to: f32, max: f32) -> f32 {
    let mut d = (to - from) % std::f32::consts::TAU;
    if d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    } else if d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    from + d.clamp(-max, max)
}

// ------------------------------------------------------------------ editing in the world

/// Moving NPCs, route drawing and spot picking: clicks on the ground.
#[allow(clippy::too_many_arguments)]
fn route_clicks(
    mut edit: ResMut<NpcEdit>,
    mut gp: ResMut<Gameplay>,
    pointer: Res<crate::camera::Pointer>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut meta: ResMut<crate::world::WorldMeta>,
    mut history: ResMut<crate::history::History>,
    store: Res<crate::objects::ObjectStore>,
    cam: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
) {
    if !edit.busy() {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Enter) || mouse.just_pressed(MouseButton::Right) {
        edit.stop();
        return;
    }
    if pointer.over_ui || !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(hit) = pointer.hit else {
        return;
    };
    if let Some(id) = edit.moving {
        // stands where clicked, facing the camera (gameplay facing: 0 = +x, counter-clockwise)
        let yaw = cam.single().ok().map(|c| {
            let d = c.translation() - hit;
            (-d.z).atan2(d.x)
        });
        history.begin("Move NPC");
        history.touch_gameplay(&gp);
        if let Some(n) = gp.npcs.iter_mut().find(|n| n.id == id) {
            n.pos = hit.to_array();
            if let Some(y) = yaw {
                n.yaw = y;
            }
        }
        history.commit(&store);
        gp.dirty = true;
        meta.dirty = true;
    } else if let Some(id) = edit.drawing_route {
        if let Some(n) = gp.npcs.iter_mut().find(|n| n.id == id) {
            n.life.route.push([hit.x, hit.z]);
            gp.dirty = true;
            meta.dirty = true;
        }
    } else if let Some((id, k)) = edit.picking_spot.take() {
        if let Some(s) = gp
            .npcs
            .iter_mut()
            .find(|n| n.id == id)
            .and_then(|n| n.life.schedule.get_mut(k))
        {
            s.at = Some([hit.x, hit.z]);
            gp.dirty = true;
            meta.dirty = true;
        }
    }
}

/// The selected NPC's route, wander circle and schedule spots on the ground.
fn draw_routes(
    gp: Res<Gameplay>,
    ed: Res<crate::editor::Editor>,
    view: Res<super::view::CharacterView>,
    terrain: Res<Terrain>,
    mut gizmos: Gizmos,
) {
    if !labs_enabled() || ed.tab != crate::editor::Tab::Characters {
        return;
    }
    let Some(super::view::Sel::Npc(id)) = view.sel else {
        return;
    };
    let Some(n) = gp.npcs.iter().find(|n| n.id == id) else {
        return;
    };
    let lift = |p: Vec2| Vec3::new(p.x, terrain.height_at(p.x, p.y) + 0.15, p.y);
    let home = Vec3::from(n.pos);
    let life = &n.life;
    let col = Color::srgba(1.0, 0.85, 0.35, 0.9);
    if life.behavior == Behavior::Wander {
        let pts: Vec<Vec3> = (0..=48)
            .map(|i| {
                let a = i as f32 / 48.0 * std::f32::consts::TAU;
                lift(home.xz() + Vec2::new(a.cos(), a.sin()) * life.wander_radius)
            })
            .collect();
        gizmos.linestrip(pts, col);
    }
    if !life.route.is_empty() {
        let mut pts: Vec<Vec3> = life.route.iter().map(|p| lift(Vec2::from(*p))).collect();
        if life.route_loop && pts.len() > 2 {
            pts.push(pts[0]);
        }
        gizmos.linestrip(pts.clone(), Color::srgba(0.5, 0.85, 1.0, 0.95));
        for p in &pts {
            gizmos.sphere(Isometry3d::from_translation(*p), 0.35, Color::srgba(0.5, 0.85, 1.0, 0.95));
        }
    }
    for s in &life.schedule {
        if let Some(a) = s.at {
            let p = lift(Vec2::from(a));
            gizmos.sphere(Isometry3d::from_translation(p), 0.5, col);
            gizmos.line(lift(home.xz()), p, col.with_alpha(0.4));
        }
    }
}

/// Speech bubbles over NPCs (while walking).
fn bubbles(
    mut contexts: bevy_egui::EguiContexts,
    b: Res<Bubbles>,
    cam: Query<(&Camera, &GlobalTransform), With<crate::camera::MainCamera>>,
) -> Result {
    if b.0.is_empty() {
        return Ok(());
    }
    let Ok((cam, ct)) = cam.single() else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?.clone();
    let painter = ctx.layer_painter(bevy_egui::egui::LayerId::new(
        bevy_egui::egui::Order::Foreground,
        bevy_egui::egui::Id::new("npc_bubbles"),
    ));
    use bevy_egui::egui::{Align2, Color32, CornerRadius, FontId, Rect, pos2, vec2};
    for (p, text) in &b.0 {
        let Ok(v) = cam.world_to_viewport(ct, *p) else {
            continue;
        };
        let g = painter.layout(text.clone(), FontId::proportional(13.0), Color32::WHITE, 220.0);
        let r = Rect::from_center_size(pos2(v.x, v.y - g.size().y * 0.5), g.size() + vec2(18.0, 10.0));
        painter.rect_filled(r, CornerRadius::same(10), Color32::from_black_alpha(185));
        painter.galley(r.min + vec2(9.0, 5.0), g, Color32::WHITE);
        let _ = Align2::CENTER_CENTER;
    }
    Ok(())
}
