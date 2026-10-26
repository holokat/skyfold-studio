//! Turns a world plan into a playable world, a step at a time over frames. Each step is one of
//! the editor's own MCP tools (so everything made is ordinary, editable, undoable content) or a
//! planning step that looks at the land made so far and queues the next tools: towns go on flat,
//! dry lowland near water, dungeons by the hills, caves in hillsides, camps and creatures out in
//! the wilds; regions round the settlements become zones whose levels rise with the distance
//! from where players start.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::f32::consts::TAU;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use serde_json::{Value, json};

use super::Setup;
use super::plan::{GameKind, WorldPlan, WorldSize};
use crate::biomes::BiomePreset;
use crate::dungeons::geom;
use crate::mcp_tools::{McpContent, McpDeferred, dir_bearing};
use crate::terrain::Terrain;

/// The build's stages, in order (the wizard shows them with their progress).
pub const STAGES: [&str; 9] = [
    "Land & climates",
    "Rivers",
    "Towns & roads",
    "Zones & levels",
    "Dungeons & caves",
    "Camps & creatures",
    "Gathering",
    "Look & sky",
    "Checks",
];
const N: usize = STAGES.len();

/// What a finished call hands on: the next steps (run before the rest).
type Then = Box<dyn FnOnce(&mut World, &mut Ctx, &Value) -> Vec<Step> + Send + Sync>;
type PlanFn = fn(&mut World, &mut Ctx) -> Vec<Step>;

pub enum Step {
    /// one MCP tool; a `must` step that fails stops the build, others are logged and skipped
    Call {
        stage: usize,
        label: String,
        tool: &'static str,
        args: Value,
        then: Option<Then>,
        must: bool,
    },
    /// looks at the world made so far and queues the steps that follow from it
    Plan { stage: usize, f: PlanFn },
    /// lets the world settle (meshes, water levels) for a few frames
    Wait { stage: usize, frames: u32 },
}

impl Step {
    fn stage(&self) -> usize {
        match self {
            Step::Call { stage, .. } | Step::Plan { stage, .. } | Step::Wait { stage, .. } => {
                *stage
            }
        }
    }
}

fn call(stage: usize, tool: &'static str, label: impl Into<String>, args: Value) -> Step {
    Step::Call {
        stage,
        label: label.into(),
        tool,
        args,
        then: None,
        must: false,
    }
}

fn call_then(
    stage: usize,
    tool: &'static str,
    label: impl Into<String>,
    args: Value,
    then: impl FnOnce(&mut World, &mut Ctx, &Value) -> Vec<Step> + Send + Sync + 'static,
) -> Step {
    Step::Call {
        stage,
        label: label.into(),
        tool,
        args,
        then: Some(Box::new(then)),
        must: false,
    }
}

fn must(stage: usize, tool: &'static str, label: impl Into<String>, args: Value) -> Step {
    Step::Call {
        stage,
        label: label.into(),
        tool,
        args,
        then: None,
        must: true,
    }
}

/// A call that finishes in a later frame.
struct Pending {
    stage: usize,
    label: String,
    poll: McpDeferred,
    then: Option<Then>,
    must: bool,
}

/// The build running (or the last one's tour and start, kept for the flythrough and walk).
#[derive(Resource, Default)]
pub struct BuildRun {
    queue: VecDeque<Step>,
    ctx: Option<Ctx>,
    pending: Option<Pending>,
    wait: u32,
    started: Option<Instant>,
    total: [usize; N],
    done: [usize; N],
    /// places the flythrough visits: the start, the capital, towns, dungeons
    pub tour: Vec<[f32; 2]>,
    /// where players start and which way they face (bearing)
    pub start: Option<([f32; 2], f32)>,
    /// what was made where: (name, kind, [x, z])
    pub places: Vec<(String, &'static str, [f32; 2])>,
}

impl BuildRun {
    fn push_front(&mut self, steps: Vec<Step>) {
        for s in steps.into_iter().rev() {
            self.total[s.stage()] += 1;
            self.queue.push_front(s);
        }
    }
    fn push_back(&mut self, s: Step) {
        self.total[s.stage()] += 1;
        self.queue.push_back(s);
    }
}

// ------------------------------------------------------------------ what the build knows

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    City,
    Town,
    Village,
}

impl Kind {
    fn id(self) -> &'static str {
        match self {
            Kind::City => "city",
            Kind::Town => "town",
            Kind::Village => "village",
        }
    }
    /// Half the settlement's extent (m): how much flat ground it wants (the town generator's own).
    fn r(self) -> f32 {
        use crate::towns::kinds::TownSize;
        match self {
            Kind::City => TownSize::City,
            Kind::Town => TownSize::Town,
            Kind::Village => TownSize::Village,
        }
        .extent()
        .length()
    }
    fn index(self) -> usize {
        match self {
            Kind::Village => 0,
            Kind::Town => 1,
            Kind::City => 2,
        }
    }
}

struct Site {
    name: String,
    kind: Kind,
    pos: Vec2,
    realm: usize,
    /// the realm's seat (its capital)
    seat: bool,
    placed: bool,
}

struct Region {
    seed: Vec2,
    site: Option<usize>,
    /// None: the contested frontier between realms
    realm: Option<usize>,
    cells: usize,
    biome: Option<BiomePreset>,
    levels: [u32; 2],
    zone_type: &'static str,
    name: String,
    zone: Option<u32>,
}

/// The land cut into cells, each owned by a region (-1: sea).
struct Raster {
    n: usize,
    g: f32,
    half: f32,
    owner: Vec<i32>,
}

impl Raster {
    fn centre(&self, i: usize, j: usize) -> Vec2 {
        Vec2::new(
            -self.half + (i as f32 + 0.5) * self.g,
            -self.half + (j as f32 + 0.5) * self.g,
        )
    }
    fn owner_at(&self, p: Vec2) -> i32 {
        let i = ((p.x + self.half) / self.g).floor();
        let j = ((p.y + self.half) / self.g).floor();
        if i < 0.0 || j < 0.0 || i >= self.n as f32 || j >= self.n as f32 {
            return -1;
        }
        self.owner[j as usize * self.n + i as usize]
    }
}

pub struct Ctx {
    plan: WorldPlan,
    rng: fastrand::Rng,
    path: PathBuf,
    sea: f32,
    peak: f32,
    half: f32,
    centroid: Vec2,
    realm_angle: f32,
    rivers: Vec<Vec<Vec2>>,
    sites: Vec<Site>,
    /// per realm: the site players start by
    starters: Vec<usize>,
    start: Option<(Vec2, f32)>,
    regions: Vec<Region>,
    raster: Option<Raster>,
    dungeons: Vec<Vec2>,
    /// named places made (for the status and the tour)
    places: Vec<(String, &'static str, Vec2)>,
    caves: Vec<Vec2>,
    camps: Vec<Vec2>,
    spawns: Vec<Vec2>,
    used_names: HashSet<String>,
    town_names: VecDeque<String>,
    zone_names: VecDeque<String>,
    dungeon_names: VecDeque<String>,
    realm_names: Vec<String>,
    /// lines for the log, handed to the wizard each frame
    log: Vec<String>,
    report: Vec<String>,
    made: BTreeMap<&'static str, usize>,
}

impl Ctx {
    fn new(plan: WorldPlan, path: PathBuf) -> Self {
        let n = &plan.names;
        let keep = |v: &Vec<String>| {
            v.iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<VecDeque<_>>()
        };
        Self {
            rng: fastrand::Rng::with_seed(plan.seed as u64),
            town_names: keep(&n.towns),
            zone_names: keep(&n.zones),
            dungeon_names: keep(&n.dungeons),
            realm_names: keep(&n.realms).into(),
            plan,
            path,
            sea: 0.0,
            peak: 1.0,
            half: 1.0,
            centroid: Vec2::ZERO,
            realm_angle: 0.0,
            rivers: vec![],
            sites: vec![],
            starters: vec![],
            start: None,
            regions: vec![],
            raster: None,
            dungeons: vec![],
            places: vec![],
            caves: vec![],
            camps: vec![],
            spawns: vec![],
            used_names: HashSet::new(),
            log: vec![],
            report: vec![],
            made: BTreeMap::new(),
        }
    }

    fn note(&mut self, s: impl Into<String>) {
        self.log.push(s.into());
    }

    fn count(&mut self, what: &'static str) {
        *self.made.entry(what).or_default() += 1;
    }

    fn river_dist(&self, p: Vec2) -> f32 {
        self.rivers
            .iter()
            .map(|r| line_dist(r, p))
            .fold(f32::INFINITY, f32::min)
    }

    fn site_dist(&self, p: Vec2) -> f32 {
        self.sites
            .iter()
            .filter(|s| s.placed)
            .map(|s| s.pos.distance(p) - s.kind.r())
            .fold(f32::INFINITY, f32::min)
    }

    /// Distance to the nearest of `pts` (∞ when there are none).
    fn near(pts: &[Vec2], p: Vec2) -> f32 {
        pts.iter()
            .map(|q| q.distance(p))
            .fold(f32::INFINITY, f32::min)
    }

    fn sector(&self, p: Vec2) -> usize {
        let realms = self.plan.realms.max(1) as usize;
        if realms == 1 {
            return 0;
        }
        let d = p - self.centroid;
        let a = (d.y.atan2(d.x) - self.realm_angle).rem_euclid(TAU);
        ((a / (TAU / realms as f32)) as usize).min(realms - 1)
    }

    fn unique(&mut self, name: String) -> Option<String> {
        let key = name.to_lowercase();
        if self.used_names.contains(&key) {
            return None;
        }
        self.used_names.insert(key);
        Some(name)
    }

    /// A made-up place name (Ashford, Ravenmere…), never used twice.
    fn place_name(&mut self) -> String {
        const A: &[&str] = &[
            "Ash", "Bright", "Cold", "Elder", "Fair", "Glen", "Hart", "Iron", "Kings", "Lark",
            "Mill", "North", "Oak", "Raven", "Stone", "Thorn", "West", "Wolf", "Brook", "Dun",
            "Ember", "Frost", "Gold", "Holl", "Mar", "Red", "Silver", "Wynd", "Amber", "Briar",
            "Crow", "Dawn", "Fern", "Grey", "Moor", "Rook", "Sable", "Storm", "Willow", "Hawk",
        ];
        const B: &[&str] = &[
            "ford", "haven", "mere", "stead", "wick", "bury", "dale", "hold", "march", "moor",
            "reach", "vale", "watch", "well", "fall", "gate", "ton", "shire", "holm", "crest",
            "barrow", "fen", "garde", "rest",
        ];
        for _ in 0..80 {
            let a = A[self.rng.usize(..A.len())];
            let b = B[self.rng.usize(..B.len())];
            if a.to_lowercase().ends_with(&b[..1]) {
                continue;
            }
            if let Some(n) = self.unique(format!("{a}{b}")) {
                return n;
            }
        }
        format!("Hamlet {}", self.rng.u32(10..999))
    }

    fn town_name(&mut self) -> String {
        while let Some(n) = self.town_names.pop_front() {
            if let Some(n) = self.unique(n) {
                return n;
            }
        }
        self.place_name()
    }

    fn zone_name(&mut self, site: Option<&str>, biome: Option<BiomePreset>) -> String {
        while let Some(n) = self.zone_names.pop_front() {
            if let Some(n) = self.unique(n) {
                return n;
            }
        }
        const P: &[&str] = &[
            "Ashen", "Silver", "Gloam", "Thorn", "Amber", "Mist", "Iron", "Golden", "Hollow",
            "Briar", "Stone", "Frost", "Sun", "Raven", "Whisper", "Elder", "Wild", "Crimson",
            "Shiver", "Dusk", "Bramble", "Storm", "Moon", "Old",
        ];
        let sufs = suffixes(biome);
        for _ in 0..60 {
            let s = sufs[self.rng.usize(..sufs.len())];
            let name = match site {
                Some(t) => format!("{t} {s}"),
                None => format!("{} {s}", P[self.rng.usize(..P.len())]),
            };
            if let Some(n) = self.unique(name) {
                return n;
            }
        }
        format!("{} Wilds", self.place_name())
    }

