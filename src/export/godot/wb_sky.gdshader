// Skyfold Studio sky: the editor's sky colours for the hour, the sun's disc, stars at night, the
// look's sky tint, and the editor's volumetric clouds (`clouds.wesl`, ported): a cumulus layer
// ray marched through the same density (the baked cover field, a height profile by cloud type,
// the Perlin–Worley base shape and Worley detail erosion), lit by a short march towards the sun
// with three scattering octaves, and the cirrus veil. The clouds march at half resolution (as the
// editor's); the reflections' cubemap reads a cheap cover-map deck instead. `wb_atmosphere.gd`
// moves the cover and the weather; the layer is flat (the editor's sits on the planet's curve).
shader_type sky;
render_mode use_half_res_pass;

uniform vec3 top_color : source_color = vec3(0.46, 0.6, 0.8);
uniform vec3 horizon_color : source_color = vec3(0.71, 0.79, 0.85);
uniform vec3 ground_color : source_color = vec3(0.16, 0.17, 0.15);
uniform float sky_energy = 1.0;
// cover 0..1 (the weather moves it), type 0 stratus … 1 towering cumulus, optical thickness,
// cirrus amount, march steps and light samples (the world's cloud quality)
uniform float cloud_cover = 0.25;
uniform float cloud_type = 0.5;
uniform float cloud_density = 1.0;
uniform float cirrus = 0.2;
uniform float steps = 48.0;
uniform float light_steps = 4.0;
uniform vec2 cloud_offset = vec2(0.0);
uniform float cloud_seed = 0.0;
// weather: grey the sky, rain (denser clouds), night (stars), lightning
uniform float dim = 0.0;
uniform float rain = 0.0;
uniform float night = 0.0;
uniform float stars = 1.0;
uniform float flash = 0.0;
// the look's sky tint (sRGB) and how far the sky leans to it
uniform vec3 sky_tint : source_color = vec3(0.55, 0.7, 0.95);
uniform float sky_tint_amount = 0.0;
uniform sampler3D base_tex : filter_linear, repeat_enable;
uniform sampler3D detail_tex : filter_linear, repeat_enable;
uniform sampler2D cirrus_tex : filter_linear_mipmap, repeat_enable;
// the editor's cloud cover field (one 32 km tile) and (coverage, shadows, offset x, offset z)
global uniform sampler2D wb_cover : filter_linear_mipmap, repeat_enable;
global uniform vec4 wb_cloud;

const float CLOUD_TILE = 32768.0;
const float BASE_TILE = 4096.0;
const float DETAIL_TILE = 512.0;
const float EVOLVE = 1.1378;
const float MAX_DIST = 65000.0;
const float MS_GAIN = 2.0;
const float CLOUD_BASE = 1500.0;
const float CIRRUS_ALT = 8500.0;

float cloud_cover_of(float n, float coverage) {
	float c = clamp(coverage, 0.0, 1.0);
	float thr = 1.0 - c;
	return max(smoothstep(thr, thr + 0.4, n), clamp((c - 0.75) * 4.0, 0.0, 1.0));
}

float hash3(vec3 p) {
	p = fract(p * 0.1031);
	p += dot(p, p.zyx + 31.32);
	return fract((p.x + p.y) * p.z);
}

float remap01(float v, float lo, float hi) {
	return clamp((v - lo) / (hi - lo), 0.0, 1.0);
}

float hg(float c, float g) {
	float g2 = g * g;
	return (1.0 - g2) / (4.0 * PI * pow(max(1.0 + g2 - 2.0 * g * c, 1e-4), 1.5));
}

float cover_at(vec2 xz) {
	float n = textureLod(wb_cover, (xz + wb_cloud.zw) / CLOUD_TILE, 0.0).r;
	return cloud_cover_of(n, cloud_cover);
}

float layer_type() {
	return cloud_type + (0.4 - cloud_type) * smoothstep(0.8, 1.0, cloud_cover) * 0.7;
}

float profile(float hf, float w) {
	float top = mix(0.3, 1.0, w);
	return smoothstep(0.0, 0.1, hf) * (1.0 - smoothstep(top * mix(0.2, 0.5, layer_type()), top, hf));
}

