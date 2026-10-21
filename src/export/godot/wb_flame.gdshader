// A Skyfold Studio fire: AetherFX's flames baked to a looping flipbook (premultiplied colour and
// coverage), on a quad that turns about the upright to face the camera. Two neighbouring frames
// are blended; each fire runs at its own phase and pace (`seed`) and half of them mirrored.
// Magic fires take their light's colour (`tint`, w: how much). Soft where it meets walls and
// floors. The node's origin is the frame's bottom middle, its scale the frame's size (m). A wide
// fire (a pit, a bowl) is one flame as wide as its bed: the frame is `spread` times the sheet's
// width, the flame stretched across all of it at the foot and narrowing back as it rises.
shader_type spatial;
render_mode blend_premul_alpha, unshaded, cull_disabled, depth_draw_never, shadows_disabled;

uniform sampler2D sheet : source_color, filter_linear, repeat_disable;
// columns, rows, frames, frames per second
uniform vec4 grid = vec4(8.0, 6.0, 48.0, 24.0);
uniform float strength = 2.2;
uniform vec4 tint = vec4(1.0, 0.62, 0.3, 0.0);
uniform float occlusion = 0.35;
uniform float soft = 0.18;
uniform sampler2D depth_tex : hint_depth_texture, filter_nearest;
instance uniform float seed = 0.0;
instance uniform float spread = 1.0;

void vertex() {
	vec3 origin = MODEL_MATRIX[3].xyz;
	float sx = length(MODEL_MATRIX[0].xyz);
	float sy = length(MODEL_MATRIX[1].xyz);
	vec3 to = INV_VIEW_MATRIX[3].xyz - origin;
	to.y = 0.0;
	vec3 z = length(to) > 1e-4 ? normalize(to) : vec3(0.0, 0.0, 1.0);
	vec3 x = normalize(cross(vec3(0.0, 1.0, 0.0), z));
	vec3 world = origin + x * VERTEX.x * sx + vec3(0.0, VERTEX.y * sy, 0.0);
	POSITION = PROJECTION_MATRIX * VIEW_MATRIX * vec4(world, 1.0);
}

vec2 cell(float i, vec2 st) {
	return (vec2(mod(i, grid.x), floor(i / grid.x)) + st) / grid.xy;
}

void fragment() {
	float frames = grid.z;
	float pace = 1.0 + (fract(seed * 13.71) - 0.5) * 0.16;
	float f = TIME * grid.w * pace + seed * frames;
	float i0 = mod(f, frames);
	float a = floor(i0);
	float b = mod(a + 1.0, frames);
	float k = i0 - a;
	vec2 st = UV;
	float s = max(spread, 1.0);
	float wide = mix(s, 1.0, smoothstep(0.06, 0.78, 1.0 - UV.y));
	st.x = 0.5 + (UV.x - 0.5) * s / wide;
	float inside = smoothstep(0.0, 0.04, st.x) * smoothstep(1.0, 0.96, st.x);
	st.x = clamp(st.x, 0.0, 1.0);
	if (fract(seed * 7.31) > 0.5) {
		st.x = 1.0 - st.x;
	}
	vec2 px = 0.5 / vec2(textureSize(sheet, 0)) * grid.xy;
	st = clamp(st, px, vec2(1.0) - px);
	vec4 c = mix(texture(sheet, cell(a, st)), texture(sheet, cell(b, st)), k);
	float heat = max(c.r, max(c.g, c.b));
	vec3 magic = tint.rgb * heat * 1.2 + vec3(pow(heat, 3.0) * 0.5);
	vec3 col = mix(c.rgb, magic, tint.w);
	// soft against the walls and the frame's edges (UV.y runs down from the top)
	float edge = smoothstep(0.0, 0.06, UV.x) * smoothstep(1.0, 0.94, UV.x) * smoothstep(0.0, 0.1, UV.y);
	float depth = texture(depth_tex, SCREEN_UV).r;
	vec4 v = INV_PROJECTION_MATRIX * vec4(SCREEN_UV * 2.0 - 1.0, depth, 1.0);
	float gap = (-v.z / v.w) - (-VERTEX.z);
	float m = edge * inside * smoothstep(0.0, soft, gap);
	ALBEDO = col * strength * m;
	ALPHA = clamp(c.a * occlusion * m, 0.0, 1.0);
}
