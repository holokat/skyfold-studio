//! 2D geometry for the plan: polygon distance and containment, polyline distance, contours of
//! a scalar field (marching squares, joined into loops), simplification and resampling.

use bevy::math::Vec2;
use std::collections::HashMap;

/// Signed distance from `p` to the closed polygon `pts`: positive inside, negative outside.
pub fn poly_sdf(pts: &[Vec2], p: Vec2) -> f32 {
    let n = pts.len();
    if n < 3 {
        return -1.0e6;
    }
    let mut d2 = f32::MAX;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (pts[j], pts[i]);
        d2 = d2.min(seg_dist2(p, a, b));
        if (b.y > p.y) != (a.y > p.y) && p.x < (a.x - b.x) * (p.y - b.y) / (a.y - b.y) + b.x {
            inside = !inside;
        }
        j = i;
    }
    let d = d2.sqrt();
    if inside { d } else { -d }
}

/// Squared distance from `p` to segment `a`–`b`.
#[inline]
pub fn seg_dist2(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
    (a + ab * t).distance_squared(p)
}

/// Distance from `p` to the polyline `pts`, and how far along it (m) its nearest point lies.
pub fn polyline_dist(pts: &[Vec2], p: Vec2) -> (f32, f32) {
    if pts.len() == 1 {
        return (pts[0].distance(p), 0.0);
    }
    let mut best = (f32::MAX, 0.0);
    let mut along = 0.0;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let ab = b - a;
        let len = ab.length();
        let t = ((p - a).dot(ab) / (len * len).max(1e-12)).clamp(0.0, 1.0);
        let d = (a + ab * t).distance(p);
        if d < best.0 {
            best = (d, along + t * len);
        }
        along += len;
    }
    best
}

pub fn polyline_len(pts: &[Vec2]) -> f32 {
    pts.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// The point `s` metres along a polyline and the direction there.
pub fn polyline_at(pts: &[Vec2], s: f32) -> (Vec2, Vec2) {
    if pts.len() < 2 {
        return (pts.first().copied().unwrap_or(Vec2::ZERO), Vec2::X);
    }
    let mut left = s.max(0.0);
    for w in pts.windows(2) {
        let len = w[0].distance(w[1]);
        let dir = (w[1] - w[0]).normalize_or(Vec2::X);
        if left <= len {
            return (w[0] + dir * left, dir);
        }
        left -= len;
    }
    let n = pts.len();
    (pts[n - 1], (pts[n - 1] - pts[n - 2]).normalize_or(Vec2::X))
}

/// Signed area (positive: counter-clockwise in x, y).
pub fn signed_area(pts: &[Vec2]) -> f32 {
    let n = pts.len();
    let mut a = 0.0;
    for i in 0..n {
        let (p, q) = (pts[i], pts[(i + 1) % n]);
        a += p.x * q.y - q.x * p.y;
    }
    a * 0.5
}

/// Douglas–Peucker on a closed loop (keeps at least a triangle).
pub fn simplify_loop(pts: &[Vec2], tol: f32) -> Vec<Vec2> {
    let n = pts.len();
    if n <= 4 {
        return pts.to_vec();
    }
    // split at the point farthest from the first, simplify both halves
    let far = (1..n)
        .max_by(|a, b| {
            pts[*a]
                .distance_squared(pts[0])
                .total_cmp(&pts[*b].distance_squared(pts[0]))
        })
        .unwrap_or(n / 2);
    let mut a: Vec<Vec2> = pts[..=far].to_vec();
    let mut b: Vec<Vec2> = pts[far..].to_vec();
    b.push(pts[0]);
    a = simplify_open(&a, tol);
    b = simplify_open(&b, tol);
    a.pop();
    b.pop();
    a.extend(b);
    if a.len() < 3 { pts.to_vec() } else { a }
}

/// Douglas–Peucker on an open polyline (keeps both ends).
pub fn simplify_open(pts: &[Vec2], tol: f32) -> Vec<Vec2> {
    if pts.len() <= 2 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    *keep.last_mut().unwrap() = true;
    let mut stack = vec![(0usize, pts.len() - 1)];
    let t2 = tol * tol;
    while let Some((i, j)) = stack.pop() {
        let mut best = (0.0f32, 0usize);
        for k in i + 1..j {
            let d = seg_dist2(pts[k], pts[i], pts[j]);
            if d > best.0 {
                best = (d, k);
            }
        }
        if best.0 > t2 {
            keep[best.1] = true;
            stack.push((i, best.1));
            stack.push((best.1, j));
        }
    }
    pts.iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(p, _)| *p)
        .collect()
}