    fn dungeon_name(&mut self, layout: &str) -> String {
        while let Some(n) = self.dungeon_names.pop_front() {
            if let Some(n) = self.unique(n) {
                return n;
            }
        }
        const ADJ: &[&str] = &[
            "Sunken",
            "Forgotten",
            "Shattered",
            "Hollow",
            "Ashen",
            "Silent",
            "Drowned",
            "Black",
            "Weeping",
            "Iron",
            "Gilded",
            "Howling",
            "Ember",
            "Frozen",
            "Thorned",
            "Ruined",
            "Whispering",
            "Bleak",
        ];
        let noun = match layout {
            "crypt" => "Crypt",
            "catacombs" => "Catacombs",
            "prison_keep" => "Keep",
            "sun_temple" => "Temple",
            "cathedral" => "Cathedral",
            "dwarven_halls" => "Forge",
            "underground_city" => "Undercity",
            "great_caverns" => "Deeps",
            "goblin_cave" => "Warren",
            "spider_den" => "Nest",
            "sewers" => "Sewers",
            "mine" => "Mine",
            "clockwork" => "Works",
            "frost_halls" => "Halls",
            "infernal" => "Pit",
            "ruined_fortress" => "Bastion",
            _ => "Vaults",
        };
        for _ in 0..60 {
            let adj = ADJ[self.rng.usize(..ADJ.len())];
            if let Some(n) = self.unique(format!("The {adj} {noun}")) {
                return n;
            }
        }
        format!("The {} {noun}", self.place_name())
    }

    fn realm_name(&mut self, r: usize) -> String {
        while self.realm_names.len() <= r {
            let n = self.place_name();
            self.realm_names.push(n);
        }
        self.realm_names[r].clone()
    }

    fn region_of(&self, p: Vec2) -> Option<usize> {
        let o = self.raster.as_ref()?.owner_at(p);
        (o >= 0).then_some(o as usize)
    }

    fn levels_at(&self, p: Vec2) -> [u32; 2] {
        match self.region_of(p) {
            Some(r) => self.regions[r].levels,
            None => self.plan.levels,
        }
    }

    fn start_region(&self) -> Option<usize> {
        self.start.and_then(|(p, _)| self.region_of(p))
    }

    /// Regions in level order (lowest first), cities and the start left out when others exist.
    fn wild_order(&self) -> Vec<usize> {
        let start = self.start_region();
        let mut v: Vec<usize> = (0..self.regions.len())
            .filter(|&i| self.regions[i].zone_type != "city" && Some(i) != start)
            .collect();
        if v.is_empty() {
            v = (0..self.regions.len()).collect();
        }
        v.sort_by_key(|&i| (self.regions[i].levels[0], self.regions[i].levels[1]));
        v
    }
}

fn suffixes(b: Option<BiomePreset>) -> &'static [&'static str] {
    use BiomePreset::*;
    match b {
        Some(TemperateForest) => &["Woods", "Forest", "Weald"],
        Some(Meadow) => &["Downs", "Fields", "Vale", "Meadows"],
        Some(Taiga) => &["Pinewood", "Timberland", "Firs"],
        Some(AutumnWoods) => &["Woods", "Hollows", "Glades"],
        Some(Jungle) => &["Jungle", "Thicket", "Wilds"],
        Some(TropicalCoast) => &["Coast", "Shores", "Strand"],
        Some(Desert) => &["Wastes", "Sands", "Dunes", "Barrens"],
        Some(Alpine) => &["Peaks", "Heights", "Crags"],
        Some(Tundra) => &["Tundra", "Frostlands", "Steppe"],
        Some(Swamp) => &["Marsh", "Mire", "Fen", "Bog"],
        Some(Volcanic) => &["Ashlands", "Cinders", "Scorch"],
        Some(HauntedWoods) => &["Gloom", "Blightwood", "Shadows"],
        None => &["Reach", "Wilds", "Lands"],
    }
}

// ------------------------------------------------------------------ geometry helpers

fn h(t: &Terrain, p: Vec2) -> f32 {
    t.height_at(p.x, p.y)
}

/// How level the ground is (1: flat), right there (over a terrain cell).
fn up(t: &Terrain, p: Vec2) -> f32 {
    t.normal_at(p.x, p.y).y
}

/// The ground's slope (rise over run) across ~24 m: small bumps don't count.
fn grade(t: &Terrain, p: Vec2) -> f32 {
    let e = 12.0;
    let dx = (h(t, p + Vec2::X * e) - h(t, p - Vec2::X * e)) / (2.0 * e);
    let dz = (h(t, p + Vec2::Y * e) - h(t, p - Vec2::Y * e)) / (2.0 * e);
    (dx * dx + dz * dz).sqrt()
}

/// Inside the world with `margin` m to spare.
fn inside(ctx: &Ctx, p: Vec2, margin: f32) -> bool {
    p.x.abs() < ctx.half - margin && p.y.abs() < ctx.half - margin
}

fn seg_dist(a: Vec2, b: Vec2, p: Vec2) -> f32 {
    let ab = b - a;
    let u = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * u)
}

fn line_dist(line: &[Vec2], p: Vec2) -> f32 {
    match line {
        [] => f32::INFINITY,
        [a] => a.distance(p),
        _ => line
            .windows(2)
            .map(|w| seg_dist(w[0], w[1], p))
            .fold(f32::INFINITY, f32::min),
    }
}

fn line_len(line: &[Vec2]) -> f32 {
    line.windows(2).map(|w| w[0].distance(w[1])).sum()
}

fn xz(p: Vec2) -> [f32; 2] {
    [p.x.round(), p.y.round()]
}

/// The first JSON a tool answered with (text answers as a string).
fn answer(out: &[McpContent]) -> Value {
    for c in out {
        match c {
            McpContent::Json(v) => return v.clone(),
            McpContent::Text(t) => return Value::String(t.clone()),
            _ => {}
        }
    }
    Value::Null
}

/// The folder the new world is saved in (scratch runs keep to their own folder).
fn world_dir(name: &str) -> PathBuf {
    let base = match std::env::var("WB_RECOVERY_DIR") {
        Ok(d) => PathBuf::from(d),
        Err(_) => crate::io::worlds_dir(),
    };
    let stem = crate::io::sanitize(name);
    let mut p = base.join(format!("{stem}.world"));
    let mut k = 2;
    while p.exists() {
        p = base.join(format!("{stem}_{k}.world"));
        k += 1;
    }
    p
}

// ------------------------------------------------------------------ running

/// Starts building `plan` (the open world is replaced).
pub fn start(world: &mut World, plan: WorldPlan) {
    if world.resource::<BuildRun>().ctx.is_some() {
        return;
    }
    let plan = plan.normalized();
    world
        .resource_mut::<crate::prompt::selection::Selection>()
        .clear();
    let path = world_dir(&plan.name);
    let p = &plan;
    let mut run = BuildRun::default();
    run.push_back(Step::Call {
        stage: 0,
        label: format!("Raised {} ({} km), {}", p.land.name().to_lowercase(), p.size.km(), p.climate.name().to_lowercase()),
        tool: "world_generate_continent",
        args: json!({ "preset": p.land.preset(), "seed": p.seed % 1_000_000, "new_world": true, "size_km": p.size.km(), "cell": 4, "biomes": true, "climate": p.climate.id() }),
        then: None,
        must: true,
    });
    run.push_back(Step::Plan {
        stage: 0,
        f: after_land,
    });
    run.push_back(Step::Plan {
        stage: 1,
        f: plan_rivers,
    });
    run.push_back(Step::Plan {
        stage: 2,
        f: plan_settlements,
    });
    run.push_back(Step::Plan {
        stage: 3,
        f: plan_regions,
    });
    run.push_back(Step::Plan {
        stage: 4,
        f: plan_dungeons,
    });
    run.push_back(Step::Plan {
        stage: 4,
        f: plan_caves,
    });
    run.push_back(Step::Plan {
        stage: 5,
        f: plan_camps,
    });
    run.push_back(Step::Plan {
        stage: 5,
        f: plan_creatures,
    });
    run.push_back(Step::Plan {
        stage: 6,
        f: plan_gathering,
    });
    let scifi = p.kind == crate::world_kind::WorldKind::SciFi;
    if !scifi {
        run.push_back(call(
            7,
            "look_set",
            format!("The look: {}", p.look.replace('_', " ")),
            json!({ "preset": p.look, "apply_scene": false }),
        ));
    }
    // (again after the look: an untouched Natural look gets the painted baseline in hand-painted worlds)
    run.push_back(call(
        7,
        "world_set_art_style",
        format!("{} look", p.art.name()),
        json!({ "style": p.art.id() }),
    ));
    run.push_back(call(
        7,
        "lighting_set",
        "The time of day",
        json!({ "hour": p.hour }),
    ));
    if scifi {
        // the alien template over the finished land (its canyons would cut through the towns
        // and roads, so the land keeps its shape): alien ground, spires and floating rocks,
        // alien scrub, then the chosen Sci-Fi look (its sky, air and plants) at the chosen hour
        run.push_back(call(
            7,
            "world_kind_apply",
            "Sci-Fi: rock spires, floating rocks, monoliths and alien scrub",
            json!({ "kind": "scifi", "parts": ["props", "flora"] }),
        ));
        run.push_back(call(
            7,
            "terrain_ground_palette",
            "Alien ground: red dust, ochre sand, layered sandstone",
            json!({ "palette": "scifi" }),
        ));
        let look = crate::world_kind::looks::ScifiLook::from_id(&p.scifi_look)
            .unwrap_or(crate::world_kind::looks::ScifiLook::AlienDusk);
        run.push_back(call(
            7,
            "look_scifi_set",
            format!("The look: {}", look.name()),
            json!({ "look": look.id() }),
        ));
        run.push_back(call(
            7,
            "lighting_set",
            "The time of day",
            json!({ "hour": p.hour }),
        ));
    }
    run.push_back(call(
        7,
        "weather_set",
        "Weather follows the land (rain in jungles, snow on the tundra)",
        json!({ "weather": "follow_zones" }),
    ));
    run.push_back(Step::Plan {
        stage: 8,
        f: plan_checks,
    });
    run.started = Some(Instant::now());
    run.ctx = Some(Ctx::new(plan.clone(), path));
    *world.resource_mut::<BuildRun>() = run;
    let mut s = world.resource_mut::<Setup>();
    s.build = Some(super::BuildState {
        stages: STAGES.iter().map(|n| (n.to_string(), 0.0)).collect(),
        current: 0,
        log: vec![format!("Building {}", plan.name)],
        done: false,
        cancelled: false,
        failed: None,
        report: vec![],
        elapsed: 0.0,
        plan,
    });
}

/// Stops the build; what's made so far stays.
pub fn cancel(world: &mut World) {
    let mut run = world.resource_mut::<BuildRun>();
    if run.ctx.is_none() {
        return;
    }
    run.queue.clear();
    run.pending = None;
    run.ctx = None;
    if let Some(b) = world.resource_mut::<Setup>().build.as_mut() {
        b.cancelled = true;
        b.done = true;
        b.log
            .push("Stopped — what was made so far stays (undo removes it step by step)".into());
    }
}

