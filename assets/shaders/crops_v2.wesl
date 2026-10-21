// Field crops (`fields::render`). Each vertex arrives relative to its plant's base inside an 8 m
// tile of rows; the instance transform holds the tile's place and the field's row direction.
// Here every plant is seated on the terrain height map, dropped when its base is outside the
// field's outline (a mask in the field's frame), shrunk into the ground towards the crop's draw
// distance, given a little colour of its own and bent in the wind (length-preserving, the same
// travelling gusts as the grass and trees). Optional pieces (hay bales) show in some tiles only.
// The fragment is the standard PBR shader with the crop's surfaces (below).

import bevy_pbr::render::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex},
    view_transformations::position_world_to_clip,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
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
    // metres per repeat of each terrain ground layer
    tiles: array<vec4<f32>, 3>,
    // x: ground photoscans loaded, y: the model's roughness, z: normal-map strength
    tex: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<storage, read> crop: CropParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var height_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var mask_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var mask_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var<uniform> field: FieldUniform;
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var atlas_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(106) var atlas_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(107) var atlas_nrm: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(108) var ground_tex: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(109) var ground_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(110) var ground_det: texture_2d_array<f32>;

// surface kinds (`fields::plants::kind`)
const KIND_GROUND: u32 = 4u;
const KIND_ATLAS: u32 = 20u;
const KIND_TILED: u32 = 21u;
// metres per repeat of a tiled atlas region (the field soil)
const TILED_METRES: f32 = 2.0;
const TULIP_STEM = 81u;
const TULIP_LEAF = 82u;
const TULIP_PETAL = 83u;
const TULIP_CORE = 84u;

/// Tulip vertices use tangent.w as a compact part marker plus their (already instance-scaled)
/// stem height. Other crop and terrain encodings do not occupy this range.
fn tulip_part_from_aux(aux: f32) -> u32 {
    let part = u32(floor(aux));
    if part >= TULIP_STEM && part <= TULIP_CORE {
        return part;
    }
    return 0u;
}

