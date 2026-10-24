// Skyfold Studio terrain: up to 12 photoscanned ground layers blended by the exported splat
// weights. Layers blend by height (pebbles and blades stand out of the layer below), rock layers
// are projected on three axes so cliffs don't stretch, and from ~40 m a larger rotated copy of
// each texture takes over so the distance keeps structure instead of a visible repeat.
shader_type spatial;
render_mode blend_mix, depth_draw_opaque, cull_back, diffuse_burley, specular_schlick_ggx;

uniform sampler2D splat_0 : hint_default_black, filter_linear, repeat_disable;
uniform sampler2D splat_1 : hint_default_black, filter_linear, repeat_disable;
uniform sampler2D splat_2 : hint_default_black, filter_linear, repeat_disable;
// one slice per used layer: albedo (sRGB), normal (OpenGL), mask (R metal, G AO, B height, A smoothness)
uniform sampler2DArray albedo_array : source_color, filter_linear_mipmap_anisotropic, repeat_enable;
uniform sampler2DArray normal_array : filter_linear_mipmap_anisotropic, repeat_enable;
uniform sampler2DArray mask_array : filter_linear_mipmap_anisotropic, repeat_enable;
uniform int layer_count = 1;
// metres per texture repeat, and 1 for layers projected on three axes (rock)
uniform float layer_tile[12];
uniform int layer_triplanar[12];
uniform vec2 terrain_origin = vec2(-2048.0, -2048.0);
uniform float terrain_size = 4096.0;
uniform float sea_level = 0.0;
uniform float normal_strength = 1.0;
// per used layer: how much standing rain water it holds, and whether it is snow
uniform float layer_holds[12];
uniform float layer_snow[12];

varying vec3 wpos;
varying vec3 wnrm;
varying float v_cloud_sh;

// the editor's shore map (level of the nearest water, distance to its edge, kind, 1), (half size,
// texels per side, sea wave height, shore foam), rain (wetness, puddle fill, rain falling) and
// the sea's colour (linear) for the sheet of water on the swash
global uniform sampler2D wb_shore : filter_nearest, repeat_disable;
global uniform vec4 wb_shore_p;
global uniform vec4 wb_rain;
global uniform vec3 wb_water_tint;
global uniform float wb_exposure;

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

float surf_size(float n, vec2 xz) {
	return 0.5 + 0.5 * value_noise2(vec2(n * 0.61, n * 0.37) + xz * 0.0035, 229u);
}

vec2 surf_cycle(vec2 xz, float t, float hs) {
	float period = 6.5 + 1.6 * clamp(hs, 0.0, 2.5);
	float lag = value_noise2(xz * 0.0105, 211u) * 2.4 + value_noise2(xz * 0.043, 223u) * 0.5;
	float cyc = t / period + lag;
	return vec2(cyc, surf_size(floor(cyc), xz));
}

float swash_front(float c, float reach) {
	float up = sin(clamp(c / 0.24, 0.0, 1.0) * 1.5708);
	float down = 1.0 - pow(clamp((c - 0.24) / 0.7, 0.0, 1.0), 0.75);
	return reach * (c < 0.24 ? up : down) - 0.04;
}

float run_up(float hs) {
	return 0.12 + 0.3 * clamp(hs, 0.0, 3.0);
}

vec4 shore_at(vec2 xz) {
	float m = wb_shore_p.y;
	if (m < 2.0) {
		return vec4(sea_level, 1e4, 0.0, 0.0);
	}
	vec2 g = clamp((xz + wb_shore_p.x) * ((m - 1.0) / (2.0 * wb_shore_p.x)), vec2(0.0), vec2(m - 1.001));
	ivec2 i = ivec2(floor(g));
	vec2 f = g - floor(g);
	vec4 a = texelFetch(wb_shore, i, 0);
	vec4 b = texelFetch(wb_shore, i + ivec2(1, 0), 0);
	vec4 c = texelFetch(wb_shore, i + ivec2(0, 1), 0);
	vec4 d = texelFetch(wb_shore, i + ivec2(1, 1), 0);
	vec2 v = mix(mix(a.xy, b.xy, f.x), mix(c.xy, d.xy, f.x), f.y);
	vec4 near = f.y > 0.5 ? (f.x > 0.5 ? d : c) : (f.x > 0.5 ? b : a);
	bool same = a.z == b.z && a.z == c.z && a.z == d.z && max(max(abs(a.x - b.x), abs(a.x - c.x)), abs(a.x - d.x)) < 3.0;
	return same ? vec4(v, near.z, 1.0) : vec4(near.xy, near.z, 1.0);
}