/// Runs the build's next steps (about 20 ms a frame; a slow tool takes its own time).
pub fn tick(world: &mut World) {
    if world.resource::<BuildRun>().ctx.is_none() {
        return;
    }
    let mut run = std::mem::take(&mut *world.resource_mut::<BuildRun>());
    let mut ctx = run.ctx.take().expect("a build's context");
    let t0 = Instant::now();
    let mut failed: Option<String> = None;
    if let Some(mut p) = run.pending.take() {
        match (p.poll.0)(world) {
            None => run.pending = Some(p),
            Some(res) => {
                run.done[p.stage] += 1;
                if let Err(e) = finished(
                    world,
                    &mut run,
                    &mut ctx,
                    p.label,
                    p.then.take(),
                    p.must,
                    res,
                ) {
                    failed = Some(e);
                }
            }
        }
    }
    while failed.is_none() && run.pending.is_none() && t0.elapsed() < Duration::from_millis(20) {
        if run.wait > 0 {
            run.wait -= 1;
            break;
        }
        let Some(step) = run.queue.pop_front() else {
            break;
        };
        let stage = step.stage();
        match step {
            Step::Wait { frames, .. } => {
                run.done[stage] += 1;
                run.wait = frames;
            }
            Step::Plan { f, .. } => {
                run.done[stage] += 1;
                let next = f(world, &mut ctx);
                run.push_front(next);
            }
            Step::Call {
                label,
                tool,
                args,
                then,
                must,
                ..
            } => {
                let res = crate::mcp::call_tool(world, tool, args);
                // a call finishing later is polled in the next frames
                if let Ok(out) = &res
                    && out.iter().any(|c| matches!(c, McpContent::Deferred(_)))
                {
                    let poll = res
                        .unwrap()
                        .into_iter()
                        .find_map(|c| {
                            if let McpContent::Deferred(d) = c {
                                Some(d)
                            } else {
                                None
                            }
                        })
                        .expect("a deferred answer");
                    run.pending = Some(Pending {
                        stage,
                        label,
                        poll,
                        then,
                        must,
                    });
                    break;
                }
                run.done[stage] += 1;
                if let Err(e) = finished(world, &mut run, &mut ctx, label, then, must, res) {
                    failed = Some(e);
                }
                // a heavy tool (the land, a town, a road) gets a frame to itself
                if t0.elapsed() > Duration::from_millis(8) {
                    break;
                }
            }
        }
    }
    let over = failed.is_some() || (run.queue.is_empty() && run.pending.is_none() && run.wait == 0);
    // the stage being worked on
    let cur = run
        .pending
        .as_ref()
        .map(|p| p.stage)
        .or(run.queue.front().map(|s| s.stage()))
        .unwrap_or(N - 1);
    let elapsed = run.started.map_or(0.0, |s| s.elapsed().as_secs_f32());
    let lines = std::mem::take(&mut ctx.log);
    for l in &lines {
        info!("SETUP {l}");
    }
    if over {
        run.tour = tour(&ctx);
        run.start = ctx.start.map(|(p, yaw)| (xz(p), yaw));
        run.places = ctx
            .places
            .iter()
            .map(|(n, k, p)| (n.clone(), *k, xz(*p)))
            .collect();
        if failed.is_none() {
            frame_start(world, &ctx);
        }
    }
    let report = if over {
        Some(std::mem::take(&mut ctx.report))
    } else {
        None
    };
    if !over {
        run.ctx = Some(ctx);
    } else {
        run.queue.clear();
        run.pending = None;
    }
    let (total, done) = (run.total, run.done);
    *world.resource_mut::<BuildRun>() = run;
    let mut s = world.resource_mut::<Setup>();
    let Some(b) = s.build.as_mut() else { return };
    b.log.extend(lines);
    b.elapsed = elapsed;
    b.current = cur;
    for (i, st) in b.stages.iter_mut().enumerate() {
        st.1 = if over || i < cur {
            1.0
        } else if i == cur && total[i] > 0 {
            (done[i] as f32 / total[i] as f32).min(0.99)
        } else {
            0.0
        };
    }
    if over {
        b.done = true;
        if let Some(e) = failed {
            b.log.push(format!("Stopped: {e}"));
            b.failed = Some(e);
        } else {
            b.log
                .push(format!("{} is ready ({:.0} s)", b.plan.name, elapsed));
        }
        if let Some(r) = report {
            b.report = r;
        }
    }
}

/// A call's answer: logged, its follow-up steps queued. Err stops the build.
fn finished(
    world: &mut World,
    run: &mut BuildRun,
    ctx: &mut Ctx,
    label: String,
    then: Option<Then>,
    must: bool,
    res: Result<Vec<McpContent>, String>,
) -> Result<(), String> {
    match res {
        Ok(out) => {
            ctx.note(label);
            if let Some(f) = then {
                let v = answer(&out);
                let next = f(world, ctx, &v);
                run.push_front(next);
            }
            Ok(())
        }
        Err(e) if must => Err(format!("{label}: {e}")),
        Err(e) => {
            let e = e.lines().next().unwrap_or_default().to_string();
            warn!("guided setup skipped {label}: {e}");
            ctx.note(format!("Skipped — {label}: {e}"));
            Ok(())
        }
    }
}

/// Places the flythrough visits: the start, the capital and towns, dungeons.
fn tour(ctx: &Ctx) -> Vec<[f32; 2]> {
    let mut v: Vec<[f32; 2]> = vec![];
    if let Some((p, _)) = ctx.start {
        v.push(xz(p));
    }
    let mut rest: Vec<Vec2> = ctx
        .sites
        .iter()
        .filter(|s| s.placed && s.kind != Kind::Village)
        .take(4)
        .map(|s| s.pos)
        .chain(ctx.dungeons.iter().take(2).copied())
        .collect();
    // nearest next, so the flight doesn't zigzag
    let mut at = ctx
        .start
        .map(|s| s.0)
        .or(rest.first().copied())
        .unwrap_or_default();
    while !rest.is_empty() {
        let k = (0..rest.len())
            .min_by(|a, b| rest[*a].distance(at).total_cmp(&rest[*b].distance(at)))
            .unwrap();
        at = rest.remove(k);
        v.push(xz(at));
    }
    v
}

/// The camera glides to where players start.
fn frame_start(world: &mut World, ctx: &Ctx) {
    let Some(at) = ctx
        .start
        .map(|s| s.0)
        .or_else(|| ctx.sites.iter().find(|s| s.placed).map(|s| s.pos))
    else {
        return;
    };
    let y = world.resource::<Terrain>().height_at(at.x, at.y);
    world.resource_mut::<crate::camera::OrbitCamera>().set_view(
        Vec3::new(at.x, y, at.y),
        0.35,
        0.55,
        520.0,
    );
}

/// Flies the camera through the finished world (start, capital, towns, dungeons).
pub fn flythrough(world: &mut World) {
    let mut places = world.resource::<BuildRun>().tour.clone();
    if places.is_empty() {
        places = world
            .resource::<crate::gameplay::Gameplay>()
            .pois
            .iter()
            .take(6)
            .map(|p| [p.pos[0], p.pos[2]])
            .collect();
    }
    if places.is_empty() {
        return;
    }
    let name = format!("{} tour", world.resource::<crate::world::WorldMeta>().name);
    let res = crate::mcp::call_tool(
        world,
        "camera_path_from_places",
        json!({ "places": places, "style": "showcase", "speed": "normal", "name": name }),
    )
    .and_then(|_| crate::mcp::call_tool(world, "camera_path_play", json!({})));
    if let Err(e) = res {
        warn!("SETUP flythrough: {e}");
    }
}

/// Shows a place the build made: "start", "capital" (else the first town) or a place's name,
/// optionally from `view` (distance m, pitch and turn in degrees). Returns its name.
pub fn goto(world: &mut World, place: &str, view: [Option<f32>; 3]) -> Result<String, String> {
    let run = world.resource::<BuildRun>();
    let want = place.trim().to_lowercase();
    let (name, kind, at) = if want == "start" {
        let (p, _) = run.start.ok_or("no player start was made")?;
        ("where players start".to_string(), "start", p)
    } else {
        let found = if want == "capital" {
            run.places
                .iter()
                .find(|p| p.1 == "city")
                .or(run.places.iter().find(|p| p.1 == "town"))
        } else {
            run.places
                .iter()
                .find(|p| p.0.to_lowercase() == want)
                .or(run
                    .places
                    .iter()
                    .find(|p| p.0.to_lowercase().contains(&want)))
        };
        let (n, k, p) = found
            .ok_or_else(|| format!("no place called `{place}` (world_setup_status lists them)"))?;
        (n.clone(), *k, *p)
    };
    let distance = match kind {
        "city" => 430.0,
        "town" => 330.0,
        "village" => 250.0,
        "start" => 160.0,
        _ => 140.0,
    };
    let [d, pitch, turn] = view;
    let y = world.resource::<Terrain>().height_at(at[0], at[1]);
    let (yaw, pitch, distance) = (
        turn.map_or(0.6, f32::to_radians),
        pitch.map_or(0.42, |p| p.clamp(3.0, 89.0).to_radians()),
        d.unwrap_or(distance).clamp(20.0, 6000.0),
    );
    world.resource_mut::<crate::camera::OrbitCamera>().set_view(
        Vec3::new(at[0], y, at[1]),
        yaw,
        pitch,
        distance,
    );
    Ok(name)
}

/// Walks into the finished world where players start.
pub fn walk(world: &mut World) {
    let args = match world.resource::<BuildRun>().start {
        Some((p, yaw)) => json!({ "position": p, "yaw_deg": yaw }),
        None => json!({}),
    };
    if let Err(e) = crate::mcp::call_tool(world, "walk_start", args) {
        warn!("SETUP walk: {e}");
    }
}

// ------------------------------------------------------------------ land

