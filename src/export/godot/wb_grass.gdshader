// Skyfold Studio meadow grass: a port of the editor's `grass.wesl`. Each vertex arrives relative
// to its clump slot inside an 8 m cell (the MultiMesh instance is the cell origin). Placement on
// the terrain heights, density culling (sparse grass grows in clumps), distance thinning with
// constant coverage, the length-preserving wind bend with travelling gusts, shaped normals and the
// base → tip colour all happen here, as in the editor. Density map: R density, G dryness, B tint
// (0 cold … 0.5 neutral … 1 warm), A blade height (×2).
shader_type spatial;
render_mode cull_disabled, depth_draw_opaque, diffuse_burley, specular_schlick_ggx;

uniform sampler2D height_tex : filter_nearest, repeat_disable;
uniform sampler2D density_tex : filter_linear, repeat_disable;
// x: terrain half size, y: height-grid cell (m), z: height-grid vertices per side, w: sea level
uniform vec4 grid;
// x: density-grid cell (m), y: density-grid vertices per side
uniform vec2 dgrid;
// xy: wind direction, z: wind speed (m/s), w: time
uniform vec4 wind;
// xy: gust field offset, zw: camera xz
uniform vec4 gust;
// x: camera y, y: far fade start, z: far fade end, w: blade height scale
uniform vec4 view;
uniform vec3 base_c;
uniform vec3 tip_c;
uniform vec3 dry_c;
uniform vec3 cold_c;
uniform vec3 warm_c;
// x: grass amount, y: realistic, z: lushness, w: unused (the editor's orbit preview)
uniform vec4 amount;

varying float v_hf;
varying vec3 v_wp;

global uniform sampler2D wb_canopy : filter_linear, repeat_disable;
global uniform vec4 wb_canopy_grid;
global uniform vec4 wb_canopy_cam;

// how much of the sky a point still sees through the crowns (`canopy_shade.wesl::canopy_sky`)
float canopy_sky(vec3 p) {
	float n = wb_canopy_grid.w;
	if (n < 2.0) {
		return 1.0;
	}
	vec2 uv = (p.xz - wb_canopy_grid.xy) / (wb_canopy_grid.z * n);
	if (any(lessThan(uv, vec2(0.0))) || any(greaterThan(uv, vec2(1.0)))) {
		return 1.0;
	}
	vec3 s = texture(wb_canopy, uv).rgb;
	float h = clamp((p.y - s.b) / max(s.g - s.b, 1.0), 0.0, 1.0);
	float sky = mix(s.r, 1.0, h * h);
	float fade = smoothstep(wb_canopy_cam.z, wb_canopy_cam.w, length(p.xz - wb_canopy_cam.xy));
	return mix(sky, 1.0, fade);
}


uint hash_u32(uint x_in) {
	uint x = x_in;
	x ^= x >> 16u;
	x *= 0x7feb352du;
	x ^= x >> 15u;
	x *= 0x846ca68bu;
	x ^= x >> 16u;
	return x;
}

uint hash2i(int ix, int iy, uint seed) {
	return hash_u32((uint(ix) * 0x8da6b343u) ^ (uint(iy) * 0xd8163841u) ^ (seed * 0xcb1ab31fu));
}

float hashf(int ix, int iy, uint seed) {
	return float(hash2i(ix, iy, seed) & 0xffffu) / 65535.0;
}

float value_noise2(vec2 p, uint seed) {
	vec2 i = floor(p);
	vec2 f = p - i;
	vec2 u = f * f * (3.0 - 2.0 * f);
	int ix = int(i.x);
	int iy = int(i.y);
	float a = hashf(ix, iy, seed);
	float b = hashf(ix + 1, iy, seed);
	float c = hashf(ix, iy + 1, seed);
	float d = hashf(ix + 1, iy + 1, seed);
	float ab = a + (b - a) * u.x;
	float cd = c + (d - c) * u.x;
	return ab + (cd - ab) * u.y;
}

float fbm2(vec2 p_in, uint octaves, uint seed) {
	vec2 p = p_in;
	float sum = 0.0;
	float amp = 0.5;
	float norm = 0.0;
	for (uint o = 0u; o < octaves; o++) {
		sum += amp * value_noise2(p, seed + o * 101u);
		norm += amp;
		amp *= 0.5;
		p = vec2(p.x * 1.6 - p.y * 1.2, p.x * 1.2 + p.y * 1.6);
	}
	return sum / norm;
}

