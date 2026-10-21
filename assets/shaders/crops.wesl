// Field crops (`fields::render`). Each vertex arrives relative to its plant's base inside an 8 m
// tile of rows; the instance transform holds the tile's place and the field's row direction.
// Here every plant is seated on the terrain height map, dropped when its base is outside the
// field's outline (a mask in the field's frame), shrunk into the ground towards the crop's draw
// distance, given a little colour of its own and bent in the wind (length-preserving, the same
// travelling gusts as the grass and trees). Optional pieces (hay bales) show in some tiles only.
// The fragment is the standard PBR shader (vertex colour = albedo).

import bevy_pbr::render::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, decompress_vertex},
    view_transformations::position_world_to_clip,
};
import super::noise::{hashf, value_noise2};
import super::wind::gust_field;

struct CropParams {
    // x: terrain half size, y: cell size, z: vertices per side, w: sea level
    grid: vec4<f32>,
    // xy: wind direction, z: wind speed, w: time
    wind: vec4<f32>,
    // xy: gust offset, zw: camera xz
    gust: vec4<f32>,
    // x: camera y
    view: vec4<f32>,
}

struct FieldUniform {
    // origin x, origin z, cos, sin of the row direction
    frame: vec4<f32>,
    // mask extent (m) in the field's frame
    mask: vec4<f32>,
    // x: fade start, y: fade end, z: where the far canopy has risen, w: where it starts (m)
    look: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<storage, read> crop: CropParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var height_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var mask_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var mask_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var<uniform> field: FieldUniform;


fn terrain_height(xz: vec2<f32>) -> f32 {
    let half = crop.grid.x;
    let cell = crop.grid.y;
    let n = i32(crop.grid.z);
    if n < 2 {
        return -1000.0;
    }
    let g = clamp((xz + vec2<f32>(half)) / cell, vec2<f32>(0.0), vec2<f32>(f32(n - 1)));
    let i = vec2<i32>(floor(g));
    let f = g - floor(g);
    let i1 = min(i + vec2<i32>(1), vec2<i32>(n - 1));
    let a = textureLoad(height_tex, i, 0).r;
    let b = textureLoad(height_tex, vec2<i32>(i1.x, i.y), 0).r;
    let c = textureLoad(height_tex, vec2<i32>(i.x, i1.y), 0).r;
    let d = textureLoad(height_tex, i1, 0).r;
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);

    var bend = 0.0;
    var chance = 0.0;
    var slot = vec2<f32>(0.0);
    var col = vec4<f32>(1.0);
@if(VERTEX_UVS_A) {
    bend = v.uv.x;
    chance = v.uv.y;
}
@if(VERTEX_UVS_B) {
    slot = v.uv_b;
}
@if(VERTEX_COLORS) {
    col = v.color;
}

    // the plant's base in the world, and inside the field?
    let base = (world_from_local * vec4<f32>(slot.x, 0.0, slot.y, 1.0)).xz;
    let d = base - field.frame.xy;
    let cs = field.frame.z;
    let sn = field.frame.w;
    let local = vec2<f32>(d.x * cs - d.y * sn, d.x * sn + d.y * cs);
    let muv = local / max(field.mask.xy, vec2<f32>(1.0));
    let in_mask = all(muv >= vec2<f32>(0.0)) && all(muv <= vec2<f32>(1.0)) && textureSampleLevel(mask_tex, mask_samp, muv, 0.0).r > 0.5;
    // optional pieces (a bale) in some tiles only
    let tile = world_from_local[3].xz;
    let shown = chance <= 0.0 || hashf(i32(floor(tile.x * 0.5)), i32(floor(tile.y * 0.5)), 37u) < chance;

    let ground = terrain_height(base);
    let cam = vec3<f32>(crop.gust.z, crop.view.x, crop.gust.w);
    let dist = distance(vec3<f32>(base.x, ground, base.y), cam);
    // plants sink into the ground towards the draw distance (no popping edge); past it the far
    // canopy (marked by chance < 0) rises out of the ground instead
    let canopy = chance < -0.5;
    var fade = 1.0 - smoothstep(field.look.x, field.look.y, dist);
    if canopy {
        fade = smoothstep(field.look.w, field.look.z, dist);
    }

    // the offset from the base, turned with the tile
    let p0 = v.position;
    let turned = (world_from_local * vec4<f32>(p0.x, 0.0, p0.z, 0.0)).xyz;
    var o = vec3<f32>(turned.x, p0.y, turned.z) * fade;

    // wind: plants lean with the gusts, the tops flutter
    let wdir = vec3<f32>(crop.wind.x, 0.0, crop.wind.y);
    let wperp = vec3<f32>(-wdir.z, 0.0, wdir.x);
    let t = crop.wind.w;
    let cell = vec2<i32>(floor(base * 4.0));
    let rnd = hashf(cell.x, cell.y, 53u);
    let gust = smoothstep(0.44, 0.62, gust_field(base - crop.gust.xy));
    let w = clamp(crop.wind.z * 0.1, 0.03, 1.3);
    let ph = rnd * 6.28;
    let lean = w * (gust * 0.42 + 0.14) + sin(t * 1.6 + ph + base.x * 0.17) * w * 0.08;
    let flut = sin(t * 6.1 + ph * 3.0) * (w * 0.05 + 0.01);
    let len = length(o);
    if bend > 0.0 && len > 1.0e-4 {
        let disp = (wdir * lean + wperp * flut) * bend * len;
        o = normalize(o + disp) * len;
    }

    var wp = vec3<f32>(base.x, ground - 0.02, base.y) + o;
    if !in_mask || !shown || (fade <= 0.002 && !canopy) {
        // dropped: a degenerate point under the ground
        wp = vec3<f32>(base.x, ground - 3.0, base.y);
    }
    out.world_position = vec4<f32>(wp, 1.0);
    out.position = position_world_to_clip(wp);

    var nrm = vec3<f32>(0.0, 1.0, 0.0);
@if(VERTEX_NORMALS) {
    nrm = v.normal;
}
    let n = (world_from_local * vec4<f32>(nrm, 0.0)).xyz;
    out.world_normal = normalize(n + wdir * lean * bend * 0.4 + vec3<f32>(0.0, 1.0e-4, 0.0));

    // each plant its own shade; gusts brush a sheen over the tops
    var c = col.rgb * (0.86 + 0.28 * rnd);
    if canopy {
        // from afar a field is never one flat colour: patches where it grew better or worse
        let patchy = value_noise2(base / 9.0, 61u) * 0.6 + value_noise2(base / 3.1, 67u) * 0.4;
        c = col.rgb * mix(0.78, 1.14, patchy);
    }
    c = mix(c, c * vec3<f32>(1.18, 1.16, 1.06), gust * bend * 0.5);
@if(VERTEX_COLORS)
    out.color = vec4<f32>(c, 1.0);
@if(VERTEX_UVS_A)
    out.uv = v.uv;
@if(VERTEX_UVS_B)
    out.uv_b = v.uv_b;
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
    return out;
}
