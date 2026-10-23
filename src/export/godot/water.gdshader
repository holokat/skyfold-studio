// Skyfold Studio water: one shader for the ocean (Gerstner swell), lakes, rivers and waterfalls.
// The water column between surface and bed (from the depth buffer) decides how much of the bed
// shows through and how deep the colour gets; ripple normals drift with the flow; foam gathers
// where the water is shallow. Reflections come from the sky.
shader_type spatial;
render_mode blend_mix, depth_draw_always, cull_disabled, diffuse_burley, specular_schlick_ggx;

// how a sandy bed `clarity` metres down looks, and infinitely deep water (sRGB)
uniform vec3 shallow_color : source_color = vec3(0.1, 0.55, 0.6);
uniform vec3 deep_color : source_color = vec3(0.02, 0.12, 0.2);
// visibility under water (m) and how see-through the water is at all (0 opaque … 1 clear)
uniform float clarity = 6.0;
uniform float transparency = 1.0;
// ocean sea state (0 glassy … 2.5 storm); ripple strength on lakes and rivers
uniform float wave_height = 1.0;
uniform float wave_speed = 1.0;
uniform float roughness_amount = 0.04;
uniform float foam = 0.4;
uniform float shore_foam = 0.5;
// drift (m/s) along flow_dir, or downstream along the mesh's V for rivers
uniform float flow = 0.0;
uniform vec2 flow_dir = vec2(1.0, 0.0);
uniform bool along_uv = false;
uniform bool ocean = false;
uniform vec3 emissive_color : source_color = vec3(0.0);
uniform float emissive_strength = 0.0;
uniform float metallic_amount = 0.0;
// the editor's optics for this style (`water::water_optics`): extinction per metre and the
// in-scatter albedo (the colour of infinitely deep water), linear
uniform vec3 extinct = vec3(0.3, 0.08, 0.06);
uniform vec3 inscat = vec3(0.01, 0.05, 0.1);
// the key light (sun or moon) and the sky's irradiance in the editor's display units, and the
// exposure that turns display units into Godot's (the Atmosphere keeps them current)
global uniform vec3 wb_sun_dir;
global uniform vec3 wb_sun_disp;
global uniform vec3 wb_sky_disp;
global uniform float wb_exposure;
// the terrain heights (metres; `wb_hgrid`: half size, cell, vertices per side) for the water
// column under each point, set by the Water node
global uniform sampler2D wb_heights : filter_nearest;
global uniform vec4 wb_hgrid;
global uniform sampler2D wb_cover : filter_linear_mipmap, repeat_enable;
global uniform vec4 wb_cloud;

float cloud_at(vec2 xz) {
	float c = clamp(wb_cloud.x, 0.0, 1.0);
	float n = texture(wb_cover, (xz + wb_cloud.zw) / 32768.0).r;
	float thr = 1.0 - c;
	return max(smoothstep(thr, thr + 0.4, n), clamp((c - 0.75) * 4.0, 0.0, 1.0));
}
// the editor's tiling RGBA value noise and bubbly foam mat (`ocean.rs`), and the sea's significant
// wave height (m) for the surf
// lava (the Molten behaviour): crust plates and glowing cracks instead of water
uniform bool molten = false;
uniform sampler2D noise_tex : filter_linear_mipmap, repeat_enable;
uniform sampler2D foam_tex : filter_linear_mipmap, repeat_enable;
uniform float sea_hs = 0.5;
uniform sampler2D ripple_normal : hint_normal, filter_linear_mipmap_anisotropic, repeat_enable;
uniform sampler2D screen_tex : hint_screen_texture, filter_linear_mipmap, repeat_disable;
uniform sampler2D depth_tex : hint_depth_texture, filter_linear, repeat_disable;

varying vec3 wpos;

uint hash_u32(uint x_in) {
	uint x = x_in;
	x ^= x >> 16u;
	x *= 0x7feb352du;
	x ^= x >> 15u;
	x *= 0x846ca68bu;
	x ^= x >> 16u;
	return x;
}

