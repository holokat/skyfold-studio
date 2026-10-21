//! A designed layout as a graph of spaces with roles — the entrance, the approach, a hub, arenas
//! and set pieces, vistas, secrets, the finale — joined by links (paths, stairs, bridges, the
//! shortcut back), and the checks every plan's layout must pass: pacing, a sightline to the
//! finale, hierarchy, a loop or an optional branch, a shortcut back near the start, purposeful
//! placement of camps, lookouts and ruins, and readable routes with gates between sections.

use bevy::math::Vec2;
use serde::Serialize;
use serde_json::{json, Value};

use super::super::compound::layout::Grid;
use super::super::geom;

/// What a space is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Entrance,
    /// a place on the way in (a foothill camp, the descent's foot, a siege line)
    Approach,
    Hub,
    /// a fighting space (a boss court, a terrace, a chamber, a courtyard)
    Arena,
    /// a landmark space (an island temple)
    SetPiece,
    /// a lookout over the route, the finale in view
    Vista,
    /// off the route, hidden
    Secret,
    Finale,
}

impl Role {
    /// The big spaces (the hierarchy's top).
    pub fn big(self) -> bool {
        matches!(self, Role::Arena | Role::SetPiece | Role::Finale)
    }
}

/// A space: where, how big, how high, and how tall its landmark stands over it.
#[derive(Clone, Debug, Serialize)]
pub struct Space {
    pub role: Role,
    pub name: String,
    #[serde(serialize_with = "ser_v2")]
    pub c: Vec2,
    pub r: f32,
    pub y: f32,
    pub landmark: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    Path,
    Stairs,
    Bridge,
    /// opens from the far side, back near the start
    Shortcut,
    /// a hidden way to a secret
    Secret,
}

/// A link between two spaces: its route, how wide, whether it's the main way, its gate.
#[derive(Clone, Debug, Serialize)]
pub struct Link {
    pub a: usize,
    pub b: usize,
    pub kind: LinkKind,
    #[serde(skip)]
    pub pts: Vec<Vec2>,
    pub width: f32,
    pub main: bool,
    /// a gate on it (where it stands)
    #[serde(skip)]
    pub gate: Option<Vec2>,
    pub one_way: bool,
    /// the steepest grade its route may have (rise over run)
    pub max_grade: f32,
}

/// A designed layout's spaces and links, and what is placed along them.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Graph {
    pub spaces: Vec<Space>,
    pub links: Vec<Link>,
    #[serde(skip)]
    pub camps: Vec<Vec2>,
    /// lookouts (towers, archers' posts): where
    #[serde(skip)]
    pub lookouts: Vec<Vec2>,
    #[serde(skip)]
    pub ruins: Vec<Vec2>,
    /// water: lake outlines and river courses
    #[serde(skip)]
    pub water: Vec<Vec<Vec2>>,
}

fn ser_v2<S: serde::Serializer>(v: &Vec2, s: S) -> Result<S::Ok, S::Error> {
    [(v.x * 10.0).round() / 10.0, (v.y * 10.0).round() / 10.0].serialize(s)
}

/// Pacing: a connector counts as tight beside a space when it is at most this share of its width.
const TIGHT: f32 = 0.5;