float puddle_amount(vec2 xz, float fill, float low, float flat_) {
	if (fill <= 0.0 || flat_ <= 0.0) {
		return 0.0;
	}
	float shape = value_noise2(xz * 0.21, 401u) * 0.65 + value_noise2(xz * 0.73, 409u) * 0.35;
	float z = (shape - 0.5) * 9.0 + low;
	float th = 2.2 - 2.0 * fill;
	return smoothstep(th, th + 0.35, z) * flat_;
}

vec2 rain_ripples(vec2 xz, float t, float rain) {
	vec2 q = xz * 3.3;
	vec2 base = floor(q);
	vec2 g = vec2(0.0);
	for (int j = -1; j <= 1; j++) {
		for (int i = -1; i <= 1; i++) {
			vec2 c = base + vec2(float(i), float(j));
			ivec2 ci = ivec2(c);
			float period = 0.6 + hashf(ci.x, ci.y, 421u) * 0.5;
			float ph = t / period + hashf(ci.x, ci.y, 431u);
			int cyc = int(floor(ph));
			float age = fract(ph);
			if (hashf(ci.x + cyc * 7, ci.y, 433u) > rain) {
				continue;
			}
			vec2 at = c + vec2(hashf(ci.x, ci.y + cyc * 13, 439u), hashf(ci.x + cyc * 3, ci.y, 443u));
			vec2 d = q - at;
			float r = length(d);
			float x = r - age * 1.25;
			float ring = sin(x * 25.0) * exp(-x * x * 30.0) * (1.0 - age) * (1.0 - age);
			g += d / max(r, 1e-3) * ring;
		}
	}
	return g;
}

// moving cloud shadows (the editor's cloud cover field, projected along the sun to 2000 m; they
// dim direct sunlight only) and the shade under the tree crowns (sky light only)
global uniform sampler2D wb_cover : filter_linear_mipmap, repeat_enable;
global uniform vec4 wb_cloud;
global uniform vec3 wb_sun_dir;
global uniform sampler2D wb_canopy : filter_linear, repeat_disable;
global uniform vec4 wb_canopy_grid;
global uniform vec4 wb_canopy_cam;

float cloud_cover_of(float n, float coverage) {
	float c = clamp(coverage, 0.0, 1.0);
	float thr = 1.0 - c;
	return max(smoothstep(thr, thr + 0.4, n), clamp((c - 0.75) * 4.0, 0.0, 1.0));
}

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

void vertex() {
	wpos = (MODEL_MATRIX * vec4(VERTEX, 1.0)).xyz;
	wnrm = normalize((MODEL_MATRIX * vec4(NORMAL, 0.0)).xyz);
}

float hash(vec2 p) {
	return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

float value_noise(vec2 p) {
	vec2 i = floor(p);
	vec2 f = fract(p);
	vec2 u = f * f * (3.0 - 2.0 * f);
	return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), u.x), mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), u.x), u.y);
}

vec2 rot(vec2 v, float c, float s) {
	return vec2(v.x * c - v.y * s, v.x * s + v.y * c);
}

// albedo + height (a), and normal xy (OpenGL, -1..1) + roughness + AO
void tex2(int k, vec2 uv, vec2 gx, vec2 gy, out vec4 a, out vec4 d) {
	vec3 p = vec3(uv, float(k));
	a.rgb = textureGrad(albedo_array, p, gx, gy).rgb;
	vec4 m = textureGrad(mask_array, p, gx, gy);
	vec2 n = textureGrad(normal_array, p, gx, gy).rg * 2.0 - 1.0;
	a.a = m.b;
	d = vec4(n, 1.0 - m.a, m.g);
}