fn after_land(world: &mut World, ctx: &mut Ctx) -> Vec<Step> {
    // `world_generate_continent` starts a fresh Gameplay resource. The guided plan's normalized
    // upper level is an explicit progression choice, so restore it before any later build steps.
    world.resource_mut::<crate::gameplay::Gameplay>().level_cap =
        Some(ctx.plan.levels[1].clamp(1, 200));
    let t = world.resource::<Terrain>();
    ctx.sea = t.sea_level;
    ctx.half = t.half();
    let (mut sum, mut n, mut peak) = (Vec2::ZERO, 0.0, t.sea_level + 1.0);
    let k = 128;
    for j in 0..k {
        for i in 0..k {
            let p = Vec2::new(
                -ctx.half + (i as f32 + 0.5) / k as f32 * 2.0 * ctx.half,
                -ctx.half + (j as f32 + 0.5) / k as f32 * 2.0 * ctx.half,
            );
            let y = h(t, p);
            if y > t.sea_level {
                sum += p;
                n += 1.0;
                peak = peak.max(y);
            }
        }
    }
    ctx.centroid = if n > 0.0 { sum / n } else { Vec2::ZERO };
    ctx.peak = peak;
    ctx.realm_angle = ctx.rng.f32() * TAU;
    let land = n / (k * k) as f32 * 100.0;
    ctx.note(format!(
        "Land covers {land:.0}% of the map; the highest peak rises {:.0} m",
        peak - ctx.sea
    ));
    let path = ctx.path.to_string_lossy().into_owned();
    // the world's art (a new world starts realistic): towns, dungeons and nature follow it
    let art = ctx.plan.art;
    vec![
        call(
            0,
            "world_set_art_style",
            format!("{} art: ground, trees and buildings", art.name()),
            json!({ "style": art.id() }),
        ),
        Step::Wait {
            stage: 0,
            frames: 3,
        },
        must(
            0,
            "world_save",
            format!(
                "Saved as {}",
                ctx.path
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
            json!({ "path": path }),
        ),
    ]
}

// ------------------------------------------------------------------ rivers

fn plan_rivers(world: &mut World, ctx: &mut Ctx) -> Vec<Step> {
    if !ctx.plan.rivers {
        return vec![];
    }
    let want = match ctx.plan.size {
        WorldSize::Small => 1,
        WorldSize::Medium => 3,
        WorldSize::Large => 5,
    };
    let t = world.resource::<Terrain>();
    let size = t.size();
    let mut found: Vec<Vec<Vec2>> = vec![];
    let (mut starts, mut short) = (0, 0);
    for _ in 0..400 {
        if found.len() >= want {
            break;
        }
        let p = Vec2::new(
            (ctx.rng.f32() * 2.0 - 1.0) * (ctx.half - 120.0),
            (ctx.rng.f32() * 2.0 - 1.0) * (ctx.half - 120.0),
        );
        let rel = (h(t, p) - ctx.sea) / (ctx.peak - ctx.sea).max(1.0);
        if !(0.3..0.85).contains(&rel) || grade(t, p) > 1.2 {
            continue;
        }
        if found.iter().any(|r| line_dist(r, p) < size * 0.1) {
            continue;
        }
        starts += 1;
        let Some((pts, to_sea)) = trace_river(t, p, ctx.sea, &found) else {
            continue;
        };
        let len = line_len(&pts);
        if len < size * 0.12 || (!to_sea && len < size * 0.2) {
            short += 1;
            continue;
        }
        found.push(pts);
    }
    info!(
        "SETUP rivers: {} found from {starts} sources ({short} too short)",
        found.len()
    );
    let mut out = vec![];
    for pts in found {
        let len = line_len(&pts);
        let width = (10.0 + 10.0 * (len / 3000.0).min(1.0)).round();
        let name = format!("{} River", ctx.place_name());
        let thin: Vec<[f32; 2]> = pts
            .iter()
            .step_by(2)
            .chain(pts.last())
            .map(|p| xz(*p))
            .collect();
        out.push(call(
            1,
            "water_add_river",
            format!("{name}, {:.1} km", len / 1000.0),
            json!({ "points": thin, "width": width, "depth": 3.5, "name": name }),
        ));
        ctx.rivers.push(pts);
    }
    if out.is_empty() {
        ctx.note("No river found a good way down to the sea");
    }
    out
}

/// Follows the land downhill from `start` (24 m steps, gently bending) to the sea, another
/// river or a basin. Some(points, reached the sea or a river).
fn trace_river(
    t: &Terrain,
    start: Vec2,
    sea: f32,
    others: &[Vec<Vec2>],
) -> Option<(Vec<Vec2>, bool)> {
    let step = 24.0;
    let (mut p, mut hp) = (start, h(t, start));
    let mut pts = vec![p];
    let mut dir: Option<Vec2> = None;
    let mut climbs = 0;
    for _ in 0..(t.size() * 1.5 / step) as usize {
        let mut best: Option<(f32, Vec2)> = None;
        for reach in [1.0, 2.0, 4.0] {
            for k in 0..24 {
                let d = Vec2::from_angle(k as f32 / 24.0 * TAU);
                if dir.is_some_and(|pd| d.dot(pd) < 0.0) {
                    continue;
                }
                let q = p + d * step * reach;
                if !t.in_bounds(q.x, q.y) {
                    continue;
                }
                let s = h(t, q) + dir.map_or(0.0, |pd| (1.0 - d.dot(pd)) * 0.4);
                if best.is_none_or(|b| s < b.0) {
                    best = Some((s, q));
                }
            }
            if best.is_some_and(|b| h(t, b.1) < hp - 0.05) {
                break;
            }
        }
        let (_, q) = best?;
        let hq = h(t, q);
        if hq >= hp - 0.05 {
            climbs += 1;
            if climbs > 6 || hq > hp + 2.0 {
                return (pts.len() >= 2).then_some((pts, false));
            }
        }
        if pts.iter().rev().skip(3).any(|o| o.distance(q) < step * 0.8) {
            return (pts.len() >= 2).then_some((pts, false));
        }
        dir = Some((q - p).normalize_or_zero());
        pts.push(q);
        if hq < sea - 0.5
            || others
                .iter()
                .any(|r| r.iter().any(|o| o.distance(q) < 50.0))
        {
            return Some((pts, true));
        }
        p = q;
        hp = hq;
    }
    Some((pts, false))
}

// ------------------------------------------------------------------ settlements and roads

/// How well a settlement of half-extent `r` fits at `c` (None: it doesn't: water, steep or
/// uneven ground, a river through it, up a mountain).
fn site_score(t: &Terrain, ctx: &Ctx, c: Vec2, r: f32) -> Option<f32> {
    let hc = h(t, c);
    if !inside(ctx, c, r + 30.0)
        || hc < ctx.sea + 2.0
        || hc > ctx.sea + 0.45 * (ctx.peak - ctx.sea)
        || grade(t, c) > 0.2
    {
        return None;
    }
    let (mut lo, mut hi) = (hc, hc);
    for ring in [0.45, 0.9] {
        for k in 0..8 {
            let q = c + Vec2::from_angle(k as f32 * TAU / 8.0 + ring) * r * ring;
            let hq = h(t, q);
            if hq < ctx.sea + 1.0 || grade(t, q) > 0.28 {
                return None;
            }
            lo = lo.min(hq);
            hi = hi.max(hq);
        }
    }
    let range = hi - lo;
    if range > r * 0.12 + 4.0 {
        return None;
    }
    let river = ctx.river_dist(c);
    if river < r * 0.8 + 15.0 {
        return None;
    }
    let mut s = -range * 0.4;
    if river < r * 3.0 {
        s += 6.0;
    }
    if (0..8).any(|k| {
        let q = c + Vec2::from_angle(k as f32 * TAU / 8.0) * r * 2.0;
        t.in_bounds(q.x, q.y) && h(t, q) < ctx.sea
    }) {
        s += 4.0;
    }
    Some(s)
}

struct Cand {
    p: Vec2,
    /// score as a village, town, city (None: too small for it)
    fit: [Option<f32>; 3],
}

fn plan_settlements(world: &mut World, ctx: &mut Ctx) -> Vec<Step> {
    let p = ctx.plan.clone();
    let realms = p.realms.max(1) as usize;
    let cands = {
        let t = world.resource::<Terrain>();
        let size = t.size();
        let step = (size / 72.0).max(40.0);
        let n = (size / step) as i32;
        let mut v = vec![];
        for j in 0..n {
            for i in 0..n {
                let jitter = Vec2::new(ctx.rng.f32() - 0.5, ctx.rng.f32() - 0.5) * step * 0.6;
                let c = Vec2::new(
                    -ctx.half + (i as f32 + 0.5) * step,
                    -ctx.half + (j as f32 + 0.5) * step,
                ) + jitter;
                let mut fit = [None; 3];
                for (k, kind) in [Kind::Village, Kind::Town, Kind::City]
                    .into_iter()
                    .enumerate()
                {
                    fit[k] = site_score(t, ctx, c, kind.r());
                    if fit[k].is_none() {
                        break;
                    }
                }
                if fit[0].is_some() {
                    v.push(Cand { p: c, fit });
                }
            }
        }
        v
    };
    let fits = |k: usize| cands.iter().filter(|c| c.fit[k].is_some()).count();
    info!(
        "SETUP settlement spots: {} village, {} town, {} city",
        fits(0),
        fits(1),
        fits(2)
    );
    let size = ctx.half * 2.0;
    let all = (p.towns + p.villages + 1) as f32;
    let town_gap = (size / all.sqrt() * 0.45).max(300.0);
    let village_gap = (town_gap * 0.55).max(220.0);
    // the best spot for a `kind` (in realm `realm`, if it has room), clear of the others
    let pick = |ctx: &Ctx,
                kind: Kind,
                realm: Option<usize>,
                gap: f32,
                extra: &dyn Fn(Vec2) -> f32|
     -> Option<Vec2> {
        cands
            .iter()
            .filter(|c| c.fit[kind.index()].is_some())
            .filter(|c| realm.is_none_or(|r| ctx.sector(c.p) == r))
            .filter(|c| {
                ctx.sites
                    .iter()
                    .all(|s| s.pos.distance(c.p) > s.kind.r() + kind.r() + gap)
            })
            .map(|c| (c.fit[kind.index()].unwrap() + extra(c.p), c.p))
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|x| x.1)
    };
    let centroid = ctx.centroid;
    // the seats: the capital (one realm, near the middle) or each realm's capital (deep in its homeland)
    let mut seats_used = 0;
    for r in 0..realms {
        let kind = if r == 0 && p.capital {
            Kind::City
        } else if realms > 1 {
            Kind::Town
        } else {
            continue;
        };
        let lean = if realms == 1 { -25.0 } else { 25.0 };
        let extra = move |q: Vec2| lean * q.distance(centroid) / size;
        let spot = {
            let c: &Ctx = ctx;
            pick(c, kind, Some(r), town_gap, &extra)
                .map(|q| (q, kind))
                .or_else(|| pick(c, Kind::Town, Some(r), town_gap, &extra).map(|q| (q, Kind::Town)))
        };
        let Some((q, kind)) = spot else { continue };
        let name = if r == 0 && p.capital && !p.names.capital.trim().is_empty() {
            ctx.unique(p.names.capital.trim().to_string())
                .unwrap_or_else(|| ctx.place_name())
        } else {
            ctx.town_name()
        };
        if r > 0 {
            seats_used += 1;
        }
        ctx.sites.push(Site {
            name,
            kind,
            pos: q,
            realm: r,
            seat: true,
            placed: false,
        });
    }
    for k in 0..p.towns.saturating_sub(seats_used) as usize {
        let r = k % realms;
        let jiggle = ctx.rng.f32() * 2.0;
        let extra = move |_: Vec2| jiggle;
        let spot = {
            let c: &Ctx = ctx;
            pick(c, Kind::Town, Some(r), town_gap, &extra)
                .or_else(|| pick(c, Kind::Town, None, town_gap, &extra))
        };
        let Some(q) = spot else { break };
        let name = ctx.town_name();
        let realm = ctx.sector(q);
        ctx.sites.push(Site {
            name,
            kind: Kind::Town,
            pos: q,
            realm,
            seat: false,
            placed: false,
        });
    }
    for k in 0..p.villages as usize {
        let r = k % realms;
        // each realm's first village is its starting village, near the seat
        let seat = ctx
            .sites
            .iter()
            .find(|s| s.seat && s.realm == r)
            .map(|s| s.pos);
        let first = k < realms;
        let jiggle = ctx.rng.f32() * 2.0;
        let extra = move |q: Vec2| match (first, seat) {
            (true, Some(s)) => -40.0 * q.distance(s) / size,
            _ => jiggle,
        };
        let spot = {
            let c: &Ctx = ctx;
            pick(c, Kind::Village, Some(r), village_gap, &extra)
                .or_else(|| pick(c, Kind::Village, None, village_gap, &extra))
        };
        let Some(q) = spot else { break };
        let name = ctx.town_name();
        let realm = ctx.sector(q);
        ctx.sites.push(Site {
            name,
            kind: Kind::Village,
            pos: q,
            realm,
            seat: false,
            placed: false,
        });
    }
    if ctx.sites.is_empty() {
        ctx.note("No flat, dry ground was big enough for a settlement");
        return vec![Step::Plan {
            stage: 2,
            f: plan_start,
        }];
    }
    let mut out = vec![];
    let art = p.art.id();
    for (i, s) in ctx.sites.iter().enumerate() {
        let what = match (s.kind, s.seat) {
            (Kind::City, _) => "the capital city",
            (Kind::Town, true) => "the realm's seat",
            (Kind::Town, false) => "the town",
            (Kind::Village, _) => "the village",
        };
        // (no palisades: MMO towns lie open in the land)
        let args = json!({ "kind": s.kind.id(), "position": xz(s.pos), "style": art, "walls": false, "seed": ctx.rng.u32(..), "name": s.name });
        out.push(call_then(
            2,
            "objects_place_town",
            format!("Built {what} of {}", s.name),
            args,
            move |_, ctx, v| {
                if let Some(c) = v
                    .get("centre")
                    .and_then(|c| serde_json::from_value::<[f32; 2]>(c.clone()).ok())
                {
                    ctx.sites[i].pos = Vec2::from_array(c);
                }
                ctx.sites[i].placed = true;
                let what = match ctx.sites[i].kind {
                    Kind::City => "capital",
                    Kind::Town => "towns",
                    Kind::Village => "villages",
                };
                ctx.count(what);
                let place = (
                    ctx.sites[i].name.clone(),
                    ctx.sites[i].kind.id(),
                    ctx.sites[i].pos,
                );
                ctx.places.push(place);
                vec![]
            },
        ));
    }
    out.push(Step::Plan {
        stage: 2,
        f: plan_roads,
    });
    if p.farms {
        out.push(call_then(
            2,
            "fields_auto",
            "Farm fields round the villages",
            json!({ "seed": p.seed % 100_000 }),
            |_, ctx, v| {
                if let Some(n) = v.get("fields").and_then(|f| f.as_array()).map(|a| a.len()) {
                    ctx.made.insert("fields", n);
                }
                vec![]
            },
        ));
    }
    out.push(Step::Plan {
        stage: 2,
        f: plan_start,
    });
    out
}

/// Roads join the settlements (the shortest network: a spanning tree), each between the
/// settlements' edges; never over open water (islands keep to themselves).
fn plan_roads(world: &mut World, ctx: &mut Ctx) -> Vec<Step> {
    if !ctx.plan.roads {
        return vec![];
    }
    let placed: Vec<usize> = (0..ctx.sites.len())
        .filter(|&i| ctx.sites[i].placed)
        .collect();
    let t = world.resource::<Terrain>();
    // the longest stretch of sea on the straight way between two points
    let sea_run = |a: Vec2, b: Vec2| -> f32 {
        let n = (a.distance(b) / 20.0).ceil().max(1.0) as usize;
        let (mut run, mut worst) = (0.0f32, 0.0f32);
        for k in 0..=n {
            let q = a.lerp(b, k as f32 / n as f32);
            run = if h(t, q) < ctx.sea { run + 20.0 } else { 0.0 };
            worst = worst.max(run);
        }
        worst
    };
    let mut pairs: Vec<(f32, usize, usize)> = vec![];
    for (x, &a) in placed.iter().enumerate() {
        for &b in &placed[x + 1..] {
            if sea_run(ctx.sites[a].pos, ctx.sites[b].pos) <= 60.0 {
                pairs.push((ctx.sites[a].pos.distance(ctx.sites[b].pos), a, b));
            }
        }
    }
    pairs.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut group: Vec<usize> = (0..ctx.sites.len()).collect();
    fn root(g: &mut [usize], mut i: usize) -> usize {
        while g[i] != i {
            g[i] = g[g[i]];
            i = g[i];
        }
        i
    }
    let mut out = vec![];
    for (_, a, b) in pairs {
        let (ra, rb) = (root(&mut group, a), root(&mut group, b));
        if ra == rb {
            continue;
        }
        group[ra] = rb;
        let (sa, sb) = (&ctx.sites[a], &ctx.sites[b]);
        let d = (sb.pos - sa.pos).normalize_or_zero();
        let from = sa.pos + d * sa.kind.r() * 0.85;
        let to = sb.pos - d * sb.kind.r() * 0.85;
        let kind = if sa.kind == Kind::City || sb.kind == Kind::City {
            "gravel"
        } else {
            "dirt_road"
        };
        let name = format!("{}–{} road", sa.name, sb.name);
        out.push(call_then(
            2,
            "roads_route",
            format!("The road from {} to {}", sa.name, sb.name),
            json!({ "from": xz(from), "to": xz(to), "kind": kind, "name": name }),
            |_, ctx, _| {
                ctx.count("roads");
                vec![]
            },
        ));
    }
    out
}

