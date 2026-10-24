// The shade under the trees (`nature::shade`): how much of the sky a point still sees through the
// tree crowns around and above it, for the sky (ambient) light only — the sun has its shadow maps.
// A square around the viewer on a 2 m grid; outside it, and without the `CANOPY_SHADE` def (a
// material built before the map existed), the light is unchanged. Use:
//
//   let sky = canopy_sky(in.world_position.xyz);
//   pbr_input.diffuse_occlusion *= sky;
//   pbr_input.specular_occlusion *= sky;

struct CanopyShade {
    // x, y: origin (world xz), z: texel size (m), w: texels per side (0: no map)
    grid: vec4<f32>,
    // x: strength, y, z: fade start / end (m from the square's centre), w: how much of the meadow
    // grass the crowns thin out
    tune: vec4<f32>,
    // per texel: pack2x16float(sky visibility, crown top y), pack2x16float(crown base y, cover)
    cells: array<vec2<u32>>,
}

@if(CANOPY_SHADE)
@group(constants::MATERIAL_BIND_GROUP) @binding(120) var<storage, read> canopy_shade: CanopyShade;

// (sky visibility, crown top y, crown base y) of one texel
@if(CANOPY_SHADE)
fn canopy_texel(x: i32, z: i32, n: i32) -> vec3<f32> {
    let c = canopy_shade.cells[u32(z * n + x)];
    let a = unpack2x16float(c.x);
    return vec3<f32>(a.x, a.y, unpack2x16float(c.y).x);
}

@if(CANOPY_SHADE)
fn canopy_cover(x: i32, z: i32, n: i32) -> f32 {
    return unpack2x16float(canopy_shade.cells[u32(z * n + x)].y).y;
}

// The meadow grass kept at xz under the crowns (1 in the open; the forest undergrowth grows there
// instead, `groundcover.rs`).
fn canopy_grass(xz: vec2<f32>) -> f32 {
@if(CANOPY_SHADE) {
    let grid = canopy_shade.grid;
    let n = i32(grid.w);
    if n < 2 {
        return 1.0;
    }
    let g = (xz - grid.xy) / grid.z - 0.5;
    let i = vec2<i32>(floor(g));
    if any(i < vec2<i32>(0)) || any(i >= vec2<i32>(n - 1)) {
        return 1.0;
    }
    let f = g - floor(g);
    let c = mix(
        mix(canopy_cover(i.x, i.y, n), canopy_cover(i.x + 1, i.y, n), f.x),
        mix(canopy_cover(i.x, i.y + 1, n), canopy_cover(i.x + 1, i.y + 1, n), f.x),
        f.y,
    );
    return 1.0 - canopy_shade.tune.w * smoothstep(0.08, 0.45, c);
}
    return 1.0;
}

fn canopy_sky(p: vec3<f32>) -> f32 {
@if(CANOPY_SHADE) {
    let grid = canopy_shade.grid;
    let n = i32(grid.w);
    if n < 2 {
        return 1.0;
    }
    let g = (p.xz - grid.xy) / grid.z - 0.5;
    let i = vec2<i32>(floor(g));
    if any(i < vec2<i32>(0)) || any(i >= vec2<i32>(n - 1)) {
        return 1.0;
    }
    let f = g - floor(g);
    let s = mix(
        mix(canopy_texel(i.x, i.y, n), canopy_texel(i.x + 1, i.y, n), f.x),
        mix(canopy_texel(i.x, i.y + 1, n), canopy_texel(i.x + 1, i.y + 1, n), f.x),
        f.y,
    );
    // under the crowns the shade is full; up through them more and more sky opens up, and above
    // the canopy there is none
    let h = clamp((p.y - s.z) / max(s.y - s.z, 1.0), 0.0, 1.0);
    let sky = mix(s.x, 1.0, h * h);
    let centre = grid.xy + grid.z * grid.w * 0.5;
    let fade = smoothstep(canopy_shade.tune.y, canopy_shade.tune.z, length(p.xz - centre));
    return mix(sky, 1.0, fade);
}
    return 1.0;
}