// top-down sample (u east, v south) with the distance copy; pert is the world-space normal tilt
void top(int k, vec3 p, vec3 dpx, vec3 dpy, float far, out vec4 a, out vec4 d) {
	float tile = layer_tile[k];
	tex2(k, p.xz / tile, dpx.xz / tile, dpy.xz / tile, a, d);
	float amt = 0.8 * far;
	if (amt > 0.01) {
		float c = 0.8090;
		float s = 0.5878;
		float sc = tile * mix(1.618, 7.0, far);
		vec4 a2;
		vec4 d2;
		tex2(k, rot(p.xz, c, s) / sc + vec2(0.37, 0.71), rot(dpx.xz, c, s) / sc, rot(dpy.xz, c, s) / sc, a2, d2);
		d2.xy = rot(d2.xy, c, -s);
		// height-aware: whichever copy stands higher wins, so the photo keeps its contrast
		float h1 = a.a + 1.0 - amt;
		float h2 = a2.a + amt;
		float t = max(h1, h2) - 0.2;
		float w1 = max(h1 - t, 0.0);
		float w2 = max(h2 - t, 0.0);
		float f = w2 / max(w1 + w2, 1e-4);
		a = mix(a, a2, f);
		d = mix(d, d2, f);
	}
}

void layer(int k, vec3 p, vec3 n, vec3 dpx, vec3 dpy, float far, out vec3 albedo, out vec3 pert, out float rough, out float ao, out float height) {
	vec4 a;
	vec4 d;
	top(k, p, dpx, dpy, far, a, d);
	// OpenGL normal maps: green points up the image = -v = north (-Z) on the ground
	pert = vec3(d.x, 0.0, -d.y);
	if (layer_triplanar[k] == 1) {
		float tile = layer_tile[k];
		vec3 bw = pow(abs(n), vec3(4.0));
		bw /= (bw.x + bw.y + bw.z);
		vec4 ax;
		vec4 dx;
		vec4 az;
		vec4 dz;
		tex2(k, p.zy * vec2(1.0, -1.0) / tile, dpx.zy * vec2(1.0, -1.0) / tile, dpy.zy * vec2(1.0, -1.0) / tile, ax, dx);
		tex2(k, p.xy * vec2(1.0, -1.0) / tile, dpx.xy * vec2(1.0, -1.0) / tile, dpy.xy * vec2(1.0, -1.0) / tile, az, dz);
		a = a * bw.y + ax * bw.x + az * bw.z;
		rough = d.z * bw.y + dx.z * bw.x + dz.z * bw.z;
		ao = d.w * bw.y + dx.w * bw.x + dz.w * bw.z;
		// side projections: u along +Z / +X, image up along +Y
		pert = pert * bw.y + vec3(0.0, dx.y, dx.x) * bw.x + vec3(dz.x, dz.y, 0.0) * bw.z;
	} else {
		rough = d.z;
		ao = d.w;
	}
	albedo = a.rgb;
	height = a.a;
}