float gust_field(vec2 gp) {
	return value_noise2(gp / 140.0, 3u) * 0.62 + value_noise2(gp / 61.0 + vec2(0.37), 4u) * 0.38;
}

float terrain_height(vec2 xz) {
	float half_size = grid.x;
	float cell = grid.y;
	int n = int(grid.z);
	if (n < 2) {
		return -1000.0;
	}
	vec2 g = clamp((xz + vec2(half_size)) / cell, vec2(0.0), vec2(float(n - 1)));
	ivec2 i = ivec2(floor(g));
	vec2 f = g - floor(g);
	ivec2 i1 = min(i + ivec2(1), ivec2(n - 1));
	float a = texelFetch(height_tex, i, 0).r;
	float b = texelFetch(height_tex, ivec2(i1.x, i.y), 0).r;
	float c = texelFetch(height_tex, ivec2(i.x, i1.y), 0).r;
	float d = texelFetch(height_tex, i1, 0).r;
	return mix(mix(a, b, f.x), mix(c, d, f.x), f.y) * cell;
}

void vertex() {
	vec3 origin = (MODEL_MATRIX * vec4(0.0, 0.0, 0.0, 1.0)).xyz;
	float hf = UV.x;
	float across = UV.y;
	vec2 slot = UV2;
	vec4 attr = CUSTOM0;
	float rnd = attr.r;
	bool realistic = amount.y > 0.5;
	float lushness = clamp(amount.z, 0.0, 1.0);
	bool realistic_blade = attr.g >= 1.5;
	float tier = realistic_blade ? (attr.g - 1.5) * 2.0 : attr.g * 2.0;
	float slot_rand = attr.b;
	float az = attr.a * TAU;

	vec2 sxz = origin.xz + slot;
	float half_size = grid.x;
	vec2 gd = (sxz + vec2(half_size)) / dgrid.x;
	vec4 dd = textureLod(density_tex, (gd + vec2(0.5)) / dgrid.y, 0.0);
	float cover = dd.r * amount.x;
	float abundance = clamp(amount.x - 1.0, 0.0, 1.0);
	float legacy_density = min(cover, 1.0);
	float realistic_density = min(dd.r * min(amount.x, 1.0), 1.0);
	float density = realistic ? realistic_density : legacy_density;
	float dry = dd.g;
	float tint = dd.b;
	float style_h = dd.a * 2.0;
	float nz = fbm2(sxz / 6.5, 3u, 9u);
	float nz2 = fbm2(sxz / 2.3, 2u, 13u);
	float clump = nz * 0.55 + nz2 * 0.45;
	float tuft_noise = fbm2(sxz / 1.35, 3u, 41u);
	float tuft_core = 1.0 - smoothstep(0.32, 0.73, tuft_noise);
	float meadow_patch = 1.0 - smoothstep(0.30, 0.72, fbm2(sxz / 8.5, 3u, 53u));
	ivec2 cid = ivec2(floor(sxz * 16.0));
	float clump_cut = realistic ? 0.29 + density * 0.66 : 0.24 + density * 0.56;
	float rich_clump_cut = min(
		clump_cut
			+ abundance * (0.09 + 0.07 * meadow_patch)
			+ lushness * (0.08 + 0.17 * meadow_patch - 0.08 * (1.0 - meadow_patch)),
		0.99);
	bool present = density > 0.03 && hashf(cid.x, cid.y, 71u) * 0.35 + clump * 0.65 < rich_clump_cut;
	if (realistic_blade) {
		float fertile_start = mix(0.08, 0.035, lushness);
		float fertile_end = mix(0.55, 0.28, lushness);
		float fertile = smoothstep(fertile_start, fertile_end, dd.r);
		float amount_to_default = clamp(amount.x, 0.0, 1.0);
		float baseline_cover = fertile * amount_to_default * (0.74 + abundance * 0.26);
		float painted_cover = fertile * amount_to_default * lushness * (0.46 + 0.36 * tuft_core + abundance * 0.18);
		float rich_realistic_cover = max(baseline_cover, fertile * amount_to_default * (0.68 + 0.24 * tuft_core + 0.06 * meadow_patch + abundance * 0.12));
		float fine_cover = realistic ? mix(baseline_cover, rich_realistic_cover, lushness) : painted_cover;
		present = density > 0.03 && (realistic || lushness > 0.001) && fract(rnd * 17.17 + slot_rand * 3.1) < fine_cover;
	}

	float ground = terrain_height(sxz);
	vec3 eye = vec3(gust.z, view.x, gust.w);
	float dist = distance(vec3(sxz.x, ground, sxz.y), eye);
	float f2 = 1.0 - smoothstep(11.0, 16.0, dist);
	float f1 = 1.0 - smoothstep(31.0, 40.0, dist);
	float cut = mix(view.y, view.z, slot_rand);
	float f0 = 1.0 - smoothstep(cut - 5.0, cut, dist);
	float tf = f0;
	if (tier > 1.5) {
		tf = f2;
	} else if (tier > 0.5) {
		tf = f1;
	}
	float legacy_comp = 7.0 / max(2.0 * f0 + 3.0 * f1 + 2.0 * f2, 0.6);
	float realistic_comp = min(28.0 / max(5.0 * f0 + 7.0 * f1 + 16.0 * f2, 1.0), 1.6);
	float base_comp = realistic ? realistic_comp : legacy_comp;
	float rich_comp = min(base_comp, 1.12 + 0.28 * f0);
	float comp = mix(base_comp, rich_comp, lushness);

	float legacy_height = (0.3 + 0.44 * (1.0 - clump) + 0.1 * nz2) * (0.6 + 0.4 * density) * (1.0 + 0.35 * dry) * style_h * view.w;
	float tall_tuft = 0.55 + 0.30 * pow(fract(rnd * 5.13 + slot_rand * 1.7), 5.0);
	float basal_tuft = 0.75 + 0.35 * pow(fract(rnd * 9.71 + slot_rand * 2.3), 3.0);
	float leaf_height = realistic_blade ? basal_tuft : tall_tuft;
	float realistic_height = (0.20 + 0.58 * (1.0 - clump) + 0.17 * nz2) * (0.7 + 0.3 * density) * (1.0 + 0.28 * dry) * style_h * view.w * leaf_height;
	float clump_core = 1.0 - smoothstep(0.25, 0.75, clump);
	float painted_height = realistic_blade ? legacy_height * (0.58 + 0.14 * clump_core) : legacy_height;
	float base_height = realistic ? realistic_height : painted_height;
	float accent = smoothstep(0.86, 0.98, fract(rnd * 9.31 + slot_rand * 4.17));
	float stalk = realistic_blade ? 0.0 : smoothstep(0.86, 0.96, fract(rnd * 12.71 + slot_rand * 5.63));
	float painted_gain = 0.22 + 0.25 * clump_core + 0.34 * accent;
	float realistic_gain = 0.28 + 0.32 * clump_core + 0.48 * accent;
	float rich_stratum = realistic_blade ? 0.48 + 0.30 * tuft_core + 0.12 * meadow_patch : 1.14 + 0.36 * tuft_core + 0.16 * accent;
	float rich_height = base_height
		* (1.0 + lushness * (realistic ? realistic_gain : painted_gain))
		* mix(1.0, rich_stratum, lushness)
		* (1.0 + lushness * stalk * 0.42);
	float height = min(rich_height, max(base_height, 2.0));
	float yaw = fract(slot_rand * 7.13) * TAU;
	float cy = cos(yaw);
	float sy = sin(yaw);
	vec3 p0 = VERTEX;
	vec3 o = vec3(p0.x * cy - p0.z * sy, p0.y, p0.x * sy + p0.z * cy) * height;
	float stalk_shape = lushness * stalk;
	float stalk_straighten = stalk_shape * smoothstep(0.08, 0.62, hf);
	float stalk_spread = 1.0 - 0.62 * stalk_straighten;
	o = vec3(o.x * stalk_spread, o.y, o.z * stalk_spread);
	float seed_lift = smoothstep(0.26, 0.62, hf) * (1.0 - smoothstep(0.62, 1.0, hf));
	o.y += height * stalk_shape * seed_lift * 0.12;
	if (realistic_blade) {
		o = vec3(o.x * 0.72, o.y * 1.04, o.z * 0.72);
	}
	float baz = az + yaw;
	vec3 side = vec3(-sin(baz), 0.0, cos(baz));
	float legacy_bw = 0.021 * (0.8 + 0.5 * rnd) * (1.0 - pow(hf, 1.6)) * (0.75 + 0.25 * (1.0 - hf)) * comp * tf * (1.0 - 0.3 * dry);
	float fine_scale = realistic_blade ? 0.84 : 1.0;
	float realistic_bw = 0.0105 * (0.78 + 0.42 * rnd) * (1.0 - pow(hf, 1.25)) * fine_scale * comp * tf * (1.0 - 0.22 * dry);
	float painted_bw = realistic_blade ? legacy_bw * 0.56 : legacy_bw;
	float base_bw = realistic ? realistic_bw : painted_bw;
	float rich_width = 1.0 + 0.28 * (1.0 - hf) * (1.0 - 0.45 * accent);
	float rich_leaf_width = realistic_blade ? 0.72 + 0.38 * tuft_core : 1.20 + 0.56 * tuft_core + 0.16 * (1.0 - hf);
	float bw = base_bw * mix(1.0, rich_width * rich_leaf_width, lushness);
	float stalk_stem = stalk * smoothstep(0.06, 0.38, hf) * (1.0 - smoothstep(0.46, 0.62, hf));
	bw *= 1.0 - lushness * stalk_stem * 0.48;
	float seed_brush = stalk * smoothstep(0.46, 0.61, hf) * (1.0 - smoothstep(0.63, 0.86, hf));
	bw *= 1.0 + lushness * seed_brush * 1.8;
	float seed_width = realistic ? 0.008 : 0.014;
	bw = mix(bw, min(bw, seed_width), lushness * seed_brush);

	// wind: length-preserving bend towards the wind, travelling gusts, flutter
	vec3 wdir = vec3(wind.x, 0.0, wind.y);
	float t = wind.w;
	vec2 gp = sxz - gust.xy;
	float gst = smoothstep(0.44, 0.62, gust_field(gp));
	float w = clamp(wind.z * 0.1, 0.03, 1.3);
	float ph = slot_rand * 6.28;
	float bend = w * (gst * 0.55 + 0.22) + sin(t * 1.9 + ph + sxz.x * 0.2) * w * 0.1;
	float flut = sin(t * 7.3 + ph * 3.0 + rnd * 20.0) * (w * 0.06 + 0.015)
		+ sin(t * 11.7 + ph * 5.7 + sxz.y * 0.37) * w * lushness * (0.025 + 0.045 * tuft_core);
	vec3 wperp = vec3(-wdir.z, 0.0, wdir.x);
	float len = length(o);
	vec3 disp = (wdir * bend + wperp * flut) * hf * hf * len;
	o = normalize(o + disp + vec3(0.0, 1.0e-5, 0.0)) * len;

	vec3 wp = vec3(sxz.x, ground - 0.03, sxz.y) + o + side * (across * bw);
	if (!present || f0 <= 0.001 || tf <= 0.001) {
		wp = vec3(sxz.x, ground - 2.0, sxz.y);
	}
	VERTEX = wp - origin;
	v_wp = wp;
	v_hf = hf;

	vec3 lean = vec3(o.x, 0.0, o.z) / max(length(o.xz), 1.0e-4);
	vec3 legacy_normal = normalize(vec3(0.0, 1.0, 0.0) + lean * 0.25 + side * across * 0.35);
	vec3 realistic_normal = normalize(vec3(0.0, 0.49, 0.0) + lean * 0.84 + side * 0.12);
	NORMAL = realistic ? realistic_normal : legacy_normal;

	vec3 lush = mix(mix(cold_c, tip_c, smoothstep(0.0, 0.5, tint)), warm_c, smoothstep(0.5, 1.0, tint));
	vec3 tipc = mix(lush, dry_c, clamp(dry * 1.1 + (rnd - 0.5) * 0.18, 0.0, 1.0));
	vec3 col = mix(base_c, tipc, smoothstep(0.0, 0.85, hf));
	col *= mix(0.55, 1.0, smoothstep(0.0, 0.7, hf));
	float dead = step(0.94, fract(rnd * 13.7));
	col = mix(col, dry_c * 0.75, dead * 0.7);
	vec3 rich_highlight = realistic ? vec3(1.03, 1.05, 0.87) : vec3(1.05, 1.07, 0.84);
	col = mix(col, col * rich_highlight, lushness * smoothstep(0.22, 0.9, hf) * (0.3 + 0.7 * clump_core));
	vec3 deep = realistic ? vec3(0.035, 0.115, 0.018) : vec3(0.055, 0.17, 0.035);
	vec3 leaf = realistic ? vec3(0.15, 0.34, 0.045) : vec3(0.16, 0.40, 0.065);
	float leaf_lit = smoothstep(0.28, 0.92, hf) * (0.38 + 0.62 * tuft_core);
	vec3 rich_col = mix(deep, mix(leaf, col, 0.32 + 0.24 * accent), leaf_lit);
	col = mix(col, rich_col, lushness * (0.52 + 0.38 * meadow_patch));
	float tuft_warmth = smoothstep(0.34, 0.70, fbm2(sxz / 2.6, 2u, 73u));
	vec3 tuft_tone = mix(vec3(0.76, 0.97, 1.02), vec3(1.10, 1.04, 0.78), tuft_warmth);
	col = mix(col, col * tuft_tone, lushness * 0.30 * (0.35 + 0.65 * tuft_core));
	vec3 seed_pale = realistic ? vec3(0.62, 0.67, 0.42) : vec3(0.68, 0.66, 0.34);
	vec3 seed_col = mix(tipc, seed_pale, 0.60 + 0.18 * dry);
	col = mix(col, seed_col, lushness * seed_brush * 0.82);
	float gust_sheen = realistic ? 0.18 : 0.45;
	col = mix(col, col * vec3(1.3, 1.28, 1.1) + vec3(0.02), gst * smoothstep(0.15, 0.9, hf) * gust_sheen);
	COLOR = vec4(col, 1.0);
}