float shoulders(float hf, float w) {
	float r = clamp(hf / mix(0.3, 1.0, w), 0.0, 1.0);
	return 0.22 * r * r;
}

float density(vec3 p, float hf, float w, bool detail) {
	vec2 q = p.xz + wb_cloud.zw;
	float evolve = TIME * EVOLVE;
	vec4 b = textureLod(base_tex, vec3(q.x, p.y + evolve, q.y) / BASE_TILE, 0.0);
	float wf = b.g * 0.625 + b.b * 0.25 + b.a * 0.125;
	float shape = remap01(b.r, wf - 1.0, 1.0);
	float raw = shape * profile(hf, w) - (1.0 - w) - shoulders(hf, w);
	if (raw < -0.25) {
		return 0.0;
	}
	float m = 0.5;
	if (detail) {
		vec2 swirl = (b.gb - 0.5) * (0.35 * (1.0 - 0.5 * hf));
		vec4 n = textureLod(detail_tex, vec3(q.x, p.y - evolve, q.y) / DETAIL_TILE + vec3(swirl.x, 0.0, swirl.y), 0.0);
		float df = n.r * 0.625 + n.g * 0.25 + n.b * 0.125;
		m = mix(df, 1.0 - df, clamp(hf * 6.0, 0.0, 1.0));
	}
	raw -= m * 0.24;
	return clamp(raw * 4.0, 0.0, 1.0);
}

float light_depth(vec3 p, vec3 l, float base, float top, float sigma) {
	float H = top - base;
	float od = 0.0;
	float t = 0.0;
	int n = int(light_steps);
	for (int j = 0; j < 8; j++) {
		if (j >= n) {
			break;
		}
		float dt = 45.0 * exp2(float(j)) * (5.0 / light_steps);
		float tm = t + dt * 0.5;
		t += dt;
		vec3 ps = p + l * tm;
		float hf = (ps.y - base) / H;
		if (hf > 1.0 || hf < 0.0) {
			break;
		}
		float w = cover_at(ps.xz);
		if (w > 0.003) {
			od += density(ps, hf, w, false) * dt;
		}
	}
	return od * sigma;
}

// the editor's gradient: zenith colour high, horizon colour low (below the horizon, only ever
// seen in reflections, the horizon colour carries on a little darker), greyed by the weather
vec3 sky_base(vec3 d) {
	float up = d.y;
	vec3 col = mix(horizon_color, top_color, pow(clamp(up, 0.0, 1.0), 0.45));
	col = mix(col, horizon_color * 0.85, smoothstep(0.0, -0.3, up));
	float lum = dot(col, vec3(0.2126, 0.7152, 0.0722));
	return mix(col, vec3(lum) * vec3(0.92, 0.95, 1.0), dim * 0.75);
}

