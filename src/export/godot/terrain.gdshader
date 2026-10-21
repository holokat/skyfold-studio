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

varying vec3 wpos;
varying vec3 wnrm;

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
	// wet shoreline
	float wet = 1.0 - smoothstep(0.0, 1.2, wpos.y - sea_level);
	albedo *= mix(1.0, 0.55, wet);
	rough = mix(rough, 0.12, wet * 0.85);

	vec3 nw = normalize(n + pert * normal_strength);
	NORMAL = normalize((VIEW_MATRIX * vec4(nw, 0.0)).xyz);
	ALBEDO = albedo;
	ROUGHNESS = clamp(rough, 0.04, 1.0);
	SPECULAR = 0.35;
	AO = ao;
	AO_LIGHT_AFFECT = 0.4;
}