impl Graph {
    pub fn add(&mut self, role: Role, name: impl Into<String>, c: Vec2, r: f32, y: f32, landmark: f32) -> usize {
        self.spaces.push(Space { role, name: name.into(), c, r, y, landmark });
        self.spaces.len() - 1
    }
    #[allow(clippy::too_many_arguments)]
    pub fn link(&mut self, a: usize, b: usize, kind: LinkKind, pts: Vec<Vec2>, width: f32, main: bool, gate: Option<Vec2>, max_grade: f32) -> usize {
        self.links.push(Link { a, b, kind, pts, width, main, gate, one_way: kind == LinkKind::Shortcut, max_grade });
        self.links.len() - 1
    }
    fn of(&self, role: Role) -> Option<usize> {
        self.spaces.iter().position(|s| s.role == role)
    }
    /// The main way from the entrance to the finale (spaces in order), over the main links.
    pub fn main_path(&self) -> Vec<usize> {
        let (Some(start), Some(end)) = (self.of(Role::Entrance), self.of(Role::Finale)) else { return vec![] };
        let n = self.spaces.len();
        let mut prev = vec![usize::MAX; n];
        let mut seen = vec![false; n];
        let mut q = std::collections::VecDeque::from([start]);
        seen[start] = true;
        while let Some(s) = q.pop_front() {
            if s == end {
                break;
            }
            for l in self.links.iter().filter(|l| l.main) {
                for (x, y) in [(l.a, l.b), (l.b, l.a)] {
                    if x == s && !seen[y] {
                        seen[y] = true;
                        prev[y] = s;
                        q.push_back(y);
                    }
                }
            }
        }
        if !seen[end] {
            return vec![];
        }
        let mut path = vec![end];
        while *path.last().unwrap() != start {
            path.push(prev[*path.last().unwrap()]);
        }
        path.reverse();
        path
    }
    fn main_link(&self, a: usize, b: usize) -> Option<&Link> {
        self.links.iter().find(|l| l.main && ((l.a == a && l.b == b) || (l.a == b && l.b == a)))
    }
    /// Every check the layout fails (empty: it passes them all).
    pub fn check(&self, g: &Grid) -> Vec<String> {
        let mut bad = vec![];
        let path = self.main_path();
        if path.len() < 3 {
            bad.push("no main way from the entrance to the finale".to_string());
            return bad;
        }
        bad.extend(self.pacing(&path));
        bad.extend(self.sightline(g, &path));
        bad.extend(self.hierarchy());
        bad.extend(self.loops_and_shortcut(&path));
        bad.extend(self.purposeful(g));
        bad.extend(self.readable(g));
        bad
    }

    /// Pacing: tight connectors alternate with open reveals all the way, building to the finale
    /// (the biggest space and the tallest landmark come last).
    fn pacing(&self, path: &[usize]) -> Vec<String> {
        let mut bad = vec![];
        for w in path.windows(2) {
            let (a, b) = (&self.spaces[w[0]], &self.spaces[w[1]]);
            let Some(l) = self.main_link(w[0], w[1]) else { continue };
            if l.width > TIGHT * 2.0 * a.r.min(b.r) {
                bad.push(format!("pacing: the way from {} to {} ({:.0} m wide) is no tighter than the spaces it joins", a.name, b.name, l.width));
            }
        }
        let open = path.iter().filter(|i| self.spaces[**i].r >= 12.0).count();
        if open < 3 {
            bad.push(format!("pacing: only {open} open spaces on the main way"));
        }
        let fin = &self.spaces[*path.last().unwrap()];
        let biggest = self.spaces.iter().filter(|s| s.role != Role::Finale).map(|s| s.r).fold(0.0f32, f32::max);
        if fin.r < biggest * 0.95 {
            bad.push(format!("pacing: the finale ({:.0} m) is smaller than another space ({biggest:.0} m)", fin.r));
        }
        let tallest = self.spaces.iter().filter(|s| s.role != Role::Finale).map(|s| s.landmark).fold(0.0f32, f32::max);
        if fin.landmark < tallest * 0.9 {
            bad.push(format!("pacing: the finale's landmark ({:.0} m) isn't the tallest ({tallest:.0} m)", fin.landmark));
        }
        bad
    }

    /// The finale's landmark is in sight from the entrance or from the first vista.
    fn sightline(&self, g: &Grid, path: &[usize]) -> Vec<String> {
        let fin = &self.spaces[*path.last().unwrap()];
        let target = (fin.c, fin.y + fin.landmark.max(4.0));
        let mut from: Vec<&Space> = vec![&self.spaces[path[0]]];
        let first_vista = path.iter().map(|i| &self.spaces[*i]).find(|s| s.role == Role::Vista).or_else(|| self.spaces.iter().find(|s| s.role == Role::Vista));
        from.extend(first_vista);
        if from.iter().any(|s| sees(g, (s.c, s.y + 1.7), target)) {
            vec![]
        } else {
            vec![format!("sightline: {} can't be seen from the entrance or the first vista", fin.name)]
        }
    }

