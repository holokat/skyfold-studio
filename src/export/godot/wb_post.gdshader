// Skyfold Studio lens effects over the whole view: the look's vignette (Bevy's curve: radius 0.9,
// smoothness 2.5, edge compensation 1) and film grain, as a quad multiplied over everything after
// it is drawn (water, rain and fires included). The colour grade is the Environment's LUT;
// sharpening is Godot's FSR at full resolution (project settings).
shader_type spatial;
render_mode unshaded, blend_mul, cull_disabled, depth_test_disabled, depth_draw_never, shadows_disabled, fog_disabled;

uniform float vignette = 0.0;
uniform float grain = 0.0;

void vertex() {
	// a full-screen quad in clip space, in front of everything
	POSITION = vec4(VERTEX.xy * 2.0, 1.0, 1.0);
}

float hash12(vec2 p) {
	vec3 p3 = fract(vec3(p.xyx) * 0.1031);
	p3 += dot(p3, p3.yzx + 33.33);
	return fract((p3.x + p3.y) * p3.z);
}

void fragment() {
	vec2 uv = SCREEN_UV;
	vec2 res = VIEWPORT_SIZE;
	float k = 1.0;
	if (vignette > 0.001) {
		vec2 scale_vec = res / min(res.x, res.y);
		scale_vec.x *= min(res.x, res.y) / res.x;
		vec2 f = (uv - 0.5) * scale_vec;
		float d = length(f) / 0.9;
		float factor = pow(clamp(1.0 - d * d, 0.0, 1.0), 2.5);
		k *= 1.0 - (1.0 - factor) * vignette;
	}
	if (grain > 0.0001) {
		float n = hash12(FRAGCOORD.xy + vec2(fract(TIME * 7.13) * 917.0, fract(TIME * 3.71) * 491.0)) - 0.5;
		k *= 1.0 + n * grain * 1.8;
	}
	ALBEDO = vec3(k);
}