/// Splits each side of a closed loop into pieces no longer than `step`.
pub fn resample_loop(pts: &[Vec2], step: f32) -> Vec<Vec2> {
    let n = pts.len();
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let k = ((a.distance(b) / step).ceil() as usize).max(1);
        for s in 0..k {
            out.push(a.lerp(b, s as f32 / k as f32));
        }
    }
    out
}

/// The loops where `f` crosses zero over the box `lo`..`hi` sampled every `cell` metres
/// (marching squares with linear interpolation; positive = inside). Loops are closed (the first
/// point isn't repeated) and wind with the inside on their left (counter-clockwise in x, y).
pub fn contours(lo: Vec2, hi: Vec2, cell: f32, f: impl Fn(Vec2) -> f32) -> Vec<Vec<Vec2>> {
    let nx = (((hi.x - lo.x) / cell).ceil() as i32).max(1) + 2;
    let ny = (((hi.y - lo.y) / cell).ceil() as i32).max(1) + 2;
    let o = lo - Vec2::splat(cell);
    let w = (nx + 1) as usize;
    let mut v = vec![0.0f32; w * (ny + 1) as usize];
    for j in 0..=ny {
        for i in 0..=nx {
            // (the outer ring is always outside: loops close)
            let x = if i == 0 || j == 0 || i == nx || j == ny {
                -1.0
            } else {
                f(o + Vec2::new(i as f32, j as f32) * cell)
            };
            v[j as usize * w + i as usize] = if x == 0.0 { -1e-6 } else { x };
        }
    }
    let at = |i: i32, j: i32| v[j as usize * w + i as usize];
    let pos = |i: i32, j: i32| o + Vec2::new(i as f32, j as f32) * cell;
    // edges keyed by (i, j, dir): dir 0 = horizontal edge from (i,j) to (i+1,j), 1 = vertical to (i,j+1)
    let cross = |a: (i32, i32), b: (i32, i32)| {
        let (fa, fb) = (at(a.0, a.1), at(b.0, b.1));
        let t = fa / (fa - fb);
        pos(a.0, a.1).lerp(pos(b.0, b.1), t)
    };
    // segments: from edge key → to edge key (inside on the left)
    let mut next: HashMap<(i32, i32, u8), (i32, i32, u8)> = HashMap::new();
    let mut point: HashMap<(i32, i32, u8), Vec2> = HashMap::new();
    let mut key_pt = |k: (i32, i32, u8)| {
        *point.entry(k).or_insert_with(|| {
            if k.2 == 0 {
                cross((k.0, k.1), (k.0 + 1, k.1))
            } else {
                cross((k.0, k.1), (k.0, k.1 + 1))
            }
        })
    };
    for j in 0..ny {
        for i in 0..nx {
            // corners: 0 (i,j) 1 (i+1,j) 2 (i+1,j+1) 3 (i,j+1); counter-clockwise in x, y
            let c = [
                at(i, j) > 0.0,
                at(i + 1, j) > 0.0,
                at(i + 1, j + 1) > 0.0,
                at(i, j + 1) > 0.0,
            ];
            let idx = c[0] as u8 | (c[1] as u8) << 1 | (c[2] as u8) << 2 | (c[3] as u8) << 3;
            // cell edges: e0 bottom (i,j)-(i+1,j), e1 right (i+1,j)-(i+1,j+1), e2 top (i,j+1)-(i+1,j+1), e3 left (i,j)-(i,j+1)
            let e = [(i, j, 0u8), (i + 1, j, 1u8), (i, j + 1, 0u8), (i, j, 1u8)];
            // segment from edge a to edge b with the inside on the left of a→b
            let segs: &[(usize, usize)] = match idx {
                0 | 15 => &[],
                1 => &[(0, 3)],
                2 => &[(1, 0)],
                3 => &[(1, 3)],
                4 => &[(2, 1)],
                5 => {
                    // saddle: decide by the centre
                    let centre = (at(i, j) + at(i + 1, j) + at(i + 1, j + 1) + at(i, j + 1)) * 0.25;
                    if centre > 0.0 {
                        &[(0, 1), (2, 3)]
                    } else {
                        &[(0, 3), (2, 1)]
                    }
                }
                6 => &[(2, 0)],
                7 => &[(2, 3)],
                8 => &[(3, 2)],
                9 => &[(0, 2)],
                10 => {
                    let centre = (at(i, j) + at(i + 1, j) + at(i + 1, j + 1) + at(i, j + 1)) * 0.25;
                    if centre > 0.0 {
                        &[(3, 0), (1, 2)]
                    } else {
                        &[(1, 0), (3, 2)]
                    }
                }
                11 => &[(1, 2)],
                12 => &[(3, 1)],
                13 => &[(0, 1)],
                14 => &[(3, 0)],
                _ => &[],
            };
            for &(a, b) in segs {
                let (ka, kb) = (e[a], e[b]);
                key_pt(ka);
                key_pt(kb);
                next.insert(ka, kb);
            }
        }
    }
    let mut loops = Vec::new();
    let mut done = std::collections::HashSet::new();
    let keys: Vec<_> = next.keys().copied().collect();
    for start in keys {
        if done.contains(&start) {
            continue;
        }
        let mut lp = Vec::new();
        let mut k = start;
        loop {
            if !done.insert(k) {
                break;
            }
            lp.push(point[&k]);
            match next.get(&k) {
                Some(n) => k = *n,
                None => break,
            }
            if k == start {
                break;
            }
        }
        if lp.len() >= 3 {
            loops.push(lp);
        }
    }
    loops
}