    /// Hierarchy: a few big set pieces, the connectors small beside them, the finale the
    /// biggest of them.
    fn hierarchy(&self) -> Vec<String> {
        let mut bad = vec![];
        let big: Vec<&Space> = self.spaces.iter().filter(|s| s.role.big()).collect();
        if !(2..=9).contains(&big.len()) {
            bad.push(format!("hierarchy: {} big spaces (2-9 wanted)", big.len()));
        }
        let small = big.iter().map(|s| s.r).fold(f32::MAX, f32::min);
        for l in self.links.iter().filter(|l| l.kind != LinkKind::Bridge) {
            if l.width > small * 0.8 {
                bad.push(format!("hierarchy: a {:.0} m connector beside a {small:.0} m set piece", l.width));
            }
        }
        bad
    }

    /// At least one loop or optional branch, and a shortcut that opens back near the start.
    fn loops_and_shortcut(&self, path: &[usize]) -> Vec<String> {
        let mut bad = vec![];
        let on: Vec<bool> = (0..self.spaces.len()).map(|i| path.contains(&i)).collect();
        let pairs: std::collections::HashSet<(usize, usize)> = self.links.iter().map(|l| (l.a.min(l.b), l.a.max(l.b))).collect();
        let reach = self.reachable();
        let nodes = reach.iter().filter(|x| **x).count();
        let has_loop = pairs.len() >= nodes;
        let branch = (0..self.spaces.len()).any(|i| !on[i] && reach[i]);
        if !has_loop && !branch {
            bad.push("no loop and no optional branch".to_string());
        }
        let start = self.spaces[path[0]].c;
        let half = path.len() / 2;
        let late = |i: usize| match path.iter().position(|x| *x == i) {
            Some(k) => k >= half,
            // (off the main way: as late as the main-way space it hangs from)
            None => self.links.iter().filter(|l| l.a == i || l.b == i).map(|l| if l.a == i { l.b } else { l.a }).filter_map(|j| path.iter().position(|x| *x == j)).any(|k| k >= half),
        };
        let ok = self.links.iter().any(|l| {
            l.kind == LinkKind::Shortcut && l.one_way && l.gate.is_some() && {
                let (x, y) = (l.a, l.b);
                let near = |i: usize| self.spaces[i].c.distance(start) < self.spaces[path[0]].r + 90.0 || path.iter().position(|p| *p == i).is_some_and(|k| k <= 1);
                (late(x) && near(y)) || (late(y) && near(x))
            }
        });
        if !ok {
            bad.push("no shortcut from late in the route back near the start (one way, gated)".to_string());
        }
        bad
    }

    fn reachable(&self) -> Vec<bool> {
        let n = self.spaces.len();
        let mut seen = vec![false; n];
        let Some(s) = self.of(Role::Entrance) else { return seen };
        let mut q = vec![s];
        seen[s] = true;
        while let Some(x) = q.pop() {
            for l in &self.links {
                for (a, b) in [(l.a, l.b), (l.b, l.a)] {
                    if a == x && !seen[b] {
                        seen[b] = true;
                        q.push(b);
                    }
                }
            }
        }
        seen
    }

    fn route_dist(&self, p: Vec2) -> f32 {
        self.links.iter().map(|l| geom::polyline_dist(&l.pts, p).0 - l.width * 0.5).fold(f32::MAX, f32::min)
    }