/// Rodrigues' rotation preserves the rigid flower cup while its normals follow the same motion.
fn rotate_about_axis(v: vec3<f32>, axis: vec3<f32>, angle: f32) -> vec3<f32> {
    let s = sin(angle);
    let c = cos(angle);
    return v * c + cross(axis, v) * s + axis * dot(axis, v) * (1.0 - c);
}

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
    var aux = 0.0;
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
@if(VERTEX_TANGENTS) {
    aux = v.tangent.w;
}
    let tulip_part = tulip_part_from_aux(aux);

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

    // Existing crops keep their established lean and flutter. Tagged tulips bend their stems
    // smoothly and carry the cup as a coherent, gently delayed head.
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
    var tulip_axis = vec3<f32>(0.0, 1.0, 0.0);
    var tulip_normal_turn = 0.0;
    var tulip_wind = 0.0;
    if tulip_part == 0u {
        let len = length(o);
        if bend > 0.0 && len > 1.0e-4 {
            let disp = (wdir * lean + wperp * flut) * bend * len;
            o = normalize(o + disp) * len;
        }
    } else {
        // Zero wind is exactly still. Per-plant phase, strength and frequency are keyed by the
        // world base while the existing gust field supplies slow travelling motion.
        tulip_wind = clamp(crop.wind.z * 0.1, 0.0, 1.3);
        let axis_raw = cross(vec3<f32>(0.0, 1.0, 0.0), wdir);
        let axis_length = length(axis_raw);
        if tulip_wind > 1.0e-5 && axis_length > 1.0e-5 {
            tulip_axis = axis_raw / axis_length;
            let strength = mix(0.82, 1.14, hashf(cell.x, cell.y, 149u));
            let frequency = mix(0.72, 1.08, hashf(cell.x, cell.y, 151u));
            let phase = hashf(cell.x, cell.y, 157u) * 6.2831853;
            let gust_wave = 0.15 + gust * 0.24
                + sin(t * frequency + phase + dot(base, vec2<f32>(0.19, -0.13))) * 0.035;
            let stem_turn = min(tulip_wind * strength * max(gust_wave, 0.04), 0.20);
            let stem_h = max(fract(aux), 0.05) * fade;
            let stem_length = max(stem_h, 0.001);

            if tulip_part == TULIP_STEM {
                let along_stem = clamp(o.y / stem_length, 0.0, 1.0);
                tulip_normal_turn = stem_turn * pow(along_stem, 1.35);
                o = rotate_about_axis(o, tulip_axis, tulip_normal_turn);
            } else if tulip_part == TULIP_LEAF {
                let along_leaf = clamp(o.y / stem_length, 0.0, 1.0);
                let leaf_lag = sin(t * frequency * 1.65 + phase + p0.x * 11.0 - p0.z * 7.0)
                    * tulip_wind * 0.035;
                tulip_normal_turn = stem_turn * (0.30 + along_leaf * 0.48) + leaf_lag;
                o = rotate_about_axis(o, tulip_axis, tulip_normal_turn);
            } else {
                // Petals and the recessed core share the rotated stem tip. A small delayed
                // rotation happens around that tip, so no individual-petal flutter can melt the cup.
                let tip = rotate_about_axis(vec3<f32>(0.0, stem_h, 0.0), tulip_axis, stem_turn);
                let delayed_wave = 0.15 + gust * 0.24
                    + sin((t - 0.18) * frequency + phase + dot(base, vec2<f32>(0.19, -0.13))) * 0.035;
                let delayed_turn = min(tulip_wind * strength * max(delayed_wave, 0.04), 0.20) * 0.93;
                o = tip + rotate_about_axis(o - vec3<f32>(0.0, stem_h, 0.0), tulip_axis, delayed_turn);
                tulip_normal_turn = delayed_turn;
            }
        }
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
    var n = (world_from_local * vec4<f32>(nrm, 0.0)).xyz;
    if tulip_part != 0u {
        n = rotate_about_axis(n, tulip_axis, tulip_normal_turn);
        out.world_normal = normalize(n + vec3<f32>(0.0, 1.0e-4, 0.0));
    } else {
        out.world_normal = normalize(n + wdir * lean * bend * 0.4 + vec3<f32>(0.0, 1.0e-4, 0.0));
    }

    // each plant its own shade; gusts brush a sheen over the tops
    var c = col.rgb * (0.86 + 0.28 * rnd);
    if canopy {
        // from afar a field is never one flat colour: patches where it grew better or worse
        let patchy = value_noise2(base / 9.0, 61u) * 0.6 + value_noise2(base / 3.1, 67u) * 0.4;
        c = col.rgb * mix(0.78, 1.14, patchy);
    }
    var gust_sheen = gust * bend * 0.5;
    if tulip_part != 0u {
        gust_sheen *= tulip_wind;
    }
    c = mix(c, c * vec3<f32>(1.18, 1.16, 1.06), gust_sheen);
// (alpha carries the part's roughness)
@if(VERTEX_COLORS)
    out.color = vec4<f32>(c, col.a);
@if(VERTEX_UVS_A)
    out.uv = v.uv;
@if(VERTEX_UVS_B)
    out.uv_b = v.uv_b;
@if(VERTEX_TANGENTS)
    out.world_tangent = v.tangent;
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
    return out;
}

// Surfaces: the crop atlas (the shared one the procedural plants are painted from, or a
// Blender-built crop model's own) — colour, cut-out alpha, normal map, read in a frame made from
// screen-space derivatives, so no tangent frame is stored (the tangent carries the atlas uv and the
// surface kind) — the terrain's own photoscanned ground layers (ridges, beds, furrows: they match
// the field's ground), or plain vertex colour. Roughness comes with the vertex colour's alpha.
@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    var tan = vec4<f32>(0.0);
@if(VERTEX_TANGENTS) {
    tan = in.world_tangent;
}
    var tint = vec3<f32>(1.0);
    var rough = 0.7;