/// Players start just outside their starting village, facing it.
fn plan_start(world: &mut World, ctx: &mut Ctx) -> Vec<Step> {
    let realms = ctx.plan.realms.max(1) as usize;
    ctx.starters = (0..realms)
        .filter_map(|r| {
            let of = |k: Kind| {
                (0..ctx.sites.len()).find(|&i| {
                    ctx.sites[i].placed && ctx.sites[i].realm == r && ctx.sites[i].kind == k
                })
            };
            of(Kind::Village).or(of(Kind::Town)).or(of(Kind::City))
        })
        .collect();
    if ctx.starters.is_empty()
        && let Some(i) = (0..ctx.sites.len()).find(|&i| ctx.sites[i].placed)
    {
        ctx.starters.push(i);
    }
    let t = world.resource::<Terrain>();
    let spot = match ctx.starters.first() {
        Some(&i) => {
            let s = &ctx.sites[i];
            let toward = ctx
                .sites
                .iter()
                .find(|o| o.seat && o.placed && o.pos.distance(s.pos) > 1.0)
                .map(|o| o.pos)
                .unwrap_or(ctx.centroid);
            let base = (toward - s.pos).normalize_or(Vec2::X);
            (0..16).find_map(|k| {
                let turn = ((k + 1) / 2) as f32 * if k % 2 == 0 { 1.0 } else { -1.0 } * TAU / 16.0;
                let d = Vec2::from_angle(turn).rotate(base);
                let q = s.pos + d * (s.kind.r() + 18.0);
                (h(t, q) > ctx.sea + 1.0 && up(t, q) > 0.9 && ctx.river_dist(q) > 25.0)
                    .then_some((q, dir_bearing(s.pos - q)))
            })
        }
        None => None,
    };
    // no settlement: flat land near the middle
    let tries: Vec<Vec2> = (0..400)
        .map(|_| Vec2::new(ctx.rng.f32() - 0.5, ctx.rng.f32() - 0.5))
        .collect();
    let spot = spot.or_else(|| {
        tries
            .iter()
            .map(|d| ctx.centroid + *d * ctx.half)
            .find(|q| inside(ctx, *q, 50.0) && h(t, *q) > ctx.sea + 2.0 && up(t, *q) > 0.95)
            .map(|q| (q, 0.0))
    });
    let Some((q, yaw)) = spot else { return vec![] };
    ctx.start = Some((q, yaw));
    let by = ctx
        .starters
        .first()
        .map(|&i| format!(" by {}", ctx.sites[i].name))
        .unwrap_or_default();
    vec![call(
        2,
        "gameplay_set_player_start",
        format!("Players start{by}"),
        json!({ "position": xz(q), "yaw_deg": yaw.round() }),
    )]
}

// ------------------------------------------------------------------ regions, zones, levels

fn plan_regions(world: &mut World, ctx: &mut Ctx) -> Vec<Step> {
    let p = ctx.plan.clone();
    let realms = p.realms.max(1) as usize;
    let t = world.resource::<Terrain>();
    let n = 128usize;
    let g = ctx.half * 2.0 / n as f32;
    let mut r = Raster {
        n,
        g,
        half: ctx.half,
        owner: vec![-1; n * n],
    };
    let land: Vec<usize> = (0..n * n)
        .filter(|&k| h(t, r.centre(k % n, k / n)) > ctx.sea - 4.0)
        .collect();
    if land.is_empty() {
        return vec![];
    }
    // seeds: the settlements, then wild land farthest from them (the frontier first, between realms)
    let mut seeds: Vec<(Vec2, Option<usize>)> = (0..ctx.sites.len())
        .filter(|&i| ctx.sites[i].placed)
        .map(|i| (ctx.sites[i].pos, Some(i)))
        .collect();
    let cap = match p.size {
        WorldSize::Small => 10,
        WorldSize::Medium => 18,
        WorldSize::Large => 32,
    };
    let mut frontier = None;
    if realms > 1 {
        let k = *land
            .iter()
            .min_by(|a, b| {
                r.centre(**a % n, **a / n)
                    .distance(ctx.centroid)
                    .total_cmp(&r.centre(**b % n, **b / n).distance(ctx.centroid))
            })
            .unwrap();
        frontier = Some(seeds.len());
        seeds.push((r.centre(k % n, k / n), None));
    }
    // wild regions besides: enough that zones climb ~8 levels at a time
    let by_levels = ((p.levels[1] - p.levels[0]) as f32 / 8.0).ceil() as usize;
    let extra = ((seeds.len() as f32 * 0.6).ceil() as usize)
        .max(if seeds.is_empty() { 4 } else { 2 })
        .max(by_levels.saturating_sub(seeds.len()));
    let pts: Vec<Vec2> = seeds.iter().map(|s| s.0).collect();
    let mut dist: Vec<f32> = land
        .iter()
        .map(|&k| Ctx::near(&pts, r.centre(k % n, k / n)))
        .collect();
    for _ in 0..extra {
        if seeds.len() >= cap.max(ctx.sites.len() + 1) {
            break;
        }
        let Some((best, _)) = dist.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) else {
            break;
        };
        let c = r.centre(land[best] % n, land[best] / n);
        seeds.push((c, None));
        for (d, &k) in dist.iter_mut().zip(&land) {
            *d = d.min(r.centre(k % n, k / n).distance(c));
        }
    }
    // each land cell goes to the nearest seed (by a warped distance that also counts the climb,
    // so borders wander and tend to follow ridges)
    let spacing = ctx.half * 2.0 / (seeds.len() as f32).sqrt();
    let heights: Vec<f32> = seeds.iter().map(|s| h(t, s.0)).collect();
    let assign = |seeds: &[(Vec2, Option<usize>)], heights: &[f32], owner: &mut Vec<i32>| {
        for &k in &land {
            let c = r.centre(k % n, k / n);
            let warp = Vec2::new(
                crate::noise::perlin2(c.x / 700.0, c.y / 700.0, p.seed),
                crate::noise::perlin2(c.x / 700.0 + 17.3, c.y / 700.0 - 5.1, p.seed),
            ) * spacing
                * 0.25;
            let hc = h(t, c);
            let best = seeds
                .iter()
                .enumerate()
                .map(|(i, s)| (i, (c + warp).distance(s.0) + 1.2 * (hc - heights[i]).abs()))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|x| x.0 as i32)
                .unwrap_or(-1);
            owner[k] = best;
        }
    };
    let mut owner = vec![-1; n * n];
    assign(&seeds, &heights, &mut owner);
    // wild regions of only a few cells are folded into their neighbours
    let small: Vec<usize> = (0..seeds.len())
        .filter(|&i| {
            seeds[i].1.is_none()
                && Some(i) != frontier
                && owner.iter().filter(|o| **o == i as i32).count() < 12
        })
        .collect();
    if !small.is_empty() {
        let keep: Vec<usize> = (0..seeds.len()).filter(|i| !small.contains(i)).collect();
        if frontier.is_some() {
            frontier = keep.iter().position(|&i| Some(i) == frontier);
        }
        seeds = keep.iter().map(|&i| seeds[i]).collect();
        let heights: Vec<f32> = keep.iter().map(|&i| heights[i]).collect();
        assign(&seeds, &heights, &mut owner);
    }
    r.owner = owner;
    // neighbours
    let m = seeds.len();
    let mut adj = vec![HashSet::new(); m];
    for j in 0..n {
        for i in 0..n {
            let a = r.owner[j * n + i];
            for (di, dj) in [(1, 0), (0, 1)] {
                if i + di >= n || j + dj >= n {
                    continue;
                }
                let b = r.owner[(j + dj) * n + i + di];
                if a >= 0 && b >= 0 && a != b {
                    adj[a as usize].insert(b as usize);
                    adj[b as usize].insert(a as usize);
                }
            }
        }
    }
    let mut counts = vec![0usize; m];
    let mut biomes: Vec<BTreeMap<u8, usize>> = vec![BTreeMap::new(); m];
    for &k in &land {
        let o = r.owner[k];
        if o >= 0 {
            counts[o as usize] += 1;
            let c = r.centre(k % n, k / n);
            *biomes[o as usize].entry(t.biome_at(c.x, c.y)).or_default() += 1;
        }
    }
    let _ = t;
    ctx.regions = (0..m)
        .map(|i| {
            let site = seeds[i].1;
            let realm = if Some(i) == frontier {
                None
            } else {
                Some(
                    site.map(|s| ctx.sites[s].realm)
                        .unwrap_or_else(|| ctx.sector(seeds[i].0)),
                )
            };
            let biome = biomes[i]
                .iter()
                .filter(|(b, _)| **b != 0)
                .max_by_key(|(_, c)| **c)
                .and_then(|(b, _)| BiomePreset::from_id(*b));
            let zone_type = if Some(i) == frontier {
                "contested"
            } else if site.is_some_and(|s| ctx.sites[s].kind == Kind::City) {
                "city"
            } else {
                "open_world"
            };
            Region {
                seed: seeds[i].0,
                site,
                realm,
                cells: counts[i],
                biome,
                levels: p.levels,
                zone_type,
                name: String::new(),
                zone: None,
            }
        })
        .collect();
    ctx.raster = Some(r);
    // levels rise with the steps from the start region (per realm; the frontier is endgame)
    let [lo, hi] = p.levels;
    let homeland_top = if realms > 1 {
        lo + (hi - lo) * 4 / 5
    } else {
        hi
    };
    for realm in 0..realms {
        let members: Vec<usize> = (0..m)
            .filter(|&i| ctx.regions[i].realm == Some(realm))
            .collect();
        if members.is_empty() {
            continue;
        }
        let start = ctx
            .starters
            .get(realm)
            .and_then(|&s| ctx.region_of(ctx.sites[s].pos))
            .filter(|s| members.contains(s))
            .unwrap_or(members[0]);
        let order = level_order(&ctx.regions, &adj, &members, start);
        let k = order.len() as u32;
        for (rank, &i) in order.iter().enumerate() {
            let a = lo + (homeland_top - lo) * rank as u32 / k;
            let b = (lo + (homeland_top - lo) * (rank as u32 + 1) / k)
                .max(a + 2)
                .min(hi.max(a + 2));
            ctx.regions[i].levels = [a, b];
        }
    }
    if let Some(f) = frontier {
        ctx.regions[f].levels = [lo + (hi - lo) * 3 / 4, hi];
    }
    // names
    if realms > 1 {
        for r in 0..realms {
            let name = ctx.realm_name(r);
            ctx.note(format!("Realm {}: {name}", r + 1));
        }
    }
    for i in 0..m {
        let site = ctx.regions[i].site.map(|s| ctx.sites[s].name.clone());
        let biome = ctx.regions[i].biome;
        ctx.regions[i].name = match (&site, ctx.regions[i].zone_type) {
            (Some(s), "city") => s.clone(),
            _ => ctx.zone_name(site.as_deref(), biome),
        };
    }
    let mut out = vec![];
    if p.zones {
        let r = ctx.raster.as_ref().unwrap();
        let mut by_size: Vec<usize> = (0..m).collect();
        // bigger first: a region enclosed by another is painted over it afterwards
        by_size.sort_by_key(|&i| std::cmp::Reverse(ctx.regions[i].cells));
        for i in by_size {
            let mut loops = region_loops(r, i as i32);
            if loops.is_empty() {
                continue;
            }
            loops.sort_by(|a, b| geom::signed_area(b).total_cmp(&geom::signed_area(a)));
            let main = loops.remove(0);
            let reg = &ctx.regions[i];
            let poly = |l: &[Vec2]| {
                l.iter()
                    .map(|q| {
                        [
                            q.x.clamp(-ctx.half, ctx.half).round(),
                            q.y.clamp(-ctx.half, ctx.half).round(),
                        ]
                    })
                    .collect::<Vec<_>>()
            };
            let biome = reg
                .biome
                .map(|b| b.def().name.to_lowercase())
                .unwrap_or_else(|| "wild land".into());
            let realm = match reg.realm {
                Some(r) if realms > 1 => format!(
                    ", in the lands of {}",
                    ctx.realm_names.get(r).cloned().unwrap_or_default()
                ),
                None => ", the contested frontier between the realms".into(),
                _ => String::new(),
            };
            let desc = format!(
                "{}{realm}. Levels {}–{}.",
                capitalise(&biome),
                reg.levels[0],
                reg.levels[1]
            );
            let args = json!({ "name": reg.name, "level_min": reg.levels[0], "level_max": reg.levels[1].min(200), "zone_type": reg.zone_type, "description": desc, "area": { "polygon": poly(&main) } });
            let extra: Vec<Vec<[f32; 2]>> = loops.iter().map(|l| poly(l)).collect();
            out.push(call_then(
                3,
                "zones_create",
                format!(
                    "Zone {}: levels {}–{}",
                    reg.name, reg.levels[0], reg.levels[1]
                ),
                args,
                move |_, ctx, v| {
                    let id = v
                        .as_str()
                        .and_then(|s| s.strip_prefix("Zone "))
                        .and_then(|s| s.split_whitespace().next())
                        .and_then(|s| s.parse::<u32>().ok());
                    ctx.regions[i].zone = id;
                    ctx.count("zones");
                    let Some(id) = id else { return vec![] };
                    extra
                        .into_iter()
                        .map(|l| {
                            call(
                                3,
                                "zones_update",
                                "…and its islands",
                                json!({ "id": id, "paint": { "polygon": l } }),
                            )
                        })
                        .collect()
                },
            ));
        }
    } else {
        ctx.note("No zones (not asked for): levels still rise away from the start");
    }
    // the settlements on the map
    for s in ctx.sites.iter().filter(|s| s.placed) {
        let levels = ctx.levels_at(s.pos);
        out.push(call(3, "gameplay_add_poi", format!("{} on the map", s.name), json!({ "kind": "Town", "name": s.name, "position": xz(s.pos), "radius": s.kind.r().round(), "level": levels })));
    }
    out
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Regions of `members` from `start` outwards, by the way across them (seed to seed over the
/// borders; a region over the water from the nearest one reached, the crossing counted 1.5×), so
/// levels rise as players travel and neighbours end up close in level.
fn level_order(
    regions: &[Region],
    adj: &[HashSet<usize>],
    members: &[usize],
    start: usize,
) -> Vec<usize> {
    let gap = |a: usize, b: usize| regions[a].seed.distance(regions[b].seed);
    let mut dist: BTreeMap<usize, f32> = BTreeMap::from([(start, 0.0)]);
    let mut done: HashSet<usize> = HashSet::new();
    loop {
        let next = dist
            .iter()
            .filter(|(i, _)| !done.contains(*i))
            .min_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, d)| (*i, *d));
        let Some((a, d)) = next else {
            let settled = &done;
            let far = members
                .iter()
                .filter(|i| !dist.contains_key(*i))
                .flat_map(|&i| settled.iter().map(move |&j| (i, j)))
                .min_by(|x, y| gap(x.0, x.1).total_cmp(&gap(y.0, y.1)));
            let Some((i, j)) = far else { break };
            let d = dist[&j] + 1.5 * gap(i, j);
            dist.insert(i, d);
            continue;
        };
        done.insert(a);
        for &b in &adj[a] {
            if members.contains(&b) && !done.contains(&b) {
                let nd = d + gap(a, b);
                if dist.get(&b).is_none_or(|x| nd < *x) {
                    dist.insert(b, nd);
                }
            }
        }
    }
    let mut v: Vec<usize> = members.to_vec();
    v.sort_by(|a, b| {
        dist.get(a)
            .unwrap_or(&f32::MAX)
            .total_cmp(dist.get(b).unwrap_or(&f32::MAX))
    });
    v
}

