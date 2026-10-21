// Swimming fish (shared by the fish material's main and prepass vertex shaders). The models are
// 1 long, nose at z = +0.5, +x the left flank (Emberfall's fish format). A travelling wave runs
// down the spine and grows toward the tail (eels: along the whole body; the flatfish: up and
// down, it lies on its side; the ray flaps its wings and trails its whip tail), plus a C-bend
// that curls the body into its turn. Model space, before the instance's transform.
//
// Per fish (its MeshTag): bits 0-7 tail-beat phase, 8-15 bend (−1..1), 16-23 amplitude (0..2),
// bit 24: swim by the real clock (previews) rather than the world's.

struct FishClock {
    // x: the world's swim clock (s); only runs while the world animates
    t: vec4<f32>,
}

struct Swum {
    position: vec3<f32>,
    normal: vec3<f32>,
}

const TAU: f32 = 6.2831853;

// params: x waves along the body (rad), y amplitude (body lengths; the ray's wing beat),
// z tail beats per second, w plan (0 fish, 1 flat, 2 ray, 3 eel)
fn fish_swim(p: vec3<f32>, n: vec3<f32>, params: vec4<f32>, tag: u32, world_t: f32, real_t: f32) -> Swum {
    var out: Swum;
    out.position = p;
    out.normal = n;
    let t = select(world_t, real_t, (tag & 0x1000000u) != 0u);
    let phase = f32(tag & 0xffu) / 255.0 * TAU + t * params.z * TAU;
    let bend = (f32((tag >> 8u) & 0xffu) / 255.0 * 2.0 - 1.0) * 0.35;
    let amp = params.y * f32((tag >> 16u) & 0xffu) / 255.0 * 2.0;
    let plan = params.w;
    if plan > 1.5 && plan < 2.5 {
        // ray: the wings flap (more toward the tips), the tail whips side to side
        let span = abs(p.x) / 0.5;
        let w = phase - p.z * params.x;
        out.position.y += amp * pow(span, 1.4) * sin(w) * 0.5;
        let slope = amp * 1.4 * pow(max(span, 1e-3), 0.4) * sin(w) * sign(p.x);
        out.normal = normalize(vec3<f32>(n.x - slope * n.y, n.y, n.z));
        let tail = max(0.0, -p.z - 0.15);
        out.position.x += amp * 0.3 * tail * tail * 6.0 * sin(phase * 0.7 - tail * 8.0) + bend * tail * tail * 2.0;
        return out;
    }
    let s = clamp(0.5 - p.z, 0.0, 1.0);
    let eel = plan > 2.5;
    let env = select(0.06 + 0.94 * s * s, 0.3 + 0.7 * s, eel);
    let w = phase - s * params.x;
    let lat = amp * env * sin(w) + bend * (s - 0.42) * (s - 0.42) * 2.2;
    // d(lat)/ds: tilts the normals with the wave
    let dl = amp * (select(1.88 * s, 0.7, eel) * sin(w) - env * params.x * cos(w)) + bend * (s - 0.42) * 4.4;
    if plan > 0.5 && plan < 1.5 {
        out.position.y += lat;
        out.normal = normalize(vec3<f32>(n.x, n.y, n.z + dl * n.y));
    } else {
        out.position.x += lat;
        out.normal = normalize(vec3<f32>(n.x, n.y, n.z + dl * n.x));
    }
    return out;
}