void fragment() {
	bool realistic = amount.y > 0.5;
	float lushness = clamp(amount.z, 0.0, 1.0);
	vec3 albedo = COLOR.rgb;
	float rough = 0.62;
	float transmission = 0.3;
	float occlusion = 1.0;
	if (realistic || lushness > 0.001) {
		float hf = clamp(v_hf, 0.0, 1.0);
		float away_from_root = smoothstep(0.04, 0.34, hf);
		float realistic_root = mix(0.82, 1.0, away_from_root);
		float rich_realistic_root = mix(0.78, 1.0, away_from_root);
		float painted_root = mix(1.0, mix(0.91, 1.0, away_from_root), lushness);
		float root_shade = realistic ? mix(realistic_root, rich_realistic_root, lushness) : painted_root;
		albedo *= root_shade;
		float realistic_occlusion = mix(0.76, 1.0, away_from_root);
		float rich_realistic_occlusion = mix(0.70, 1.0, away_from_root);
		float painted_occlusion = mix(1.0, mix(0.87, 1.0, away_from_root), lushness);
		occlusion = realistic ? mix(realistic_occlusion, rich_realistic_occlusion, lushness) : painted_occlusion;
		float realistic_roughness = mix(0.88, 0.74, smoothstep(0.25, 1.0, hf));
		float rich_realistic_roughness = mix(0.91, 0.70, smoothstep(0.2, 0.94, hf));
		float painted_roughness = mix(0.96, 0.82, smoothstep(0.2, 1.0, hf));
		rough = realistic ? mix(realistic_roughness, rich_realistic_roughness, lushness) : mix(rough, painted_roughness, lushness);
		float realistic_transmission = mix(0.18, 0.48, smoothstep(0.12, 0.88, hf));
		float rich_realistic_transmission = mix(0.16, 0.54, smoothstep(0.1, 0.86, hf));
		float painted_transmission = mix(0.18, 0.30, smoothstep(0.2, 0.9, hf));
		transmission = realistic ? mix(realistic_transmission, rich_realistic_transmission, lushness) : mix(transmission, painted_transmission, lushness);
	}
	ALBEDO = albedo;
	ROUGHNESS = rough;
	SPECULAR = 0.28;
	AO = occlusion * canopy_sky(v_wp);
	AO_LIGHT_AFFECT = 0.0;
	BACKLIGHT = albedo * transmission;
}
