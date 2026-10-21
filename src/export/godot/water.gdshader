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
uniform sampler2D ripple_normal : hint_normal, filter_linear_mipmap_anisotropic, repeat_enable;
uniform sampler2D screen_tex : hint_screen_texture, filter_linear_mipmap, repeat_disable;
uniform sampler2D depth_tex : hint_depth_texture, filter_linear, repeat_disable;

varying vec3 wpos;
varying float crest;

vec3 gerstner(vec2 dir, float wavelength, float steep, vec2 p, float t, inout vec3 tangent, inout vec3 binormal) {
	float k = 6.28318 / wavelength;
	float c = sqrt(9.8 / k);
	vec2 d = normalize(dir);
	float f = k * (dot(d, p) - c * t);
	float a = steep / k;
	tangent += vec3(-d.x * d.x * steep * sin(f), d.x * steep * cos(f), -d.x * d.y * steep * sin(f));
	binormal += vec3(-d.x * d.y * steep * sin(f), d.y * steep * cos(f), -d.y * d.y * steep * sin(f));
	return vec3(d.x * a * cos(f), a * sin(f), d.y * a * cos(f));
}

void vertex() {
	wpos = (MODEL_MATRIX * vec4(VERTEX, 1.0)).xyz;
	crest = 0.0;
	if (ocean && wave_height > 0.01) {
		float t = TIME * wave_speed;
		float s = clamp(wave_height, 0.0, 2.5) * 0.11;
		vec3 tangent = vec3(1.0, 0.0, 0.0);
		vec3 binormal = vec3(0.0, 0.0, 1.0);
		vec3 d = vec3(0.0);
		d += gerstner(vec2(1.0, 0.35), 72.0, s, wpos.xz, t, tangent, binormal);
		d += gerstner(vec2(0.6, -0.8), 41.0, s * 0.8, wpos.xz, t, tangent, binormal);
		d += gerstner(vec2(-0.3, 1.0), 23.0, s * 0.6, wpos.xz, t, tangent, binormal);
		d += gerstner(vec2(0.9, 0.5), 11.0, s * 0.4, wpos.xz, t, tangent, binormal);
		VERTEX += d;
		wpos += d;
		// highest crests foam (amplitudes are steepness × wavelength / 2π)
		float amp = s / 6.28318 * (72.0 + 41.0 * 0.8 + 23.0 * 0.6 + 11.0 * 0.4);
		crest = smoothstep(0.75, 1.0, d.y / max(amp, 0.01));
		NORMAL = normalize(cross(binormal, tangent));
	}
}

void fragment() {
	float t = TIME * wave_speed;
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
	NORMAL_MAP_DEPTH = 0.35 + clamp(wave_height, 0.0, 2.5) * 0.25;

	// the bed behind this pixel (Godot 4.3+: reversed Z, NDC depth 0..1)
	float depth = texture(depth_tex, SCREEN_UV).r;
	vec4 bed = INV_VIEW_MATRIX * INV_PROJECTION_MATRIX * vec4(SCREEN_UV * 2.0 - 1.0, depth, 1.0);
	bed.xyz /= bed.w;
	float ray = length(bed.xyz - wpos);
	float column = max(wpos.y - bed.y, 0.0);
	// refraction: shift the view of the bed by the ripples, less in very shallow water
	vec2 offset = nm.xy * 0.03 * clamp(ray / 4.0, 0.0, 1.0);
	vec3 behind = texture(screen_tex, SCREEN_UV + offset).rgb;
	float vis = max(clarity, 0.1);
	float see = exp(-ray / vis * 2.0) * clamp(transparency, 0.0, 1.0);
	vec3 body = mix(shallow_color, deep_color, 1.0 - exp(-column / (vis * 1.5)));
	float shore = (1.0 - smoothstep(0.0, 0.35 + shore_foam * 0.9, ray)) * shore_foam;
	float f = clamp(shore + crest * foam, 0.0, 1.0);

	ALBEDO = mix(body * (1.0 - see), vec3(0.9), f);
	EMISSION = behind * see * (1.0 - f) * mix(vec3(1.0), shallow_color * 1.4, 0.35) + emissive_color * emissive_strength;
	// a little roughness keeps sun glints on the ripples from sparkling into white dashes
	ROUGHNESS = mix(clamp(roughness_amount, 0.06, 1.0), 0.8, f);
	METALLIC = metallic_amount;
	SPECULAR = 0.25;
}