/// A region's outlines (outer loops only, simplified, largest first).
fn region_loops(r: &Raster, idx: i32) -> Vec<Vec<Vec2>> {
    let (mut i0, mut j0, mut i1, mut j1) = (r.n, r.n, 0, 0);
    for j in 0..r.n {
        for i in 0..r.n {
            if r.owner[j * r.n + i] == idx {
                (i0, j0, i1, j1) = (i0.min(i), j0.min(j), i1.max(i), j1.max(j));
            }
        }
    }
    if i0 > i1 {
        return vec![];
    }
    let loops = geom::contours(r.centre(i0, j0), r.centre(i1, j1), r.g, |p| {
        if r.owner_at(p) == idx { 1.0 } else { -1.0 }
    });
    loops
        .into_iter()
        .map(|l| geom::simplify_loop(&l, r.g * 0.6))
        .filter(|l| l.len() >= 3 && geom::signed_area(l) > r.g * r.g * 3.0)
        .collect()
}

// ------------------------------------------------------------------ dungeons and caves

/// Dungeon layouts that suit a land.
fn layouts_for(b: Option<BiomePreset>, raid: bool) -> &'static [&'static str] {
    use BiomePreset::*;
    if raid {
        return match b {
            Some(Volcanic) => &["infernal", "great_caverns"],
            Some(Taiga | Tundra | Alpine) => &["frost_halls", "underground_city"],
            Some(HauntedWoods | Swamp) => &["cathedral", "catacombs"],
            _ => &["underground_city", "great_caverns", "cathedral"],
        };
    }
    match b {
        Some(Desert) => &["sun_temple", "ruined_fortress", "catacombs"],
        Some(Jungle | TropicalCoast) => &["spider_den", "sun_temple", "ruined_fortress"],
        Some(Taiga | Tundra) => &["frost_halls", "dwarven_halls", "mine"],
        Some(Alpine) => &["dwarven_halls", "frost_halls", "mine"],
        Some(Volcanic) => &["infernal", "dwarven_halls", "mine"],
        Some(Swamp) => &["sewers", "spider_den", "crypt"],
        Some(HauntedWoods) => &["crypt", "catacombs", "cathedral"],
        _ => &[
            "crypt",
            "goblin_cave",
            "ruined_fortress",
            "prison_keep",
            "catacombs",
            "mine",
        ],
    }
}

/// A portal frame and colour that suit a layout.
fn portal_for(layout: &str) -> (&'static str, &'static str) {
    match layout {
        "crypt" | "catacombs" => ("hp:portal_ruin", "void"),
        "dwarven_halls" | "mine" | "underground_city" => ("hp:portal_dwarven", "arcane"),
        "sun_temple" | "cathedral" => ("hp:portal_gate", "holy"),
        "frost_halls" => ("hp:portal_spires", "frost"),
        "infernal" => ("hp:portal_ring", "fire"),
        "spider_den" | "goblin_cave" => ("hp:portal_ruin", "fel"),
        "sewers" => ("hp:portal_arch", "tide"),
        "prison_keep" | "ruined_fortress" => ("hp:portal_arch", "blood"),
        _ => ("hp:portal_arch", "arcane"),
    }
}

/// A spot in region `region` (or anywhere when None) passing `ok`, the best by `score`, from
/// `tries` random samples.
fn find_spot(
    ctx: &mut Ctx,
    region: Option<usize>,
    tries: usize,
    ok: &dyn Fn(&Ctx, Vec2) -> Option<f32>,
) -> Option<Vec2> {
    let r = ctx.raster.as_ref()?;
    let cells: Vec<usize> = (0..r.n * r.n)
        .filter(|&k| match region {
            Some(g) => r.owner[k] == g as i32,
            None => r.owner[k] >= 0,
        })
        .collect();
    if cells.is_empty() {
        return None;
    }
    let g = r.g;
    let mut best: Option<(f32, Vec2)> = None;
    for _ in 0..tries {
        let k = cells[ctx.rng.usize(..cells.len())];
        let r = ctx.raster.as_ref()?;
        let c =
            r.centre(k % r.n, k / r.n) + Vec2::new(ctx.rng.f32() - 0.5, ctx.rng.f32() - 0.5) * g;
        if let Some(s) = ok(ctx, c)
            && best.is_none_or(|b| s > b.0)
        {
            best = Some((s, c));
        }
    }
    best.map(|b| b.1)
}

fn plan_dungeons(world: &mut World, ctx: &mut Ctx) -> Vec<Step> {
    let p = ctx.plan.clone();
    let total = (p.dungeons + p.raids) as usize;
    if total == 0 || ctx.regions.is_empty() {
        return vec![];
    }
    let order = ctx.wild_order();
    let gap = (ctx.half * 2.0 / (total as f32).sqrt() * 0.35).max(300.0);
    let [lo, hi] = p.levels;
    let rooms = if ["normal", "large", "extra_large"].contains(&p.dungeon_rooms.as_str()) {
        p.dungeon_rooms.clone()
    } else {
        "normal".into()
    };
    let mut out = vec![];
    let mut used: Vec<&'static str> = vec![];
    let mut fixed = p.dungeon_layouts.iter().cycle();
    // which are open-air, built from design sheets: the raids first, then every other party one
    let outdoor = outdoor_picks(
        p.dungeons as usize,
        p.raids as usize,
        p.outdoor_dungeons as usize,
    );
    let mut sheets_used: Vec<String> = vec![];
    for k in 0..total {
        let raid = k >= p.dungeons as usize;
        // party dungeons spread along the level climb; raids in the highest regions
        let region = if raid {
            order[order.len() - 1 - (k - p.dungeons as usize) % order.len()]
        } else {
            order[((k as f32 + 0.5) / p.dungeons.max(1) as f32 * order.len() as f32) as usize
                % order.len()]
        };
        let spot = {
            let t = world.resource::<Terrain>();
            let ok = |ctx: &Ctx, q: Vec2| -> Option<f32> {
                let y = h(t, q);
                let rel = (y - ctx.sea) / (ctx.peak - ctx.sea).max(1.0);
                if !inside(ctx, q, 80.0)
                    || !(0.02..0.7).contains(&rel)
                    || up(t, q) < 0.9
                    || ctx.site_dist(q) < 220.0
                    || ctx.river_dist(q) < 40.0
                    || Ctx::near(&ctx.dungeons, q) < gap
                {
                    return None;
                }
                if (0..4).any(|a| up(t, q + Vec2::from_angle(a as f32 * TAU / 4.0) * 5.0) < 0.88) {
                    return None;
                }
                // by a hillside reads best
                let rise = (0..8)
                    .map(|a| h(t, q + Vec2::from_angle(a as f32 * TAU / 8.0) * 50.0) - y)
                    .fold(0.0, f32::max);
                Some(rise.min(25.0) + ctx.rng_free(q))
            };
            find_spot(ctx, Some(region), 160, &ok).or_else(|| find_spot(ctx, None, 300, &ok))
        };
        let Some(q) = spot else {
            ctx.note("No good spot was left for another dungeon");
            break;
        };
        ctx.dungeons.push(q);
        let reg = ctx.region_of(q).unwrap_or(region);
        if outdoor.contains(&k) {
            if let Some(step) = sheet_dungeon(ctx, &p, q, reg, raid, k, &mut sheets_used) {
                out.push(step);
                continue;
            }
        }
        let layout: String = match fixed.next() {
            Some(l) if crate::dungeons::presets::get(l).is_some() => l.clone(),
            _ => {
                let opts = layouts_for(ctx.regions[reg].biome, raid);
                let l = opts
                    .iter()
                    .find(|l| !used.contains(*l))
                    .copied()
                    .unwrap_or(opts[k % opts.len()]);
                used.push(l);
                l.to_string()
            }
        };
        let name = ctx.dungeon_name(&layout);
        let zl = ctx.regions[reg].levels;
        let levels = if raid {
            [hi.saturating_sub(2).max(lo), hi]
        } else {
            [zl[1].saturating_sub(2).max(zl[0]), (zl[1] + 3).min(hi)]
        };
        let levels = [levels[0].min(255), levels[1].min(255)];
        let (frame, color) = portal_for(&layout);
        let toward = ctx
            .sites
            .iter()
            .filter(|s| s.placed)
            .min_by(|a, b| a.pos.distance(q).total_cmp(&b.pos.distance(q)))
            .map(|s| s.pos)
            .or(ctx.start.map(|s| s.0))
            .unwrap_or(ctx.centroid);
        let facing = dir_bearing(toward - q).round();
        let size = if raid {
            "large"
        } else if p.size == WorldSize::Small {
            "small"
        } else {
            "medium"
        };
        let room_size = if raid && rooms == "normal" {
            "large".to_string()
        } else {
            rooms.clone()
        };
        let args = json!({
            "name": name, "kind": "interior", "layout": layout, "size": size, "room_size": room_size,
            "irregularity": p.dungeon_irregularity, "art": "handpainted", "instancing": if raid { "raid" } else { "party" },
            "way_in": { "kind": "portal", "position": xz(q), "facing_deg": facing, "frame": frame, "color": color, "label": name }
        });
        let what = if raid { "Raid" } else { "Dungeon" };
        let label = format!(
            "{what}: {name} ({}, levels {}–{})",
            layout.replace('_', " "),
            levels[0],
            levels[1]
        );
        out.push(call_then(4, "dungeons_create", label, args, move |_, ctx, v| {
            ctx.count(if raid { "raids" } else { "dungeons" });
            ctx.places.push((name.clone(), if raid { "raid" } else { "dungeon" }, q));
            let Some(id) = v.pointer("/dungeon/id").and_then(|i| i.as_u64()) else { return vec![] };
            vec![
                call(4, "dungeons_set", format!("{name}: levels {}–{}", levels[0], levels[1]), json!({ "id": id, "levels": levels })),
                call(4, "gameplay_add_poi", format!("{name} on the map"), json!({ "kind": "Dungeon", "name": name, "position": xz(q), "level": levels })),
            ]
        }));
    }
    if p.art == super::plan::Art::Photoreal && !out.is_empty() {
        ctx.note("Dungeons are hand-painted for now (realistic dungeons are coming)");
    }
    out
}