// the cumulus layer and the cirrus along view direction d from the eye: premultiplied colour, alpha
vec4 clouds(vec3 d, vec3 cam, vec2 px, vec3 L, vec3 key, vec3 sky_far) {
	float ty = layer_type();
	float dens_k = cloud_density * (1.0 + 0.8 * rain + 0.4 * dim);
	float base = CLOUD_BASE;
	float top = CLOUD_BASE + 400.0 + 2600.0 * ty * ty;
	float sigma = 0.06 * dens_k;
	float t0 = 0.0;
	float t1 = -1.0;
	if (cam.y < base) {
		if (d.y > 0.0) {
			t0 = (base - cam.y) / d.y;
			t1 = (top - cam.y) / d.y;
		}
	} else if (cam.y < top) {
		t1 = d.y > 0.0 ? (top - cam.y) / d.y : (d.y < 0.0 ? (base - cam.y) / d.y : MAX_DIST);
	} else if (d.y < 0.0) {
		t0 = (top - cam.y) / d.y;
		t1 = (base - cam.y) / d.y;
	}
	t1 = min(t1, MAX_DIST);
	float ck = dot(d, L);
	float pk0 = mix(hg(ck, 0.8), hg(ck, -0.25), 0.3);
	float pk1 = mix(hg(ck, 0.48), hg(ck, -0.15), 0.3);
	float pk2 = mix(hg(ck, 0.29), hg(ck, -0.09), 0.3);
	// sky light from above (the gradient averaged over the upper hemisphere) and the ground's
	vec3 amb_top = (top_color * 0.36 + mix(horizon_color, top_color, 0.4) * 0.64) * sky_energy * (1.0 - 0.55 * dim);
	vec3 ground = key * max(L.y, 0.0) * (0.2 / PI);
	vec3 amb_bot = amb_top * 0.5 + ground;
	float T = 1.0;
	vec3 Lr = vec3(0.0);
	float t_acc = 0.0;
	float w_acc = 0.0;
	float seg = t1 - t0;
	if (seg > 0.0) {
		float H = top - base;
		float n = clamp(seg / 55.0, 24.0, steps);
		float ds0 = seg / n;
		float jit = fract(52.9829189 * fract(0.06711056 * px.x + 0.00583715 * px.y));
		float t = t0 + ds0 * jit;
		int fine = 0;
		for (int i = 0; i < 256; i++) {
			if (t >= t1 || T < 0.01) {
				break;
			}
			float coarse = ds0 * (1.0 + t / 30000.0);
			float ds = fine > 0 ? max(coarse * 0.18, 16.0) : coarse;
			vec3 pw = cam + d * t;
			float hf = (pw.y - base) / H;
			float dens = 0.0;
			if (hf >= 0.0 && hf <= 1.0) {
				float w = cover_at(pw.xz);
				if (w > 0.003) {
					dens = density(pw, hf, w, true) * smoothstep(450.0 * 0.6, 450.0, t);
				}
			}
			if (dens > 0.001) {
				if (fine == 0 && t - coarse > t0) {
					fine = 10;
					t -= coarse;
					continue;
				}
				fine = 10;
				float lod = 1.0 / (1.0 + t / 12000.0);
				float st = dens * sigma * lod;
				float od = light_depth(pw, L, base, top, sigma) * lod;
				float ms = pk0 * exp(-od) + MS_GAIN * (0.85 * pk1 * exp(-od * 0.3) + 0.72 * pk2 * exp(-od * 0.1) + 0.048 / (1.0 + od * 0.12));
				vec3 amb = mix(amb_bot, amb_top, clamp(hf * 1.3, 0.0, 1.0)) * (1.0 - 0.4 * clamp(dens * 1.5, 0.0, 1.0));
				vec3 lum = key * ms + amb;
				vec3 s = st * lum;
				float tr = exp(-st * ds);
				Lr += T * (s - s * tr) / max(st, 1e-7);
				float dt = T * (1.0 - tr);
				t_acc += t * dt;
				w_acc += dt;
				T *= tr;
			} else if (fine > 0) {
				fine -= 1;
			}
			t += ds;
		}
	}
	vec3 col = vec3(0.0);
	float alpha = 0.0;
	if (w_acc > 1e-5) {
		float tc = t_acc / w_acc;
		alpha = T < 0.02 ? 1.0 : 1.0 - T;
		col = Lr;
		// distant clouds melt into the horizon haze, then fade out
		col = mix(col, sky_far * alpha * 1.1, smoothstep(6000.0, 50000.0, tc) * 0.8);
		float fade = 1.0 - smoothstep(30000.0, MAX_DIST, tc);
		col *= fade;
		alpha *= fade;
	}
	// the cirrus veil: a thin textured shell, ice-crystal phase
	if (cirrus > 0.001 && d.y > 0.0 && cam.y < CIRRUS_ALT) {
		float tcir = (CIRRUS_ALT - cam.y) / d.y;
		vec3 pw = cam + d * tcir;
		vec2 q = (pw.xz + wb_cloud.zw * 1.4) / CLOUD_TILE;
		float lodc = log2(max(tcir * 0.0015 / max(d.y, 0.05) / (CLOUD_TILE / 1024.0), 1.0));
		vec4 c1 = textureLod(cirrus_tex, q, lodc);
		vec4 c2 = textureLod(cirrus_tex, vec2(q.y, -q.x) * 3.0 + 0.37, lodc + 1.585);
		float fib = mix(c1.r, c1.b, smoothstep(0.35, 0.65, c2.g));
		float veil = c1.g * 0.7 + c2.g * 0.3;
		float cov = smoothstep(1.0 - cirrus, 1.0 - cirrus + 0.4, veil);
		float tau = cirrus * cov * (0.1 + 0.9 * fib * fib) * 0.4;
		float ca = (1.0 - exp(-tau / max(d.y, 0.15))) * (1.0 - smoothstep(120000.0, 260000.0, tcir));
		if (ca > 1e-4) {
			float hd = (acos(clamp(ck, -1.0, 1.0)) - 0.384) / 0.02;
			float phase = hg(ck, 0.75) * 0.45 + hg(ck, 0.2) * 0.35 + hg(ck, -0.2) * 0.2 + exp(-hd * hd) * 0.03;
			vec3 cc = (key * phase + amb_top * 0.9) * ca;
			col += (1.0 - alpha) * cc;
			alpha = alpha + (1.0 - alpha) * ca;
		}
	}
	return vec4(col, alpha);
}