@if(VERTEX_COLORS) {
    tint = in.color.rgb;
    rough = in.color.a;
}
    // (vertex colour alpha is the roughness, not opacity)
    pbr_input.material.base_color = vec4<f32>(pbr_input.material.base_color.rgb, 1.0);
    pbr_input.material.perceptual_roughness = clamp(rough, 0.05, 1.0);
    // (derivatives first, while every fragment of the quad is still running)
    let wp = in.world_position.xyz;
    let dpx = dpdx(wp);
    let dpy = dpdy(wp);
    let uv = tan.xy;
    let duvx = dpdx(uv);
    let duvy = dpdy(uv);
    let kind = u32(tan.z + 0.5);
    let tulip_part = tulip_part_from_aux(tan.w);
    // The existing material flag compiles Bevy's diffuse-transmission lobe. Restrict it to the
    // thin tulip parts: petals use a restrained enhanced value, leaves are quieter, and the stem and
    // recessed core remain opaque.
    if tulip_part == TULIP_PETAL {
        pbr_input.material.diffuse_transmission = 0.38;
    } else if tulip_part == TULIP_LEAF {
        pbr_input.material.diffuse_transmission = 0.08;
    } else if tulip_part == TULIP_STEM || tulip_part == TULIP_CORE {
        pbr_input.material.diffuse_transmission = 0.0;
    }
    // Crop materials skip the normal prepass because their vertices are placed here.  A camera
    // with SSAO therefore gives `pbr_input.N` the normal already in that buffer (usually the
    // terrain behind a petal), not this fragment's interpolated normal.  Start from the latter
    // before building the atlas normal-map frame.  `VERTEX_TANGENTS` is our UV/kind payload, so
    // Bevy's usual no-normal-map back-face correction does not run; apply it here as well.
    var n = normalize(pbr_input.world_normal);
    if !is_front {
        n = -n;
    }
    var keep = true;
    if kind == KIND_ATLAS || kind == KIND_TILED {
        var suv = uv;
        var gx = duvx;
        var gy = duvy;
        if kind == KIND_TILED {
            // the region (corner tan.xy, width tan.w, height to width a tenth of the kind's
            // fraction) repeated over the world's x/z; the gradients come from the unwrapped
            // coordinates, so no seam in the mips
            var aspect = (tan.z - 21.0) * 10.0;
            if aspect < 0.02 {
                // (an older build without the aspect: a region of square texels)
                let dims = vec2<f32>(textureDimensions(atlas_tex));
                aspect = dims.x / dims.y;
            }
            let size = vec2<f32>(tan.w, tan.w * aspect);
            let w = wp.xz / TILED_METRES;
            suv = tan.xy + fract(w) * size;
            gx = dpx.xz / TILED_METRES * size;
            gy = dpy.xz / TILED_METRES * size;
        }
        let a = textureSampleGrad(atlas_tex, atlas_samp, suv, gx, gy);
        keep = a.a >= 0.5;
        let tn = textureSampleGrad(atlas_nrm, atlas_samp, suv, gx, gy).xy * 2.0 - 1.0;
        let t1 = cross(dpy, n);
        let t2 = cross(n, dpx);
        let tt = t1 * gx.x + t2 * gy.x;
        let bb = t1 * gx.y + t2 * gy.y;
        let inv = inverseSqrt(max(max(dot(tt, tt), dot(bb, bb)), 1.0e-12));
        let tz = sqrt(max(1.0 - dot(tn, tn), 0.0));
        let k = field.tex.z;
        n = normalize(tt * inv * tn.x * k + bb * inv * tn.y * k + n * tz);
        pbr_input.material.base_color = vec4<f32>(a.rgb * tint, 1.0);
        if kind == KIND_TILED {
            pbr_input.material.diffuse_transmission = 0.0;
        }
    } else if kind == KIND_GROUND && field.tex.x > 0.5 {
        let layer = u32(tan.w + 0.5);
        let tile = field.tiles[layer / 4u][layer % 4u];
        let guv = wp.xz / tile;
        let g = textureSampleGrad(ground_tex, ground_samp, guv, i32(layer), dpx.xz / tile, dpy.xz / tile);
        let d = textureSampleGrad(ground_det, ground_samp, guv, i32(layer), dpx.xz / tile, dpy.xz / tile);
        let pert = d.xy * 2.0 - 1.0;
        n = normalize(n + vec3<f32>(pert.x, 0.0, pert.y) * field.tex.z);
        pbr_input.material.base_color = vec4<f32>(g.rgb * tint, 1.0);
        pbr_input.material.perceptual_roughness = clamp(d.z, 0.05, 1.0);
        pbr_input.material.diffuse_transmission = 0.0;
        pbr_input.diffuse_occlusion = min(pbr_input.diffuse_occlusion, vec3<f32>(d.w)); // keeps the screen-space AO
    }
    if !keep {
        discard;
    }
    pbr_input.N = n;
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