void fragment() {
	vec2 suv = (wpos.xz - terrain_origin) / terrain_size;
	vec4 s0 = texture(splat_0, suv);
	vec4 s1 = texture(splat_1, suv);
	vec4 s2 = texture(splat_2, suv);
	float w[12] = float[12](s0.r, s0.g, s0.b, s0.a, s1.r, s1.g, s1.b, s1.a, s2.r, s2.g, s2.b, s2.a);
	vec3 n = normalize(wnrm);
	vec3 dpx = dFdx(wpos);
	vec3 dpy = dFdy(wpos);
	float dist = length(wpos - CAMERA_POSITION_WORLD);
	float far = smoothstep(40.0, 260.0, dist);

	vec3 albedos[12];
	vec3 perts[12];
	float roughs[12];
	float aos[12];
	float heights[12];
	float max_h = -10.0;
	for (int k = 0; k < layer_count; k++) {
		if (w[k] > 0.004) {
			layer(k, wpos, n, dpx, dpy, far, albedos[k], perts[k], roughs[k], aos[k], heights[k]);
			heights[k] += w[k];
			max_h = max(max_h, heights[k]);
		}
	}
	vec3 albedo = vec3(0.0);
	vec3 pert = vec3(0.0);
	float rough = 0.0;
	float ao = 0.0;
	float wsum = 0.0;
	for (int k = 0; k < layer_count; k++) {
		if (w[k] > 0.004) {
			float hw = max(heights[k] - max_h + 0.25, 0.0) * w[k];
			albedo += albedos[k] * hw;
			pert += perts[k] * hw;
			rough += roughs[k] * hw;
			ao += aos[k] * hw;
			wsum += hw;
		}
	}
	albedo /= max(wsum, 1e-4);
	pert /= max(wsum, 1e-4);
	rough /= max(wsum, 1e-4);
	ao /= max(wsum, 1e-4);
	// large-scale tonal variation breaks up uniform areas
	float tone = value_noise(wpos.xz * 0.0018) * 0.6 + value_noise(wpos.xz * 0.0071) * 0.4;
	albedo *= 0.9 + 0.2 * tone;
	// ---- wetness and the shore (`terrain.wesl`): rain, the swash on sea beaches, lapped lake
	// shores, damp river banks, the charred and glowing rims of lava; rain puddles
	float snow_w = 0.0;
	float holds = 0.0;
	for (int k = 0; k < layer_count; k++) {
		snow_w += w[k] * layer_snow[k];
		holds += w[k] * layer_holds[k];
	}
	vec4 sh = shore_at(wpos.xz);
	float above = wpos.y - sh.x;
	int shore_kind = int(sh.z + 0.5);
	float wet = wb_rain.x * (1.0 - snow_w);
	float film = 0.0;
	float foam = 0.0;
	vec3 glow = vec3(0.0);
	float t = TIME;
	float edge_n = shore_kind != 0 ? value_noise2(wpos.xz * 0.6, 241u) - 0.5 : 0.0;
	if (shore_kind == 1 && sh.y < 120.0 && above < 3.0) {
		float hs = wb_shore_p.z;
		vec2 cyc = surf_cycle(wpos.xz, t, hs);
		float c = fract(cyc.x);
		float reach = run_up(hs) * cyc.y;
		float front = swash_front(c, reach) + edge_n * 0.05 + (value_noise2(wpos.xz * 2.1, 243u) - 0.5) * 0.025;
		film = smoothstep(-0.004, 0.03, front - above);
		float lace = value_noise2(wpos.xz * 3.1 + vec2(c * 2.0, 0.0), 247u) * 0.6 + value_noise2(wpos.xz * 9.0, 251u) * 0.4;
		float lead = 1.0 - smoothstep(0.0, 0.035 + 0.04 * cyc.y, abs(front - 0.012 - above));
		float rush = 1.0 - smoothstep(0.18, 0.6, c);
		float stranded = smoothstep(0.0, 0.03, above - front) * (1.0 - smoothstep(0.0, 0.12, above - front)) * smoothstep(0.24, 0.32, c) * (1.0 - smoothstep(0.5, 0.95, c));
		foam = max(lead * smoothstep(0.32, 0.62, lace) * mix(0.45, 1.0, rush), stranded * smoothstep(0.6, 0.78, lace) * 0.8);
		foam *= clamp(wb_shore_p.w, 0.0, 1.0) * smoothstep(0.0, 0.05, reach);
		float top = run_up(hs) * (0.95 + 0.35 * value_noise2(wpos.xz * 0.045, 253u)) + 0.06;
		wet = max(wet, 1.0 - smoothstep(top * 0.4, top, above + edge_n * 0.06));
	} else if (shore_kind == 2 && sh.y < 30.0 && above < 1.0 && above > -0.2) {
		float lap = sin(t * 1.3 + dot(wpos.xz, vec2(0.21, 0.17)) + edge_n * 5.0) * 0.5 + 0.5;
		float near = 1.0 - smoothstep(2.0, 7.0, sh.y);
		film = smoothstep(-0.004, 0.012, 0.015 + 0.045 * lap - above) * smoothstep(-0.2, -0.05, above) * near;
		wet = max(wet, (1.0 - smoothstep(0.06, 0.3, above + edge_n * 0.06)) * (1.0 - smoothstep(4.0, 12.0, sh.y)));
	} else if (shore_kind == 3 && sh.y < 30.0 && above < 2.0 && above > -0.2) {
		wet = max(wet, (1.0 - smoothstep(0.05, 0.75, above + edge_n * 0.3)) * 0.85 * (1.0 - smoothstep(6.0, 16.0, sh.y)));
		film = smoothstep(-0.004, 0.01, 0.02 - above) * smoothstep(-0.2, -0.05, above) * (1.0 - smoothstep(2.0, 7.0, sh.y));
	} else if (shore_kind == 4 && sh.y < 70.0 && above < 25.0) {
		float rim = (1.0 - smoothstep(0.0, 2.4 + edge_n * 1.6, sh.y)) * (1.0 - smoothstep(0.1, 1.4, above));
		float flick = 0.7 + 0.3 * value_noise2(vec2(t * 1.9, 0.0) + wpos.xz * 0.45, 257u);
		albedo *= mix(1.0, 0.4, 1.0 - smoothstep(0.0, 10.0, sh.y));
		float dark = 1.0 - smoothstep(-0.08, 0.25, normalize(wb_sun_dir).y);
		float reach = exp(-sh.y / 6.0) * (1.0 - smoothstep(3.0, 18.0, above));
		glow = vec3(1.0, 0.3, 0.05) * rim * rim * flick * 1.6 + albedo * vec3(1.0, 0.34, 0.08) * reach * mix(0.15, 1.6, dark) * (0.85 + 0.15 * flick);
	}
	wet = clamp(wet, 0.0, 1.0);
	albedo *= mix(1.0, 0.6, wet);
	rough = mix(rough, 0.3, wet * 0.85);
	albedo = mix(albedo, albedo * 0.55 + wb_water_tint * 0.03, film);
	rough = mix(rough, 0.035, film);
	albedo = mix(albedo, vec3(0.8, 0.83, 0.84), foam);
	rough = mix(rough, 0.7, foam);
	float puddle = 0.0;
	vec2 ripple = vec2(0.0);
	if (wb_rain.y > 0.0 && dist < 420.0) {
		float flat_ = smoothstep(0.93, 0.975, n.y) * (1.0 - snow_w) * smoothstep(0.1, 0.3, wpos.y - sea_level) * (1.0 - smoothstep(300.0, 420.0, dist));
		float low = (min(holds, 1.0) - 1.0) * 4.0 - (1.0 - n.y) * 25.0;
		puddle = puddle_amount(wpos.xz, wb_rain.y, low, flat_);
		albedo = mix(albedo, albedo * 0.45, puddle);
		rough = mix(rough, 0.02, puddle);
		if (puddle > 0.0 && wb_rain.z > 0.0 && dist < 45.0) {
			ripple = rain_ripples(wpos.xz, t, wb_rain.z) * (1.0 - smoothstep(25.0, 45.0, dist));
		}
	}
	// a sheet of water and a puddle smooth the ground's relief (rings from the drops)
	pert *= 1.0 - max(film * 0.85, puddle);
	pert -= vec3(ripple.x, 0.0, ripple.y) * (0.12 * puddle);
	// (display units: the rim of lava glows the same by day and by night)
	EMISSION = glow / max(wb_exposure, 1e-3);

	vec3 nw = normalize(n + pert * normal_strength);
	NORMAL = normalize((VIEW_MATRIX * vec4(nw, 0.0)).xyz);
	ALBEDO = albedo;
	ROUGHNESS = clamp(rough, 0.04, 1.0);
	SPECULAR = 0.35;
	AO = ao * canopy_sky(wpos);
	AO_LIGHT_AFFECT = 0.4;
	vec3 L = normalize(wb_sun_dir);
	vec2 cloud_p = wpos.xz + L.xz * ((2000.0 - wpos.y) / max(L.y, 0.05));
	float cloud = smoothstep(0.25, 0.7, cloud_cover_of(texture(wb_cover, (cloud_p + wb_cloud.zw) / 32768.0).r, wb_cloud.x));
	v_cloud_sh = 1.0 - cloud * wb_cloud.y;
}

// the standard lights, with the cloud shadow on the sun (and the moon)
void light() {
	float shade = LIGHT_IS_DIRECTIONAL ? v_cloud_sh : 1.0;
	float ndl = max(dot(NORMAL, LIGHT), 0.0);
	DIFFUSE_LIGHT += LIGHT_COLOR * ATTENUATION * ndl * shade / PI;
	vec3 H = normalize(VIEW + LIGHT);
	float ndh = max(dot(NORMAL, H), 0.0);
	float ndv = max(dot(NORMAL, VIEW), 1e-3);
	float a = ROUGHNESS * ROUGHNESS;
	float a2 = a * a;
	float dd = ndh * ndh * (a2 - 1.0) + 1.0;
	float D = a2 / (PI * dd * dd);
	float k = a * 0.5;
	float vis = 0.25 / ((ndl * (1.0 - k) + k) * (ndv * (1.0 - k) + k));
	float F = 0.0306 + 0.9694 * pow(1.0 - max(dot(VIEW, H), 0.0), 5.0);
	SPECULAR_LIGHT += LIGHT_COLOR * ATTENUATION * ndl * D * vis * F * shade / PI;
}