    /// Camps by water or along the route; lookouts on high ground over a path; ruins along the
    /// old roads.
    fn purposeful(&self, g: &Grid) -> Vec<String> {
        let mut bad = vec![];
        for c in &self.camps {
            let water = self.water.iter().map(|w| geom::polyline_dist(w, *c).0).fold(f32::MAX, f32::min);
            if water > 35.0 && self.route_dist(*c) > 25.0 {
                bad.push(format!("placement: a camp at {c:?} is neither by water nor by the route"));
            }
        }
        for o in &self.lookouts {
            let near = self.links.iter().filter_map(|l| {
                let (d, s) = geom::polyline_dist(&l.pts, *o);
                (d < 90.0).then(|| g.at(geom::polyline_at(&l.pts, s).0))
            });
            let below = near.fold(f32::MIN, f32::max);
            if below == f32::MIN {
                bad.push(format!("placement: a lookout at {o:?} covers no path"));
            } else if g.at(*o) < below + 3.0 {
                bad.push(format!("placement: a lookout at {o:?} isn't above the path it covers"));
            }
        }
        if self.ruins.len() < 6 {
            bad.push(format!("placement: only {} ruins", self.ruins.len()));
        } else {
            let along = self.ruins.iter().filter(|r| self.route_dist(**r) < 22.0).count();
            if (along as f32) < self.ruins.len() as f32 * 0.4 {
                bad.push(format!("placement: only {along} of {} ruins lie along the routes", self.ruins.len()));
            }
        }
        bad
    }

    /// Readable routes: wide enough, never too steep for what they are, and a gate on every main
    /// way into a big space.
    fn readable(&self, g: &Grid) -> Vec<String> {
        let mut bad = vec![];
        for l in &self.links {
            if l.width < 3.0 {
                bad.push(format!("readable: a {:.1} m path", l.width));
            }
            if l.main && self.spaces[l.b].role.big() && l.gate.is_none() {
                bad.push(format!("readable: no gate on the way into {}", self.spaces[l.b].name));
            }
            if l.kind != LinkKind::Bridge {
                let steep = grade(g, &l.pts);
                if steep > l.max_grade + 0.08 {
                    bad.push(format!("readable: the way from {} to {} climbs at {:.0}% (its kind allows {:.0}%)", self.spaces[l.a].name, self.spaces[l.b].name, steep * 100.0, l.max_grade * 100.0));
                }
            }
        }
        bad
    }

    pub fn summary(&self) -> Value {
        let path = self.main_path();
        json!({
            "spaces": self.spaces.iter().enumerate().map(|(i, s)| json!({ "id": i, "role": s.role, "name": s.name, "position": [r1(s.c.x), r1(s.c.y)], "radius_m": r1(s.r), "height_m": r1(s.y), "on_main_way": path.contains(&i) })).collect::<Vec<_>>(),
            "links": self.links.iter().map(|l| json!({ "from": self.spaces[l.a].name, "to": self.spaces[l.b].name, "kind": l.kind, "width_m": r1(l.width), "length_m": r1(geom::polyline_len(&l.pts)), "main": l.main, "gated": l.gate.is_some(), "one_way": l.one_way })).collect::<Vec<_>>(),
            "main_way": path.iter().map(|i| self.spaces[*i].name.clone()).collect::<Vec<_>>(),
            "camps": self.camps.len(),
            "lookouts": self.lookouts.len(),
            "ruins": self.ruins.len(),
        })
    }
}

fn r1(x: f32) -> f32 {
    (x * 10.0).round() / 10.0
}

/// Is `to` in sight from `from` over the ground (each a point and its height)?
pub fn sees(g: &Grid, from: (Vec2, f32), to: (Vec2, f32)) -> bool {
    let len = from.0.distance(to.0);
    let n = (len / 2.0).ceil().max(1.0) as usize;
    (1..n).all(|i| {
        let t = i as f32 / n as f32;
        let d = t * len;
        if d < 8.0 || len - d < 8.0 {
            return true;
        }
        let p = from.0.lerp(to.0, t);
        g.at(p) < from.1 + (to.1 - from.1) * t - 0.3
    })
}

/// The steepest grade along a route, over 8 m stretches.
pub fn grade(g: &Grid, pts: &[Vec2]) -> f32 {
    let len = geom::polyline_len(pts);
    if len < 8.0 {
        return 0.0;
    }
    let n = (len / 4.0).ceil() as usize;
    let at = |s: f32| g.at(geom::polyline_at(pts, s).0);
    let mut worst = 0.0f32;
    for i in 0..n {
        let s = len * i as f32 / n as f32;
        if s + 8.0 > len {
            break;
        }
        worst = worst.max((at(s + 8.0) - at(s)).abs() / 8.0);
    }
    worst
}