/// Which of `dungeons` party dungeons and `raids` raids (indices: party first) are open-air: the
/// raids first, then party dungeons spread along the climb.
fn outdoor_picks(dungeons: usize, raids: usize, want: usize) -> Vec<usize> {
    let mut v: Vec<usize> = (dungeons..dungeons + raids).take(want).collect();
    let left = want.saturating_sub(v.len()).min(dungeons);
    for i in 0..left {
        // (every other one, from the second: 1, 3, 0, 2…)
        let k = (1 + 2 * i) % dungeons.max(1);
        let k = if v.contains(&k) {
            (0..dungeons).find(|x| !v.contains(x)).unwrap_or(k)
        } else {
            k
        };
        v.push(k);
    }
    v
}

/// An open-air dungeon at `q`, built from the library sheet that suits its land (None: no
/// library, so it is made underground as before).
fn sheet_dungeon(
    ctx: &mut Ctx,
    p: &super::plan::WorldPlan,
    q: Vec2,
    reg: usize,
    raid: bool,
    k: usize,
    used: &mut Vec<String>,
) -> Option<Step> {
    use crate::dungeons::sheets::{blueprint::Kind, library};
    let climate = format!("{:?}", p.climate);
    let biome = ctx.regions[reg]
        .biome
        .map(|b| format!("{b:?}"))
        .unwrap_or_default();
    let family = library::family_for(&climate, &biome).to_string();
    let kind = if raid { Kind::Raid } else { Kind::Party };
    let e = library::pick(&family, kind, used, p.seed.wrapping_add(k as u32 * 7919))?;
    used.push(e.id.clone());
    let name = e.name.clone();
    let [lo, hi] = p.levels;
    let zl = ctx.regions[reg].levels;
    let levels = if raid {
        [hi.saturating_sub(2).max(lo), hi]
    } else {
        [zl[1].saturating_sub(2).max(zl[0]), (zl[1] + 3).min(hi)]
    };
    let levels = [levels[0].min(255), levels[1].min(255)];
    let (frame, color) = match family.as_str() {
        "arid" => ("hp:portal_gate", "holy"),
        "cold" => ("hp:portal_spires", "frost"),
        "volcanic" => ("hp:portal_ring", "fire"),
        "tropical" => ("hp:portal_arch", "tide"),
        "twilight" => ("hp:portal_ruin", "void"),
        _ => ("hp:portal_arch", "arcane"),
    };
    let toward = ctx
        .sites
        .iter()
        .filter(|s| s.placed)
        .min_by(|a, b| a.pos.distance(q).total_cmp(&b.pos.distance(q)))
        .map(|s| s.pos)
        .or(ctx.start.map(|s| s.0))
        .unwrap_or(ctx.centroid);
    let facing = dir_bearing(toward - q).round();
    let args = json!({
        "name": name, "sheet": e.id, "art": "handpainted", "instancing": if raid { "raid" } else { "party" },
        "way_in": { "kind": "portal", "position": xz(q), "facing_deg": facing, "frame": frame, "color": color, "label": name }
    });
    let what = if raid { "Raid" } else { "Dungeon" };
    let label = format!(
        "{what}: {name} (open-air, {}, levels {}–{})",
        e.layout_type.replace('_', " "),
        levels[0],
        levels[1]
    );
    let unique = p.unique_dungeons;
    let kind_id = if raid { "raid" } else { "party" };
    Some(call_then(
        4,
        "dungeons_create",
        label,
        args,
        move |_, ctx, v| {
            ctx.count(if raid { "raids" } else { "dungeons" });
            ctx.places
                .push((name.clone(), if raid { "raid" } else { "dungeon" }, q));
            let Some(id) = v.pointer("/dungeon/id").and_then(|i| i.as_u64()) else {
                return vec![];
            };
            let mut then = vec![
                call(
                    4,
                    "dungeons_set",
                    format!("{name}: levels {}–{}", levels[0], levels[1]),
                    json!({ "id": id, "levels": levels }),
                ),
                call(
                    4,
                    "gameplay_add_poi",
                    format!("{name} on the map"),
                    json!({ "kind": "Dungeon", "name": name, "position": xz(q), "level": levels }),
                ),
            ];
            // its own unique sheet, painted in the background by the user's AI
            if unique {
                then.push(call(
                    4,
                    "dungeons_sheet_generate",
                    format!("{name}: a unique sheet of its own"),
                    json!({ "family": family, "kind": kind_id, "dungeon": id }),
                ));
            }
            then
        },
    ))
}

impl Ctx {
    /// A little deterministic variety for spot scores.
    fn rng_free(&self, q: Vec2) -> f32 {
        crate::noise::value2(q.x * 0.013, q.y * 0.013, self.plan.seed) * 4.0
    }
}

fn plan_caves(world: &mut World, ctx: &mut Ctx) -> Vec<Step> {
    let want = ctx.plan.caves as usize;
    if want == 0 || ctx.regions.is_empty() {
        return vec![];
    }
    let order = ctx.wild_order();
    let mut out = vec![];
    for k in 0..want * 2 {
        if ctx.caves.len() >= want {
            break;
        }
        let region = order[(k * 7 + 3) % order.len()];
        let spot = {
            let t = world.resource::<Terrain>();
            let ok = |ctx: &Ctx, q: Vec2| -> Option<f32> {
                let rel = (h(t, q) - ctx.sea) / (ctx.peak - ctx.sea).max(1.0);
                let u = up(t, q);
                if !inside(ctx, q, 100.0)
                    || !(0.08..0.8).contains(&rel)
                    || !(0.55..0.85).contains(&u)
                    || ctx.site_dist(q) < 160.0
                    || ctx.river_dist(q) < 30.0
                    || Ctx::near(&ctx.dungeons, q) < 200.0
                    || Ctx::near(&ctx.caves, q) < 350.0
                {
                    return None;
                }
                Some(ctx.rng_free(q) + (0.85 - u) * 10.0)
            };
            find_spot(ctx, Some(region), 160, &ok)
        };
        let Some(q) = spot else { continue };
        ctx.caves.push(q);
        let name = format!("{} Cave", ctx.place_name());
        let label = format!("Cave: {name}");
        let args = json!({ "position": xz(q), "seed": ctx.rng.u32(..100_000), "name": name });
        out.push(call_then(
            4,
            "caves_create",
            label,
            args,
            move |_, ctx, _| {
                ctx.count("caves");
                ctx.places.push((name, "cave", q));
                vec![]
            },
        ));
    }
    if ctx.caves.len() < want {
        ctx.note(format!(
            "Room for {} of {want} caves (they need hillsides away from towns)",
            ctx.caves.len()
        ));
    }
    out
}

// ------------------------------------------------------------------ camps and creatures

fn plan_camps(world: &mut World, ctx: &mut Ctx) -> Vec<Step> {
    if !ctx.plan.encounters || ctx.regions.is_empty() {
        return vec![];
    }
    let art = ctx.plan.art.id();
    let lib = crate::mcp::call_tool(
        world,
        "objects_library",
        json!({ "kind": "scenes", "style": art }),
    )
    .map(|o| answer(&o))
    .unwrap_or(Value::Null);
    let mut scenes: Vec<(String, String, f32)> = lib
        .get("assets")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    Some((
                        e.get("id")?.as_str()?.to_string(),
                        e.get("name")?.as_str()?.to_string(),
                        e.get("radius").and_then(|r| r.as_f64()).unwrap_or(20.0) as f32,
                    ))
                })
                .filter(|s| s.2 <= 70.0)
                .collect()
        })
        .unwrap_or_default();
    if scenes.is_empty() {
        ctx.note(format!(
            "No ready-made encounter sites in the {} style yet",
            ctx.plan.art.name().to_lowercase()
        ));
        return vec![];
    }
    ctx.rng.shuffle(&mut scenes);
    let order = ctx.wild_order();
    let want = match ctx.plan.size {
        WorldSize::Small => 2,
        WorldSize::Medium => 4,
        WorldSize::Large => 8,
    }
    .min(order.len().max(1) * 2);
    let mut out = vec![];
    for k in 0..want {
        let (id, name, rad) = scenes[k % scenes.len()].clone();
        let region =
            order[((k as f32 + 0.5) / want as f32 * order.len() as f32) as usize % order.len()];
        let spot = {
            let t = world.resource::<Terrain>();
            let ok = |ctx: &Ctx, q: Vec2| -> Option<f32> {
                let y = h(t, q);
                if !inside(ctx, q, rad + 40.0) || y < ctx.sea + 1.5 || grade(t, q) > 0.25 {
                    return None;
                }
                let (mut lo, mut hi) = (y, y);
                for a in 0..8 {
                    let s = q + Vec2::from_angle(a as f32 * TAU / 8.0) * rad * 0.8;
                    let hs = h(t, s);
                    if hs < ctx.sea + 1.0 || grade(t, s) > 0.32 {
                        return None;
                    }
                    lo = lo.min(hs);
                    hi = hi.max(hs);
                }
                if hi - lo > rad * 0.15 + 1.5
                    || ctx.site_dist(q) < rad + 120.0
                    || ctx.river_dist(q) < rad + 25.0
                    || Ctx::near(&ctx.dungeons, q) < rad + 80.0
                    || Ctx::near(&ctx.caves, q) < rad + 80.0
                    || Ctx::near(&ctx.camps, q) < rad * 2.0 + 150.0
                {
                    return None;
                }
                Some(ctx.rng_free(q) - (hi - lo))
            };
            find_spot(ctx, Some(region), 200, &ok)
        };
        let Some(q) = spot else { continue };
        ctx.camps.push(q);
        let levels = ctx.levels_at(q);
        let place = ctx.regions[region].name.clone();
        let yaw = (ctx.rng.f32() * 360.0).round();
        out.push(call_then(
            5,
            "objects_place",
            format!("{name} in {place}"),
            json!({ "items": [{ "asset": id, "position": xz(q), "yaw_deg": yaw }] }),
            move |_, ctx, _| {
                ctx.count("camps");
                ctx.places.push((name.clone(), "camp", q));
                vec![call(
                    5,
                    "gameplay_add_poi",
                    format!("{name} on the map"),
                    json!({ "kind": "Camp", "name": name, "position": xz(q), "level": levels }),
                )]
            },
        ));
    }
    out
}

/// Higher creatures than the starter bestiary's (levels 31 up).
const HIGH_CREATURES: [(&str, &str, bool); 10] = [
    ("Shadow Stalker", "cat", false),
    ("Grave Wight", "undead", false),
    ("Basilisk", "basilisk", false),
    ("Stone Giant", "giant", true),
    ("Wyvern", "drake", false),
    ("Frost Giant", "giant", true),
    ("Fire Drake", "drake", true),
    ("Dread Knight", "undead", true),
    ("Void Horror", "aberration", true),
    ("Elder Wyrm", "dragon", true),
];

/// Whether a creature belongs in a land.
fn fits_land(b: Option<BiomePreset>, family: &str, name: &str) -> bool {
    use BiomePreset::*;
    let f = family.to_lowercase();
    let n = name.to_lowercase();
    match b {
        Some(Taiga | Tundra | Alpine) => {
            n.contains("frost") || ["troll", "harpy", "giant", "bear"].contains(&f.as_str())
        }
        Some(Swamp) => ["swamp beast", "spider", "crab"].contains(&f.as_str()),
        Some(Volcanic) => {
            ["elemental", "drake", "dragon"].contains(&f.as_str())
                || n.contains("fire")
                || n.contains("ash")
        }
        Some(HauntedWoods) => ["spider", "undead", "aberration"].contains(&f.as_str()),
        Some(Desert) => ["basilisk", "bandit", "harpy", "drake"].contains(&f.as_str()),
        Some(Jungle | TropicalCoast) => ["spider", "crab", "cat"].contains(&f.as_str()),
        _ => {
            [
                "critter", "boar", "wolf", "bear", "bandit", "goblin", "treant", "spider",
            ]
            .contains(&f.as_str())
                && !n.contains("frost")
        }
    }
}