/// Rotates `v` by `a` radians (x towards y).
#[inline]
pub fn rot(v: Vec2, a: f32) -> Vec2 {
    let (s, c) = a.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// Triangulates a simple polygon (ear clipping; any winding). Indices into `pts`.
pub fn triangulate(pts: &[Vec2]) -> Vec<[u32; 3]> {
    let n = pts.len();
    if n < 3 {
        return vec![];
    }
    let ccw = signed_area(pts) > 0.0;
    let mut idx: Vec<usize> = if ccw {
        (0..n).collect()
    } else {
        (0..n).rev().collect()
    };
    let mut out = Vec::with_capacity(n - 2);
    let mut guard = 0;
    while idx.len() > 3 && guard < n * n {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for k in 0..m {
            let (ia, ib, ic) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            let (a, b, c) = (pts[ia], pts[ib], pts[ic]);
            if (b - a).perp_dot(c - b) <= 1e-9 {
                continue;
            }
            let inside = idx
                .iter()
                .any(|&j| j != ia && j != ib && j != ic && in_tri(pts[j], a, b, c));
            if inside {
                continue;
            }
            out.push([ia as u32, ib as u32, ic as u32]);
            idx.remove(k);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    if idx.len() == 3 {
        out.push([idx[0] as u32, idx[1] as u32, idx[2] as u32]);
    }
    out
}

fn in_tri(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let d1 = (b - a).perp_dot(p - a);
    let d2 = (c - b).perp_dot(p - b);
    let d3 = (a - c).perp_dot(p - c);
    d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_distance_and_sign() {
        let sq = [
            Vec2::new(-1.0, -1.0),
            Vec2::new(1.0, -1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(-1.0, 1.0),
        ];
        assert!((poly_sdf(&sq, Vec2::ZERO) - 1.0).abs() < 1e-5);
        assert!((poly_sdf(&sq, Vec2::new(3.0, 0.0)) + 2.0).abs() < 1e-5);
    }

    #[test]
    fn circle_contour_is_one_loop_with_inside_on_the_left() {
        let loops = contours(Vec2::splat(-6.0), Vec2::splat(6.0), 0.25, |p| {
            4.0 - p.length()
        });
        assert_eq!(loops.len(), 1);
        let lp = &loops[0];
        assert!(signed_area(lp) > 0.0, "counter-clockwise");
        let area = signed_area(lp);
        assert!(
            (area - std::f32::consts::PI * 16.0).abs() < 0.6,
            "area {area}"
        );
        for p in lp {
            assert!((p.length() - 4.0).abs() < 0.02);
        }
    }

    #[test]
    fn two_blobs_two_loops() {
        let loops = contours(Vec2::new(-10.0, -4.0), Vec2::new(10.0, 4.0), 0.5, |p| {
            (2.0 - (p - Vec2::new(-5.0, 0.0)).length())
                .max(2.0 - (p - Vec2::new(5.0, 0.0)).length())
        });
        assert_eq!(loops.len(), 2);
    }

    #[test]
    fn ear_clipping_an_l() {
        let l = [
            Vec2::new(0.0, 0.0),
            Vec2::new(4.0, 0.0),
            Vec2::new(4.0, 1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, 4.0),
            Vec2::new(0.0, 4.0),
        ];
        let t = triangulate(&l);
        assert_eq!(t.len(), 4);
        let area: f32 = t
            .iter()
            .map(|t| {
                (l[t[1] as usize] - l[t[0] as usize])
                    .perp_dot(l[t[2] as usize] - l[t[0] as usize])
                    .abs()
                    * 0.5
            })
            .sum();
        assert!((area - 7.0).abs() < 1e-4);
    }
}