void sky() {
	vec3 d = normalize(EYEDIR);
	float up = d.y;
	vec3 L = LIGHT0_ENABLED ? LIGHT0_DIRECTION : vec3(0.0, 1.0, 0.0);
	vec3 sun_col = LIGHT0_ENABLED ? LIGHT0_COLOR * LIGHT0_ENERGY : vec3(0.0);
	if (AT_HALF_RES_PASS) {
		// the volumetric clouds (the key light dimmed under a gloomy deck, as the editor's)
		vec3 key = sun_col * (1.0 - 0.6 * dim);
		vec4 c = clouds(d, POSITION, FRAGCOORD.xy, L, key, sky_base(d) * sky_energy);
		COLOR = c.rgb;
		ALPHA = c.a;
	} else {
		vec3 col = sky_base(d);
		float sd = dot(d, L);
		if (!AT_CUBEMAP_PASS) {
			if (night > 0.01 && stars > 0.5 && up > 0.0) {
				vec3 cell = floor(d * 240.0);
				float s = hash3(cell + 17.0);
				vec3 at = (cell + vec3(hash3(cell + 3.1), hash3(cell + 5.7), hash3(cell + 9.3))) / 240.0;
				float star = smoothstep(0.0016, 0.0006, length(d - normalize(at))) * step(0.996, s);
				float tw = 0.75 + 0.25 * sin(TIME * 2.0 + s * 40.0);
				col += vec3(star * night * tw * 1.6) * smoothstep(0.0, 0.15, up);
			}
			col += sun_col * (smoothstep(0.9996, 0.99985, sd) * 18.0 + pow(max(sd, 0.0), 300.0) * 0.6) * (1.0 - dim * 0.9);
		}
		col += sun_col * pow(max(sd, 0.0), 8.0) * 0.12 * (1.0 - dim);
		if (sky_tint_amount > 0.001) {
			float l = dot(col, vec3(0.2126, 0.7152, 0.0722));
			vec3 tinted = sky_tint * (l / max(dot(sky_tint, vec3(0.2126, 0.7152, 0.0722)), 1e-3));
			col = mix(col, tinted, sky_tint_amount * (0.4 + 0.6 * smoothstep(-0.05, 0.5, up)));
		}
		col *= sky_energy;
		if (AT_CUBEMAP_PASS) {
			// reflections: the clouds as a cheap deck from the same cover field
			if (up > 0.0) {
				vec2 hit = POSITION.xz + d.xz / max(up, 0.02) * max(1750.0 - POSITION.y, 50.0);
				float c = cloud_cover_of(textureLod(wb_cover, (hit + wb_cloud.zw) / CLOUD_TILE, 3.0).r, cloud_cover) * smoothstep(0.0, 0.08, up);
				vec3 cc = (sun_col * 0.25 + horizon_color * 0.8 * sky_energy) * (1.0 - 0.5 * dim);
				col = mix(col, cc, c * 0.85);
			}
		} else {
			col = col * (1.0 - HALF_RES_COLOR.a) + HALF_RES_COLOR.rgb;
		}
		col += vec3(0.75, 0.8, 1.0) * flash * 2.0;
		COLOR = col;
	}
}