fn plan_creatures(world: &mut World, ctx: &mut Ctx) -> Vec<Step> {
    if !ctx.plan.creatures || ctx.regions.is_empty() {
        return vec![];
    }
    let [lo, hi] = ctx.plan.levels;
    if let Err(e) =
        crate::mcp::call_tool(world, "gameplay_add_creature", json!({ "starter": true }))
    {
        ctx.note(format!("Skipped — the starter bestiary: {e}"));
        return vec![];
    }
    if hi > 30 {
        let k = (((hi - 30) / 7) as usize).clamp(2, HIGH_CREATURES.len());
        let w = ((hi - 30) / k as u32).max(1);
        for (i, (name, family, elite)) in HIGH_CREATURES.iter().take(k).enumerate() {
            let a = 31 + w * i as u32;
            let level = [a.min(255), (a + w + 1).min(hi).max(a).min(255)];
            let _ = crate::mcp::call_tool(
                world,
                "gameplay_add_creature",
                json!({ "name": name, "family": family, "level": level, "aggression": "Hostile", "elite": elite }),
            );
        }
    }
    let creatures: Vec<(String, String, [u32; 2])> = world
        .resource::<crate::gameplay::Gameplay>()
        .creatures
        .iter()
        .map(|c| {
            (
                c.name.clone(),
                c.family.clone(),
                [c.level[0] as u32, c.level[1] as u32],
            )
        })
        .collect();
    ctx.note(format!(
        "A bestiary of {} creatures (levels {lo}–{hi})",
        creatures.len()
    ));
    let radius = (ctx.half * 2.0 / 40.0).clamp(60.0, 160.0);
    let mut out = vec![];
    for i in 0..ctx.regions.len() {
        if ctx.regions[i].zone_type == "city" {
            continue;
        }
        let km2 =
            ctx.regions[i].cells as f32 * ctx.raster.as_ref().map_or(0.0, |r| r.g * r.g) / 1.0e6;
        let want = ((km2 / 0.5).round() as usize).clamp(1, 4);
        let [a, b] = ctx.regions[i].levels;
        let biome = ctx.regions[i].biome;
        // the creatures whose levels meet the region's, those of its land first
        let mut table: Vec<(f32, &str)> = creatures
            .iter()
            .map(|(n, f, l)| {
                let overlap = (l[1].min(b) as f32 - l[0].max(a) as f32).max(-1.0);
                let gap = if l[1] < a {
                    (a - l[1]) as f32
                } else if l[0] > b {
                    (l[0] - b) as f32
                } else {
                    0.0
                };
                let s = if overlap >= 0.0 { 10.0 + overlap } else { -gap }
                    + if fits_land(biome, f, n) { 4.0 } else { 0.0 };
                (s, n.as_str())
            })
            .collect();
        table.sort_by(|x, y| y.0.total_cmp(&x.0));
        let names: Vec<String> = table.iter().take(3).map(|x| x.1.to_string()).collect();
        if names.is_empty() {
            continue;
        }
        for _ in 0..want {
            let spot = {
                let t = world.resource::<Terrain>();
                let ok = |ctx: &Ctx, q: Vec2| -> Option<f32> {
                    if !inside(ctx, q, radius * 0.5) || h(t, q) < ctx.sea + 1.0 || up(t, q) < 0.82 {
                        return None;
                    }
                    if ctx.site_dist(q) < radius * 0.6 + 40.0
                        || Ctx::near(&ctx.dungeons, q) < radius * 0.5 + 50.0
                        || Ctx::near(&ctx.caves, q) < radius * 0.5 + 50.0
                        || Ctx::near(&ctx.camps, q) < radius * 0.5 + 60.0
                        || Ctx::near(&ctx.spawns, q) < radius * 2.0
                        || ctx.river_dist(q) < 20.0
                    {
                        return None;
                    }
                    Some(ctx.rng_free(q))
                };
                find_spot(ctx, Some(i), 120, &ok)
            };
            let Some(q) = spot else { break };
            ctx.spawns.push(q);
            let label = format!("Creatures in {}: {}", ctx.regions[i].name, names.join(", "));
            out.push(call_then(5, "gameplay_add_spawn_region", label, json!({ "center": xz(q), "radius": radius.round(), "creatures": names, "level": [a.min(255), b.min(255)] }), |_, ctx, _| {
                ctx.count("spawn regions");
                vec![]
            }));
        }
    }
    out
}

// ------------------------------------------------------------------ gathering

fn plan_gathering(world: &mut World, ctx: &mut Ctx) -> Vec<Step> {
    let g = ctx.plan.gathering.clone();
    let mut kinds = vec![];
    if g.ore {
        kinds.extend(["Ore", "Crystal"]);
    }
    if g.herbs {
        kinds.push("Herb");
    }
    if g.wood {
        kinds.push("Wood");
    }
    if g.stone {
        kinds.push("Stone");
    }
    let mut out = vec![];
    if !kinds.is_empty() {
        let label = format!(
            "Gathering: {}",
            kinds
                .iter()
                .map(|k| k.to_lowercase())
                .collect::<Vec<_>>()
                .join(", ")
        );
        out.push(call_then(6, "gameplay_scatter_resources", label, json!({ "kinds": kinds, "density": g.density.clamp(0.1, 3.0), "seed": ctx.plan.seed % 100_000 }), |_, ctx, v| {
            if let Some(n) = v.get("added").and_then(|n| n.as_u64()) {
                ctx.made.insert("gathering nodes", n as usize);
            }
            vec![]
        }));
    }
    if g.fish {
        let want = match ctx.plan.size {
            WorldSize::Small => 3,
            WorldSize::Medium => 6,
            WorldSize::Large => 10,
        };
        let t = world.resource::<Terrain>();
        let mut spots: Vec<Vec2> = vec![];
        for s in ctx.sites.iter().filter(|s| s.placed) {
            if spots.len() >= want {
                break;
            }
            // the nearest river bend, else the nearest sea a boat's throw out
            // (the lower reaches: the head of a river is too shallow for fish)
            let river = ctx
                .rivers
                .iter()
                .flat_map(|r| r.iter().skip(r.len() / 3).copied())
                .filter(|q| q.distance(s.pos) < 700.0 && h(t, *q) > ctx.sea)
                .min_by(|a, b| a.distance(s.pos).total_cmp(&b.distance(s.pos)));
            let sea = || {
                (1..18).find_map(|k| {
                    (0..16)
                        .map(|a| {
                            s.pos
                                + Vec2::from_angle(a as f32 * TAU / 16.0)
                                    * (s.kind.r() + k as f32 * 40.0)
                        })
                        .find(|q| t.in_bounds(q.x, q.y) && h(t, *q) < ctx.sea - 2.0)
                })
            };
            if let Some(q) = river.or_else(sea)
                && Ctx::near(&spots, q) > 150.0
            {
                spots.push(q);
            }
        }
        for q in spots {
            out.push(call_then(
                6,
                "fish_add",
                "A fishing spot",
                json!({ "position": xz(q) }),
                |_, ctx, _| {
                    ctx.count("fishing spots");
                    vec![]
                },
            ));
        }
    }
    out
}

// ------------------------------------------------------------------ checks

fn plan_checks(_world: &mut World, ctx: &mut Ctx) -> Vec<Step> {
    let path = ctx.path.to_string_lossy().into_owned();
    let mut out = vec![call_then(
        8,
        "analyze_reachability",
        "Checked who can walk where",
        json!({}),
        |_, ctx, v| {
            if let Some(pct) = v.get("reachable_pct").and_then(|x| x.as_f64()) {
                ctx.report.push(format!(
                    "Players can walk to {pct:.0}% of the land from where they start"
                ));
            }
            let stuck = v
                .get("unreachable_items")
                .and_then(|x| x.as_array())
                .map_or(0, |a| a.len());
            if stuck > 0 {
                ctx.report.push(format!("{stuck} places or things can't be reached on foot (Analysis › Reachability shows them)"));
            }
            vec![]
        },
    )];
    if ctx.plan.zones {
        out.push(call_then(8, "analyze_level_flow", "Checked the level flow between zones", json!({}), |_, ctx, v| {
            let flagged = v.get("flagged").and_then(|x| x.as_u64()).unwrap_or(0);
            let low = ctx.regions.iter().min_by_key(|r| r.levels[0]).map(|r| r.name.clone()).unwrap_or_default();
            let high = ctx.regions.iter().max_by_key(|r| r.levels[1]).map(|r| r.name.clone()).unwrap_or_default();
            let [lo, hi] = ctx.plan.levels;
            ctx.report.push(format!("Levels climb from {lo} in {low} to {hi} in {high}"));
            if flagged > 0 {
                ctx.report.push(format!("{flagged} zone borders step up more than 5 levels at once: Analysis › Level flow shows where a road or buffer would ease them"));
            }
            vec![]
        }));
    }
    out.push(call_then(8, "analyze_content_gaps", "Looked for empty stretches", json!({}), |_, ctx, v| {
        if let Some(d) = v.get("dead_pct").and_then(|x| x.as_f64()) {
            ctx.report.push(if d < 5.0 { "Something to do is never far away".into() } else { format!("{d:.0}% of the land is more than 45 s on foot from anything to do (Analysis › Content gaps)") });
        }
        vec![]
    }));
    out.push(Step::Call {
        stage: 8,
        label: "Saved the world".into(),
        tool: "world_save",
        args: json!({ "path": path }),
        then: Some(Box::new(save_plan)),
        must: false,
    });
    out
}

/// The plan is kept with the world (`plan.json`), and what was made is summed up.
fn save_plan(_: &mut World, ctx: &mut Ctx, _: &Value) -> Vec<Step> {
    if let Ok(s) = serde_json::to_string_pretty(&ctx.plan)
        && let Err(e) = std::fs::write(ctx.path.join("plan.json"), s)
    {
        warn!("SETUP couldn't keep the plan: {e}");
    }
    let order = [
        "capital",
        "towns",
        "villages",
        "roads",
        "fields",
        "zones",
        "dungeons",
        "raids",
        "caves",
        "camps",
        "spawn regions",
        "gathering nodes",
        "fishing spots",
    ];
    let made: Vec<String> = order
        .iter()
        .filter_map(|k| {
            ctx.made.get(k).filter(|n| **n > 0).map(|n| {
                if *k == "capital" {
                    "a capital".to_string()
                } else {
                    format!("{n} {k}")
                }
            })
        })
        .collect();
    if !made.is_empty() {
        ctx.report.insert(0, format!("Made {}", made.join(", ")));
    }
    if ctx.plan.game == GameKind::RealmWar && ctx.plan.realms > 1 {
        ctx.report.push(format!(
            "{} realms, each with its own homeland; the frontier in the middle is contested",
            ctx.plan.realms
        ));
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(x: f32) -> Region {
        Region {
            seed: Vec2::new(x, 0.0),
            site: None,
            realm: Some(0),
            cells: 10,
            biome: None,
            levels: [1, 60],
            zone_type: "open_world",
            name: String::new(),
            zone: None,
        }
    }

    #[test]
    fn levels_rise_along_the_way_and_across_the_water() {
        // 0 – 1 – 2 joined over land, 3 an island 200 m beyond 2
        let regions: Vec<Region> = [0.0, 100.0, 200.0, 400.0].map(region).into();
        let mut adj = vec![HashSet::new(); 4];
        for (a, b) in [(0, 1), (1, 2)] {
            adj[a].insert(b);
            adj[b].insert(a);
        }
        assert_eq!(
            level_order(&regions, &adj, &[0, 1, 2, 3], 0),
            vec![0, 1, 2, 3]
        );
        // from the middle both neighbours come before the island
        let order = level_order(&regions, &adj, &[0, 1, 2, 3], 1);
        assert_eq!((order[0], order[3]), (1, 3));
    }

    #[test]
    fn a_region_outline_follows_its_cells() {
        let n = 8;
        let mut owner = vec![-1; n * n];
        for j in 2..5 {
            for i in 1..6 {
                owner[j * n + i] = 0;
            }
        }
        let r = Raster {
            n,
            g: 10.0,
            half: 40.0,
            owner,
        };
        let loops = region_loops(&r, 0);
        assert_eq!(loops.len(), 1);
        // 5 × 3 cells of 100 m² (the outline runs between cell centres, corners cut)
        let a = geom::signed_area(&loops[0]);
        assert!((a - 1500.0).abs() < 400.0, "{a}");
        assert!(region_loops(&r, 1).is_empty());
    }
}
