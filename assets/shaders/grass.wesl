// Meadow grass blades (after tidewater's GrassField). Each vertex arrives relative to its clump
// slot inside an 8 m cell; the instance transform holds the cell origin. Placement on the terrain
// height map, density culling, distance thinning with constant coverage, length-preserving wind
// bend with travelling gusts, shaped normals and base→tip colour all happen here. The fragment
// keeps the standard PBR path, with a realistic-only thin-leaf response and root occlusion.
// The density map carries the biome's grass style per terrain vertex: R density, G dryness,
// B tint (0 cold blue-green … 0.5 neutral … 1 warm yellow-green), A blade height (×2).
// Sparse grass grows in clumps with bare ground between them rather than thinning evenly.

import bevy_pbr::render::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex},
    view_transformations::position_world_to_clip,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
};
import super::noise::{fbm2, hashf};
import super::wind::gust_field;

struct GrassParams {
    // x: terrain half size, y: cell size, z: vertices per side, w: sea level
    grid: vec4<f32>,
    // xy: wind direction, z: wind strength, w: time
    wind: vec4<f32>,
    // xy: gust offset, zw: grass LOD origin xz (eye while walking, editor focus while orbiting)
    gust: vec4<f32>,
    // x: camera y, y: far fade start, z: far fade end, w: height scale
    view: vec4<f32>,
    base: vec4<f32>,
    tip: vec4<f32>,
    dry: vec4<f32>,
    // tip colours for cold (boreal, alpine) and warm (tropical) grass
    cold: vec4<f32>,
    warm: vec4<f32>,
    // x: grass amount (density multiplier), y: realistic art treatment, z: lushness (0..1),
    // w: editor orbit preview uses focus-centred horizontal LOD instead of eye distance
    amount: vec4<f32>,
}

// rewritten every frame in a storage buffer, so the material itself never changes
@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<storage, read> grass: GrassParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var height_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var density_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var density_samp: sampler;

const TAU: f32 = 6.2831853;