float hashf(int ix, int iy, uint seed) {
	return float(hash_u32((uint(ix) * 0x8da6b343u) ^ (uint(iy) * 0xd8163841u) ^ (seed * 0xcb1ab31fu)) & 0xffffu) / 65535.0;
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

float terrain_h(vec2 xz) {
	int n = int(wb_hgrid.z);
	if (n < 2) {
		return -1000.0;
	}
	vec2 g = clamp((xz + vec2(wb_hgrid.x)) / wb_hgrid.y, vec2(0.0), vec2(float(n - 1)));
	ivec2 i = ivec2(floor(g));
	vec2 f = g - floor(g);
	ivec2 i1 = min(i + ivec2(1), ivec2(n - 1));
	float a = texelFetch(wb_heights, i, 0).r;
	float b = texelFetch(wb_heights, ivec2(i1.x, i.y), 0).r;
	float c = texelFetch(wb_heights, ivec2(i.x, i1.y), 0).r;
	float d = texelFetch(wb_heights, i1, 0).r;
	return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

vec3 voronoi2(vec2 p, uint seed) {
	vec2 i = floor(p);
	vec2 f = p - i;
	float d1 = 8.0;
	float d2 = 8.0;
	float id = 0.0;
	for (int y = -1; y <= 1; y++) {
		for (int x = -1; x <= 1; x++) {
			int cx = int(i.x) + x;
			int cy = int(i.y) + y;
			vec2 o = vec2(hashf(cx, cy, seed), hashf(cx, cy, seed + 7u)) * 0.85 + 0.075;
			vec2 r = vec2(float(x), float(y)) + o - f;
			float d = dot(r, r);
			if (d < d1) {
				d2 = d1;
				d1 = d;
				id = hashf(cx, cy, seed + 13u);
			} else if (d < d2) {
				d2 = d;
			}
		}
	}
	return vec3(sqrt(d1), sqrt(d2), id);
}

vec2 rot2(vec2 v, float a) {
	float c = cos(a);
	float s = sin(a);
	return vec2(v.x * c - v.y * s, v.x * s + v.y * c);
}

// the surf (`shore.wesl`): wave number and phase at xz, and the size of wave n
float surf_size(float n, vec2 xz) {
	return 0.5 + 0.5 * value_noise2(vec2(n * 0.61, n * 0.37) + xz * 0.0035, 229u);
}

vec2 surf_cycle(vec2 xz, float t, float hs) {
	float period = 6.5 + 1.6 * clamp(hs, 0.0, 2.5);
	float lag = value_noise2(xz * 0.0105, 211u) * 2.4 + value_noise2(xz * 0.043, 223u) * 0.5;
	float cyc = t / period + lag;
	return vec2(cyc, surf_size(floor(cyc), xz));
}

// one Gerstner wave's height and slope at p, faded out where it gets smaller than the pixel
// footprint (the swell is evaluated per pixel: the ocean plane's vertices are tens of metres
// apart, far too coarse for these wavelengths, and per-vertex waves alias into a lattice)
void gerstner(vec2 dir, float wavelength, float steep, vec2 p, float t, float footprint, inout float h, inout vec2 slope, inout float amp) {
	float k = 6.28318 / wavelength;
	float c = sqrt(9.8 / k);
	vec2 d = normalize(dir);
	float f = k * (dot(d, p) - c * t);
	float fade = 1.0 - smoothstep(0.15, 0.5, footprint / wavelength);
	h += steep / k * sin(f) * fade;
	slope += d * steep * cos(f) * fade;
	amp += steep / k * fade;
}

void vertex() {
	wpos = (MODEL_MATRIX * vec4(VERTEX, 1.0)).xyz;
}

void fragment() {
	float t = TIME * wave_speed;
	float crest = 0.0;
	if (ocean && wave_height > 0.01) {
		float s = clamp(wave_height, 0.0, 2.5) * 0.11;
		float footprint = max(length(dFdx(wpos.xz)), length(dFdy(wpos.xz)));
		float h = 0.0;
		vec2 slope = vec2(0.0);
		float amp = 0.0;
		gerstner(vec2(1.0, 0.35), 72.0, s, wpos.xz, t, footprint, h, slope, amp);
		gerstner(vec2(0.6, -0.8), 41.0, s * 0.8, wpos.xz, t, footprint, h, slope, amp);
		gerstner(vec2(-0.3, 1.0), 23.0, s * 0.6, wpos.xz, t, footprint, h, slope, amp);
		gerstner(vec2(0.9, 0.5), 11.0, s * 0.4, wpos.xz, t, footprint, h, slope, amp);
		// the highest crests foam in rough seas only (in a calm sea the few spots where all four
		// waves peak together would make a regular grid of white dots)
		crest = smoothstep(0.75, 1.0, h / max(amp, 0.01)) * step(0.05, amp) * smoothstep(1.0, 2.0, wave_height);
		// (the editor fades its wave cascades with distance; grazing far water stays calm)
		slope *= 1.0 - 0.9 * smoothstep(150.0, 900.0, length(CAMERA_POSITION_WORLD.xz - wpos.xz));
		NORMAL = normalize((VIEW_MATRIX * vec4(normalize(vec3(-slope.x, 1.0, -slope.y)), 0.0)).xyz);
	}
	vec2 dir = normalize(flow_dir + vec2(1e-4));
	vec2 uv;
	vec2 drift;
	if (along_uv) {
		uv = UV * vec2(1.0, 1.0) * 0.6;
		drift = vec2(0.0, -1.0) * flow * 0.35;
	} else {
		uv = wpos.xz / 22.0;
		drift = dir * (flow * 0.08 + 0.01);
	}
	vec3 n1 = texture(ripple_normal, uv + drift * t * 0.6).rgb;
	vec3 n2 = texture(ripple_normal, uv * 1.9 + vec2(0.37, 0.71) - drift.yx * t * 0.45).rgb;
	vec3 nm = normalize(n1 * 2.0 - 1.0 + n2 * 2.0 - 1.0);
	NORMAL_MAP = nm * 0.5 + 0.5;
	// far away the ripples fade into roughness (the editor's distance fades and LEAN roughness),
	// so the grazing sea doesn't alias into dark bands
	float vdist = length(CAMERA_POSITION_WORLD.xz - wpos.xz);
	NORMAL_MAP_DEPTH = (0.35 + clamp(wave_height, 0.0, 2.5) * 0.25) * (1.0 - 0.8 * smoothstep(120.0, 700.0, vdist));

	if (molten) {
		// ---- lava (`water.wesl::lava`): two sizes of crust plates, cracks opening and closing,
		// molten rock from deep red to yellow where hottest, glowing (display units)
		float tl = TIME;
		float fpl = max(length(fwidth(wpos.xz)), 1e-4);
		float dl = wb_hgrid.z > 1.5 ? max(wpos.y - terrain_h(wpos.xz), 0.0) : 2.0;
		float heat = clamp(dl / 4.0, 0.0, 1.0) * 0.3 + 0.2;
		vec2 q = wpos.xz + (vec2(value_noise2(wpos.xz * 0.02 + tl * 0.004, 331u), value_noise2(wpos.xz * 0.02 - tl * 0.004, 337u)) - 0.5) * 12.0;
		vec2 warp = vec2(value_noise2(q * 0.11 + vec2(tl * 0.013, 0.0), 301u), value_noise2(q * 0.11 + vec2(5.2, -tl * 0.011), 307u)) - 0.5;
		vec2 p = q + warp * 3.5;
		vec3 v1 = voronoi2(p * 0.22, 311u);
		vec3 v2 = voronoi2(p * 0.7 + vec2(3.7, 1.9), 313u);
		float open = 0.5 + 0.5 * sin(tl * 0.35 + v1.z * 6.2832);
		float w1 = mix(0.012, 0.055, heat) * (0.5 + 0.8 * open);
		float w2 = mix(0.006, 0.03, heat);
		float aa = fpl * 0.6;
		float crack1 = 1.0 - smoothstep(w1, w1 + 0.035 + aa * 0.22, v1.y - v1.x);
		float crack2 = (1.0 - smoothstep(w2, w2 + 0.03 + aa * 0.7, v2.y - v2.x)) * 0.55;
		float crack = max(crack1, crack2);
		float hot = clamp(heat * 0.55 + open * 0.2 + (value_noise2(q * 0.05 + vec2(0.0, tl * 0.02), 317u) - 0.5) * 0.5, 0.0, 1.0);
		vec3 mol = mix(vec3(0.7, 0.07, 0.01), mix(vec3(1.3, 0.38, 0.05), vec3(2.0, 1.1, 0.3), smoothstep(0.75, 1.0, hot)), smoothstep(0.2, 0.75, hot));
		float shade = 0.035 + 0.05 * v1.z + 0.03 * value_noise2(q * 1.7, 319u);
		float thin = smoothstep(0.5, 1.0, heat) * (1.0 - smoothstep(0.0, 0.25, v1.y - v1.x));
		float farl = smoothstep(0.2, 2.5, fpl);
		float lit = mix(crack, mix(0.04, 0.2, heat), farl);
		vec3 glow = mol * (lit * 1.3 + thin * 0.12) + vec3(0.25, 0.04, 0.0) * heat * 0.2;
		float rope = sin(p.y * 2.3 + v2.x * 3.0) * 0.5 * (1.0 - farl);
		vec2 sl = vec2(warp.x * 0.25, warp.y * 0.25 + rope * 0.08) * (1.0 - crack);
		NORMAL = normalize((VIEW_MATRIX * vec4(normalize(vec3(-sl.x, 1.0, -sl.y)), 0.0)).xyz);
		NORMAL_MAP_DEPTH = 0.0;
		ALBEDO = vec3(shade, shade * 0.93, shade * 0.88);
		ROUGHNESS = mix(0.75, 0.55, heat);
		METALLIC = 0.0;
		SPECULAR = 0.5;
		vec3 emis = emissive_color * emissive_strength;
		EMISSION = glow * (0.55 + emis.r * 0.3) / max(wb_exposure, 1e-3);
		ALPHA = smoothstep(0.0, 0.08, dl);
		return;
	}
	// ---- the editor's water optics (`water.wesl`): the water column along the refracted view
	// ray, Beer-law extinction, sun and sky light scattered in it towards the deep colour, the
	// lit bed seen through it, Fresnel. Light terms are in the editor's display units (global
	// uniforms the Atmosphere keeps current), Godot's exposure taken out for its emission.
	vec3 V = normalize(CAMERA_POSITION_WORLD - wpos);
	vec3 Nb = normalize((INV_VIEW_MATRIX * vec4(NORMAL, 0.0)).xyz);
	vec3 Nw = normalize(Nb + vec3(nm.x, 0.0, -nm.y) * NORMAL_MAP_DEPTH * 0.35);
	vec3 Tr = refract(-V, Nw, 1.0 / 1.333);
	vec3 Tv = normalize(vec3(Tr.x, min(Tr.y, -0.08), Tr.z));
	float tdown = max(-Tv.y, 0.04);
	// the bed straight behind this pixel (reversed Z: 0 is the sky)
	float d0 = texture(depth_tex, SCREEN_UV).r;
	vec4 b0 = INV_VIEW_MATRIX * INV_PROJECTION_MATRIX * vec4(SCREEN_UV * 2.0 - 1.0, d0, 1.0);
	b0.xyz /= b0.w;
	bool sky_behind = d0 <= 1e-7;
	// the water column straight under this point and the refracted path through the terrain
	// heights (two refinements, as the editor)
	float depth_v = wb_hgrid.z > 1.5 ? max(wpos.y - terrain_h(wpos.xz), 0.0) : (sky_behind ? 400.0 : max(wpos.y - b0.y, 0.0));
	float column = depth_v;
	float L0 = depth_v / tdown;
	float path = L0;
	if (wb_hgrid.z > 1.5 && L0 < 100.0) {
		float L1 = max(wpos.y - terrain_h(wpos.xz + Tv.xz * min(L0, 200.0)), 0.0) / tdown;
		path = max(wpos.y - terrain_h(wpos.xz + Tv.xz * min(0.5 * (L0 + L1), 200.0)), 0.0) / tdown;
	}
	path = min(path, 400.0);
	// the refracted lookup where the ray ends, if what is there lies behind the surface
	vec3 p_end = wpos + Tv * min(path, 60.0);
	vec4 c_end = PROJECTION_MATRIX * VIEW_MATRIX * vec4(p_end, 1.0);
	vec2 ruv = c_end.xy / c_end.w * 0.5 + 0.5;
	vec2 suv = SCREEN_UV;
	vec3 q = b0.xyz;
	if (all(greaterThan(ruv, vec2(0.002))) && all(lessThan(ruv, vec2(0.998)))) {
		float dr = texture(depth_tex, ruv).r;
		vec4 br = INV_VIEW_MATRIX * INV_PROJECTION_MATRIX * vec4(ruv * 2.0 - 1.0, dr, 1.0);
		br.xyz /= br.w;
		if (dr <= 1e-7 || distance(br.xyz, CAMERA_POSITION_WORLD) > distance(wpos, CAMERA_POSITION_WORLD) + 0.05) {
			suv = ruv;
			sky_behind = dr <= 1e-7;
			q = br.xyz;
		}
	}
	// (what the lookup found bounds the path and the column, as in the editor)
	if (!sky_behind) {
		path = clamp(min(path, distance(q, wpos)), 0.0, 400.0);
		column = max(wpos.y - q.y, 0.0);
	} else {
		path = 400.0;
	}
	vec3 scene = texture(screen_tex, suv).rgb;
	vec3 L0d = normalize(wb_sun_dir);
	vec3 trans = sky_behind ? vec3(0.0) : exp(-extinct * path);
	vec3 L = normalize(wb_sun_dir);
	// moving cloud shadows dim the light in the water (the sky dome less than the sun)
	vec2 cloud_p = wpos.xz + L0d.xz * ((1800.0 - wpos.y) / max(L0d.y, 0.05));
	float cloud_sh = 1.0 - cloud_at(cloud_p) * wb_cloud.y;
	vec3 sun = wb_sun_disp * cloud_sh;
	vec3 sky_irr = wb_sky_disp * (1.0 - cloud_at(wpos.xz) * 0.28);
	vec3 Ls = normalize(vec3(L.x / 1.333, max(sqrt(max(1.0 - (1.0 - L.y * L.y) / (1.333 * 1.333), 0.0)), 0.1), L.z / 1.333));
	float mu_s = max(Ls.y, 0.1);
	float mu_v = max(-Tv.y, 0.15);
	float k_sun = 1.0 + mu_v / mu_s;
	float k_amb = 1.0 + mu_v / 0.75;
	float phase = 1.0 + 0.6 * smoothstep(-0.6, 0.2, dot(Tv, Ls));
	vec3 sun_in = sun * (0.98 - 0.98 * pow(1.0 - max(L.y, 0.0), 5.0)) * smoothstep(0.0, 0.12, L.y) * max(L.y, 0.0) / mu_s;
	vec3 in_sun = sun_in * inscat * (2.7 * phase) * (1.0 - exp(-extinct * k_sun * path)) / k_sun;
	vec3 in_amb = sky_irr * inscat * 3.7 * (1.0 - exp(-extinct * k_amb * path)) / k_amb;
	float sun_w = dot(sun_in, vec3(0.3, 0.5, 0.2)) / max(dot(sun_in + sky_irr * 1.5, vec3(0.3, 0.5, 0.2)), 1e-4);
	vec3 lit = mix(exp(-extinct * column * 1.2) * 0.9, exp(-extinct * column / mu_s), sun_w);
	vec3 opaque_body = sun_in * inscat * (2.7 * phase) / k_sun + sky_irr * inscat * 3.7 / k_amb;
	float T = clamp(transparency, 0.0, 1.0);
	float ex = max(wb_exposure, 1e-3);
	// ---- white water (`water.wesl`): the surf on beaches, lapping lake shores, the current on
	// rivers, thresholding the foam mat by the local coverage so it tears into lace
	float fp = max(length(fwidth(wpos.xz)), 1e-4);
	vec2 np = along_uv ? UV * 8.0 + vec2(0.0, -t * max(flow, 0.15)) : wpos.xz + dir * t * 0.04;
	vec2 fwarp = texture(noise_tex, np / 3.7).gb - 0.5;
	float foam_noise = texture(noise_tex, rot2(np, 0.7) / 9.0 + fwarp * 0.12).r;
	float lace_n = texture(noise_tex, rot2(np, 2.1) / 2.3 + fwarp * 0.3 + vec2(0.41, 0.17)).g;
	float dpt = depth_v;
	float white = 0.0;
	if (ocean) {
		float hs = sea_hs;
		vec2 cyc = surf_cycle(wpos.xz, t, hs);
		float c = fract(cyc.x);
		float next = surf_size(floor(cyc.x) + 1.0, wpos.xz);
		float brk = clamp(hs * 1.1 * next + 0.3, 0.3, 3.0);
		float s_left = 1.0 - c;
		float s_here = 0.9 * dpt / brk;
		float bore = (1.0 - smoothstep(0.0, 0.06, abs(s_left - s_here))) * smoothstep(brk * 1.15, brk * 0.75, dpt);
		float behind = (s_left < s_here ? exp(-(s_here - s_left) * 5.0) : 0.0) * smoothstep(brk * 1.1, brk * 0.5, dpt);
		float after = exp(-c * 3.5) * smoothstep(brk * 0.9, 0.0, dpt) * cyc.y;
		float lace0 = smoothstep(0.35, 0.75, foam_noise * 0.55 + lace_n * 0.45);
		float bead = (1.0 - smoothstep(0.02, 0.14, dpt)) * smoothstep(0.25, 0.4, c) * smoothstep(0.55, 0.7, lace_n) * 0.6;
		white = (bore * (0.55 + 0.45 * lace0) + max(behind, after) * lace0 * 0.85 + bead) * shore_foam * smoothstep(0.0, 0.08, hs);
	} else if (along_uv) {
		float spd = clamp(flow / 2.5, 0.0, 1.5);
		float flecks = smoothstep(0.72, 0.86, lace_n * 0.6 + foam_noise * 0.4) * (0.25 + 0.5 * spd);
		float shallow = (1.0 - smoothstep(0.15, 0.9, dpt)) * smoothstep(0.4, 0.7, foam_noise) * spd;
		white = max(flecks * foam, shallow) * smoothstep(0.0, 0.05, foam + shore_foam);
	} else {
		white = (1.0 - smoothstep(0.02, 0.18, dpt)) * smoothstep(0.6, 0.8, lace_n) * 0.45 * shore_foam;
	}
	float cov = clamp(max(crest * foam, clamp(white, 0.0, 1.0)), 0.0, 1.0);
	vec2 sp = along_uv ? np : wpos.xz - dir * flow * t;
	vec2 fuv = along_uv ? vec2(sp.x * 0.09, sp.y * 0.03) : sp * 0.09;
	vec4 p1 = texture(foam_tex, fuv);
	vec4 p2 = texture(foam_tex, rot2(fuv, 0.6435) * 2.37 + vec2(0.31, 0.77));
	float pattern = p1.x * 0.62 + p2.x * 0.38;
	float thresh = 1.05 - cov * 1.1;
	float soft = 0.06 + fp * 0.1 + (along_uv ? 0.07 : 0.0);
	float lace = smoothstep(thresh - soft, thresh + soft, pattern) * (p1.y * 0.25 + 0.8);
	float cover = clamp(mix(lace, cov * 0.85, smoothstep(0.15, 1.2, fp)), 0.0, 0.97);
	// Fresnel (F0 = 0.02, Schlick) decides what enters the water
	float ndv = clamp(dot(Nw, V), 1e-3, 1.0);
	float fres = 0.02 + 0.98 * pow(1.0 - ndv, 5.0);
	float into = (1.0 - fres) * (1.0 - cover) * (1.0 - metallic_amount);
	// (the scene behind is in Godot's units already; the in-water light is in display units)
	vec3 column_col = (scene * lit * trans) * T + (in_sun + in_amb) * T / ex + opaque_body * (1.0 - T) / ex;
	vec3 murk_foam = mix(vec3(0.8), min(shallow_color * 2.2, vec3(0.8)), 0.6 * (1.0 - smoothstep(0.6, 3.0, clarity)));
	ALBEDO = mix(murk_foam * cover, shallow_color, metallic_amount);
	EMISSION = column_col * into + emissive_color * emissive_strength;
	ROUGHNESS = mix(clamp(roughness_amount + 0.12 * smoothstep(120.0, 900.0, vdist), 0.04, 1.0), 0.92, cover);
	METALLIC = metallic_amount * (1.0 - cover);
	SPECULAR = 0.354;
	// the waterline: the film thins to nothing on the sand
	ALPHA = smoothstep(0.0, 0.05, depth_v);
}