fn terrain_height(xz: vec2<f32>) -> f32 {
    let half = grass.grid.x;
    let cell = grass.grid.y;
    let n = i32(grass.grid.z);
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
    let origin = world_from_local[3].xyz;

    var hf = 0.0;
    var across = 0.0;
    var slot = vec2<f32>(0.0);
    var attr = vec4<f32>(0.0);
@if(VERTEX_UVS_A) {
    hf = v.uv.x;
    across = v.uv.y;
}
@if(VERTEX_UVS_B) {
    slot = v.uv_b;
}
@if(VERTEX_COLORS) {
    attr = v.color;
}
    let rnd = attr.r;
    let realistic = grass.amount.y > 0.5;
    // This is an appearance control, deliberately independent from the existing amount dial.
    // At zero every formula below reduces to the established meadow recipe.
    let lushness = clamp(grass.amount.z, 0.0, 1.0);
    // 0.0, 0.5 and 1.0 are the original hand-painted tiers. The 1.5 offset identifies
    // realistic-only fine blades without repacking the existing mesh attributes.
    let realistic_blade = attr.g >= 1.5;
    let tier = select(attr.g * 2.0, (attr.g - 1.5) * 2.0, realistic_blade);
    let slot_rand = attr.b;
    let az = attr.a * TAU;

    let sxz = origin.xz + slot;
    let half = grass.grid.x;
    let n = grass.grid.z;
    let g = (sxz + vec2<f32>(half)) / grass.grid.y;
    let dd = textureSampleLevel(density_tex, density_samp, (g + vec2<f32>(0.5)) / n, 0.0);
    // Keep the original amount response for the shared blade layer. Above amount 1, the added
    // fine layer continues to fill in, so the existing 1..2 dial really adds realistic cover.
    let cover = dd.r * grass.amount.x;
    // The second half of the amount dial should visibly close meadow gaps even where the source
    // density texture already reaches one. It is kept separate from lushness, which controls the
    // layered species treatment below.
    let abundance = clamp(grass.amount.x - 1.0, 0.0, 1.0);
    let legacy_density = min(cover, 1.0);
    let realistic_density = min(dd.r * min(grass.amount.x, 1.0), 1.0);
    let density = select(legacy_density, realistic_density, realistic);
    let dry = dd.g;
    let tint = dd.b;
    let style_h = dd.a * 2.0;
    // clump noise: thin grass survives in patches (largest clumps where the noise is low)
    let nz = fbm2(sxz / 6.5, 3u, 9u);
    let nz2 = fbm2(sxz / 2.3, 2u, 13u);
    let clump = nz * 0.55 + nz2 * 0.45;
    // A second, metre-scale field organizes the lush-only understorey and tall leaves into
    // recognisable tufts. It deliberately does not affect the legacy recipe at zero.
    let tuft_noise = fbm2(sxz / 1.35, 3u, 41u);
    let tuft_core = 1.0 - smoothstep(0.32, 0.73, tuft_noise);
    let meadow_patch = 1.0 - smoothstep(0.30, 0.72, fbm2(sxz / 8.5, 3u, 53u));
    let cid = vec2<i32>(floor(sxz * 16.0));
    // The shared painted recipe keeps its original threshold. Realistic grass fills more of a
    // lush field but retains patch-scale variation instead of becoming an even carpet.
    let clump_cut = select(0.24 + density * 0.56, 0.29 + density * 0.66, realistic);
    // Lush fields close a few gaps, but keep the low-frequency clump field that stops them
    // reading as an even artificial carpet.
    let rich_clump_cut = min(
        clump_cut
            + abundance * (0.09 + 0.07 * meadow_patch)
            + lushness * (0.08 + 0.17 * meadow_patch - 0.08 * (1.0 - meadow_patch)),
        0.99,
    );
    var present = density > 0.03 && hashf(cid.x, cid.y, 71u) * 0.35 + clump * 0.65 < rich_clump_cut;
    if realistic_blade {
        // Each basal blade has its own offset root. Do not also subject it to its parent clump's
        // threshold, which creates isolated starbursts and exposes bare green ground. The density
        // texture supplies the authored sparse areas and broad natural variation instead.
        // Lushness can fill a painted forest floor's deliberately light density contribution,
        // without inventing grass on true zero-density water, roads, or masks. At zero this is
        // exactly the original threshold.
        let fertile_start = mix(0.08, 0.035, lushness);
        let fertile_end = mix(0.55, 0.28, lushness);
        let fertile = smoothstep(fertile_start, fertile_end, dd.r);
        // The first unit of the existing amount dial grows the basal layer from zero to its
        // lush default; the second fills its remaining gaps.
        let amount_to_default = clamp(grass.amount.x, 0.0, 1.0);
        let baseline_cover = fertile * amount_to_default * (0.74 + abundance * 0.26);
        // The painted recipe has no fine layer at zero. At full lushness it uses these existing
        // slots as short, visible brush-strokes; realistic grass fills a few remaining gaps.
        let painted_cover = fertile * amount_to_default * lushness * (0.46 + 0.36 * tuft_core + abundance * 0.18);
        let rich_realistic_cover = max(baseline_cover, fertile * amount_to_default * (0.68 + 0.24 * tuft_core + 0.06 * meadow_patch + abundance * 0.12));
        let fine_cover = select(painted_cover, mix(baseline_cover, rich_realistic_cover, lushness), realistic);
        present = density > 0.03 && (realistic || lushness > 0.001) && fract(rnd * 17.17 + slot_rand * 3.1) < fine_cover;
    }

    let ground = terrain_height(sxz);
    let preview_orbit = grass.amount.w > 0.5;
    let eye = vec3<f32>(grass.gust.z, grass.view.x, grass.gust.w);
    let eye_dist = distance(vec3<f32>(sxz.x, ground, sxz.y), eye);
    // Orbiting should preview the field around the point being edited, even when its eye is well
    // above the ground. Walking retains the true 3D eye distance so sloped terrain still grades
    // the LOD and fade exactly as before.
    let focus_dist = distance(sxz, grass.gust.zw);
    let dist = select(eye_dist, focus_dist, preview_orbit);
    // Tiers thin out with distance. The realistic recipe carries more fine survivors, reducing
    // the width compensation that made the far meadow read as broad ribbon blades.
    let f2_eye = 1.0 - smoothstep(11.0, 16.0, dist);
    let f1_eye = 1.0 - smoothstep(31.0, 40.0, dist);
    // High-orbit preview allocates only the far mesh. Its compensation must not count the
    // missing near and mid tiers as survivors when the focus happens to be at distance zero.
    let f2 = select(f2_eye, 0.0, preview_orbit);
    let f1 = select(f1_eye, 0.0, preview_orbit);
    let cut = mix(grass.view.y, grass.view.z, slot_rand);
    let f0 = 1.0 - smoothstep(cut - 5.0, cut, dist);
    var tf = f0;
    if tier > 1.5 {
        tf = f2;
    } else if tier > 0.5 {
        tf = f1;
    }
    let legacy_comp = 7.0 / max(2.0 * f0 + 3.0 * f1 + 2.0 * f2, 0.6);
    let realistic_comp = min(28.0 / max(5.0 * f0 + 7.0 * f1 + 16.0 * f2, 1.0), 1.6);
    let base_comp = select(legacy_comp, realistic_comp, realistic);
    // Extra short blades carry the coverage in a rich meadow. Cap the far survivor widening so
    // the transition does not turn into a band of broad ribbons.
    let rich_comp = min(base_comp, 1.12 + 0.28 * f0);
    let comp = mix(base_comp, rich_comp, lushness);

    // clump size from patch noise (tallest in the middle of a clump), density, dryness (dry grass
    // grows taller and thinner) and the biome's blade height
    let legacy_height = (0.3 + 0.44 * (1.0 - clump) + 0.1 * nz2) * (0.6 + 0.4 * density) * (1.0 + 0.35 * dry) * style_h * grass.view.w;
    // Original blades remain shorter spreading tufts. The independently rooted fine layer rises
    // through them with varied knee-height leaves, forming a dense meadow rather than repeated
    // radial stars.
    let tall_tuft = 0.55 + 0.30 * pow(fract(rnd * 5.13 + slot_rand * 1.7), 5.0);
    let basal_tuft = 0.75 + 0.35 * pow(fract(rnd * 9.71 + slot_rand * 2.3), 3.0);
    let leaf_height = select(tall_tuft, basal_tuft, realistic_blade);
    let realistic_height = (0.20 + 0.58 * (1.0 - clump) + 0.17 * nz2) * (0.7 + 0.3 * density) * (1.0 + 0.28 * dry) * style_h * grass.view.w * leaf_height;
    let clump_core = 1.0 - smoothstep(0.25, 0.75, clump);
    // When the painted world borrows the fine slots, keep them as shorter brush-strokes beneath
    // its familiar larger blades. The realistic path retains its established height profile.
    let painted_height = select(legacy_height, legacy_height * (0.58 + 0.14 * clump_core), realistic_blade);
    let base_height = select(painted_height, realistic_height, realistic);
    // Richness changes the silhouette as well as coverage: clustered leaves rise higher, while
    // a small deterministic subset becomes the irregular tall accents seen in a lived-in meadow.
    let accent = smoothstep(0.86, 0.98, fract(rnd * 9.31 + slot_rand * 4.17));
    // A sparse family of flowering stalks breaks the repeated crisscross silhouette: their
    // higher, bright seed brush is deliberately limited to the shared outer blades, leaving the
    // dense fine layer to read as ground cover.
    let stalk = select(
        smoothstep(0.86, 0.96, fract(rnd * 12.71 + slot_rand * 5.63)),
        0.0,
        realistic_blade,
    );
    let painted_gain = 0.22 + 0.25 * clump_core + 0.34 * accent;
    let realistic_gain = 0.28 + 0.32 * clump_core + 0.48 * accent;
    let rich_stratum = select(
        1.14 + 0.36 * tuft_core + 0.16 * accent,
        0.48 + 0.30 * tuft_core + 0.12 * meadow_patch,
        realistic_blade,
    );
    let rich_height = base_height
        * (1.0 + lushness * select(painted_gain, realistic_gain, realistic))
        * mix(1.0, rich_stratum, lushness)
        * (1.0 + lushness * stalk * 0.42);
    // Keep rare stalks within meadow scale even when wet/tall biome multipliers coincide.
    // The baseline remains exact: this limit is never lower than its own unmodified height.
    // With the mesh centre-line envelope this also stays inside the CPU's 6 m rich bounds.
    let height = min(rich_height, max(base_height, 2.0));
    let yaw = fract(slot_rand * 7.13) * TAU;
    let cy = cos(yaw);
    let sy = sin(yaw);
    let p0 = v.position;
    var o = vec3<f32>(p0.x * cy - p0.z * sy, p0.y, p0.x * sy + p0.z * cy) * height;
    // The mesh has centre-line rows only at 0, .3, .62 and 1. Shape the sparse flowering
    // family around its real .62 row: a straighter narrow stem rises into a lifted seed brush,
    // instead of trying to create detail between vertices that do not exist.
    let stalk_shape = lushness * stalk;
    let stalk_straighten = stalk_shape * smoothstep(0.08, 0.62, hf);
    let stalk_spread = 1.0 - 0.62 * stalk_straighten;
    o = vec3<f32>(o.x * stalk_spread, o.y, o.z * stalk_spread);
    let seed_lift = smoothstep(0.26, 0.62, hf) * (1.0 - smoothstep(0.62, 1.0, hf));
    o.y += height * stalk_shape * seed_lift * 0.12;
    if realistic_blade {
        // Meadow leaves rise through the short tufts; preserving some sideways bend prevents a
        // uniform picket-fence silhouette.
        o = vec3<f32>(o.x * 0.72, o.y * 1.04, o.z * 0.72);
    }
    let baz = az + yaw;
    let side = vec3<f32>(-sin(baz), 0.0, cos(baz));
    let legacy_bw = 0.021 * (0.8 + 0.5 * rnd) * (1.0 - pow(hf, 1.6)) * (0.75 + 0.25 * (1.0 - hf)) * comp * tf * (1.0 - 0.3 * dry);
    let fine_scale = select(1.0, 0.84, realistic_blade);
    let realistic_bw = 0.0105 * (0.78 + 0.42 * rnd) * (1.0 - pow(hf, 1.25)) * fine_scale * comp * tf * (1.0 - 0.22 * dry);
    let painted_bw = select(legacy_bw, legacy_bw * 0.56, realistic_blade);
    let base_bw = select(painted_bw, realistic_bw, realistic);
    // Fuller bases make the added layer read as cover at walking height; tall accents remain
    // narrow so they do not become flat cards against the horizon.
    let rich_width = 1.0 + 0.28 * (1.0 - hf) * (1.0 - 0.45 * accent);
    let rich_leaf_width = select(
        1.20 + 0.56 * tuft_core + 0.16 * (1.0 - hf),
        0.72 + 0.38 * tuft_core,
        realistic_blade,
    );
    var bw = base_bw * mix(1.0, rich_width * rich_leaf_width, lushness);
    // Keep the stalk narrow through the .3 row, then make the actual .62 mesh row a loose pale
    // seed brush. Its next triangle tapers to the tip, giving a meadow plume rather than a reed.
    let stalk_stem = stalk * smoothstep(0.06, 0.38, hf) * (1.0 - smoothstep(0.46, 0.62, hf));
    bw *= 1.0 - lushness * stalk_stem * 0.48;
    let seed_brush = stalk * smoothstep(0.46, 0.61, hf) * (1.0 - smoothstep(0.63, 0.86, hf));
    bw *= 1.0 + lushness * seed_brush * 1.8;
    // Seed heads are small botanical accents. Without a physical width limit, the wider
    // painted blade recipe turns this upper row into large flat diamonds.
    let seed_width = select(0.014, 0.008, realistic);
    bw = mix(bw, min(bw, seed_width), lushness * seed_brush);

    // wind: length-preserving bend towards the wind, travelling gusts, flutter
    let wdir = vec3<f32>(grass.wind.x, 0.0, grass.wind.y);
    let t = grass.wind.w;
    let gp = sxz - grass.gust.xy;
    let gust = smoothstep(0.44, 0.62, gust_field(gp));
    let w = clamp(grass.wind.z * 0.1, 0.03, 1.3);
    let ph = slot_rand * 6.28;
    let bend = w * (gust * 0.55 + 0.22) + sin(t * 1.9 + ph + sxz.x * 0.2) * w * 0.1;
    let flut = sin(t * 7.3 + ph * 3.0 + rnd * 20.0) * (w * 0.06 + 0.015)
        + sin(t * 11.7 + ph * 5.7 + sxz.y * 0.37) * w * lushness * (0.025 + 0.045 * tuft_core);
    let wperp = vec3<f32>(-wdir.z, 0.0, wdir.x);
    let len = length(o);
    let disp = (wdir * bend + wperp * flut) * hf * hf * len;
    o = normalize(o + disp + vec3<f32>(0.0, 1.0e-5, 0.0)) * len;

    var wp = vec3<f32>(sxz.x, ground - 0.03, sxz.y) + o + side * (across * bw);
    if !present || f0 <= 0.001 || tf <= 0.001 {
        // collapse culled blades to a degenerate point under the ground
        wp = vec3<f32>(sxz.x, ground - 2.0, sxz.y);
    }
    out.world_position = vec4<f32>(wp, 1.0);
    out.position = position_world_to_clip(wp);

    // The painted path keeps its rounded ground-like normal. Realistic blades retain a vertical
    // component but face out from their bend, so sunlight describes the individual leaf planes.
    let lean = vec3<f32>(o.x, 0.0, o.z) / max(length(o.xz), 1.0e-4);
    let legacy_normal = normalize(vec3<f32>(0.0, 1.0, 0.0) + lean * 0.25 + side * across * 0.35);
    let realistic_normal = normalize(vec3<f32>(0.0, 0.49, 0.0) + lean * 0.84 + side * 0.12);
    out.world_normal = select(legacy_normal, realistic_normal, realistic);

    // colour: dark base → sunlit tip, straw where dry, a few dead blades, gust sheen
    let lush = mix(mix(grass.cold.rgb, grass.tip.rgb, smoothstep(0.0, 0.5, tint)), grass.warm.rgb, smoothstep(0.5, 1.0, tint));
    let tipc = mix(lush, grass.dry.rgb, clamp(dry * 1.1 + (rnd - 0.5) * 0.18, 0.0, 1.0));
    var col = mix(grass.base.rgb, tipc, smoothstep(0.0, 0.85, hf));
    col *= mix(0.55, 1.0, smoothstep(0.0, 0.7, hf));
    let dead = step(0.94, fract(rnd * 13.7));
    col = mix(col, grass.dry.rgb * 0.75, dead * 0.7);
    let painted_highlight = vec3<f32>(1.05, 1.07, 0.84);
    let realistic_highlight = vec3<f32>(1.03, 1.05, 0.87);
    let rich_highlight = select(painted_highlight, realistic_highlight, realistic);
    col = mix(col, col * rich_highlight, lushness * smoothstep(0.22, 0.9, hf) * (0.3 + 0.7 * clump_core));
    // Preserve climate/dryness at the tips, but give lush tufts a dark cool base and reserve
    // yellow-green for a sparse set of sunlit outer leaves instead of every blade.
    let realistic_deep = vec3<f32>(0.035, 0.115, 0.018);
    let realistic_leaf = vec3<f32>(0.15, 0.34, 0.045);
    let painted_deep = vec3<f32>(0.055, 0.17, 0.035);
    let painted_leaf = vec3<f32>(0.16, 0.40, 0.065);
    let deep = select(painted_deep, realistic_deep, realistic);
    let leaf = select(painted_leaf, realistic_leaf, realistic);
    let leaf_lit = smoothstep(0.28, 0.92, hf) * (0.38 + 0.62 * tuft_core);
    let rich_col = mix(deep, mix(leaf, col, 0.32 + 0.24 * accent), leaf_lit);
    col = mix(col, rich_col, lushness * (0.52 + 0.38 * meadow_patch));
    // A metre-scale cool/warm variation separates neighbouring loose tufts without repainting
    // the biome's authored climate tint or making the meadow read as one coloured lawn.
    let tuft_warmth = smoothstep(0.34, 0.70, fbm2(sxz / 2.6, 2u, 73u));
    let tuft_tone = mix(vec3<f32>(0.76, 0.97, 1.02), vec3<f32>(1.10, 1.04, 0.78), tuft_warmth);
    col = mix(col, col * tuft_tone, lushness * 0.30 * (0.35 + 0.65 * tuft_core));
    let seed_pale = select(vec3<f32>(0.68, 0.66, 0.34), vec3<f32>(0.62, 0.67, 0.42), realistic);
    let seed_col = mix(tipc, seed_pale, 0.60 + 0.18 * dry);
    col = mix(col, seed_col, lushness * seed_brush * 0.82);
    let gust_sheen = select(0.45, 0.18, realistic);
    col = mix(col, col * vec3<f32>(1.3, 1.28, 1.1) + vec3<f32>(0.02), gust * smoothstep(0.15, 0.9, hf) * gust_sheen);

@if(VERTEX_COLORS)
    out.color = vec4<f32>(col, 1.0);
@if(VERTEX_UVS_A)
    out.uv = v.uv;
@if(VERTEX_UVS_B)
    out.uv_b = v.uv_b;
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
    return out;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    let realistic = grass.amount.y > 0.5;
    let lushness = clamp(grass.amount.z, 0.0, 1.0);
    if realistic || lushness > 0.001 {
        // The dense basal layer needs its own grounding without reenabling the depth prepass
        // that produced SSAO speckles. Keep the effect local to each blade and let the standard
        // diffuse-transmission lobe handle real directional backlighting.
        let hf = clamp(in.uv.x, 0.0, 1.0);
        let away_from_root = smoothstep(0.04, 0.34, hf);
        let realistic_root = mix(0.82, 1.0, away_from_root);
        let rich_realistic_root = mix(0.78, 1.0, away_from_root);
        // Painted grass keeps a broad, legible colour block at its roots. It gains grounding as
        // it gets lush, without borrowing the photographic leaf shading wholesale.
        let painted_root = mix(1.0, mix(0.91, 1.0, away_from_root), lushness);
        let root_shade = select(painted_root, mix(realistic_root, rich_realistic_root, lushness), realistic);
        pbr_input.material.base_color = vec4<f32>(
            pbr_input.material.base_color.rgb * root_shade,
            pbr_input.material.base_color.a,
        );
        let realistic_occlusion = mix(0.76, 1.0, away_from_root);
        let rich_realistic_occlusion = mix(0.70, 1.0, away_from_root);
        let painted_occlusion = mix(1.0, mix(0.87, 1.0, away_from_root), lushness);
        let occlusion = select(painted_occlusion, mix(realistic_occlusion, rich_realistic_occlusion, lushness), realistic);
        pbr_input.diffuse_occlusion = min(pbr_input.diffuse_occlusion, vec3<f32>(occlusion));
        let realistic_roughness = mix(0.88, 0.74, smoothstep(0.25, 1.0, hf));
        let rich_realistic_roughness = mix(0.91, 0.70, smoothstep(0.2, 0.94, hf));
        let painted_roughness = mix(0.96, 0.82, smoothstep(0.2, 1.0, hf));
        pbr_input.material.perceptual_roughness = select(
            mix(pbr_input.material.perceptual_roughness, painted_roughness, lushness),
            mix(realistic_roughness, rich_realistic_roughness, lushness),
            realistic,
        );
        let realistic_transmission = mix(0.18, 0.48, smoothstep(0.12, 0.88, hf));
        let rich_realistic_transmission = mix(0.16, 0.54, smoothstep(0.1, 0.86, hf));
        let painted_transmission = mix(0.18, 0.30, smoothstep(0.2, 0.9, hf));
        pbr_input.material.diffuse_transmission = select(
            mix(pbr_input.material.diffuse_transmission, painted_transmission, lushness),
            mix(realistic_transmission, rich_realistic_transmission, lushness),
            realistic,
        );
    }
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
