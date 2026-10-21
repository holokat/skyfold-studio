// Art-style post pass, after tonemapping: painterly (smooth Kuwahara) filter, ink outlines from
// depth, colour haze and sky tint, split toning, lift, vibrance, foliage hue and film grain.
// Colour work happens in display (gamma) space; the input and output are linear.

import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

struct LookPost {
    // x: painterly strength, y: painterly radius (px), z: outline strength, w: film grain
    paint: vec4<f32>,
    // rgb: shadow tint, w: lift
    shadow: vec4<f32>,
    // rgb: highlight tint, w: vibrance
    highlight: vec4<f32>,
    // rgb: sky tint (linear), w: amount
    sky: vec4<f32>,
    // rgb: haze colour (linear), w: amount
    haze: vec4<f32>,
    // x: near plane, y: time, z: foliage hue shift, w: haze distance (m)
    misc: vec4<f32>,
    // xyz: camera right, w: tan(fov/2) * aspect
    cam_right: vec4<f32>,
    // xyz: camera up, w: tan(fov/2)
    cam_up: vec4<f32>,
    // xyz: camera forward, w: window scale factor
    cam_fwd: vec4<f32>,
    // x: distance (m) where the colour haze starts, y: contrast (display space)
    extra: vec4<f32>,
}

@group(0) @binding(0) var screen_tex: texture_2d<f32>;
@group(0) @binding(1) var screen_samp: sampler;
@group(0) @binding(2) var depth_tex: texture_depth_2d;
@group(0) @binding(3) var<uniform> look: LookPost;

const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

fn to_gamma(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.2));
}

fn hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn rgb_to_hsv(c: vec3<f32>) -> vec3<f32> {
    let k = vec4<f32>(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    let p = select(vec4<f32>(c.gb, k.xy), vec4<f32>(c.bg, k.wz), c.g < c.b);
    let q = select(vec4<f32>(p.xyw, c.r), vec4<f32>(c.r, p.yzx), p.x < c.r);
    let d = q.x - min(q.w, q.y);
    let e = 1.0e-10;
    return vec3<f32>(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}

fn hsv_to_rgb(c: vec3<f32>) -> vec3<f32> {
    let k = vec4<f32>(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    let p = abs(fract(c.xxx + k.xyz) * 6.0 - k.www);
    return c.z * mix(k.xxx, clamp(p - k.xxx, vec3<f32>(0.0), vec3<f32>(1.0)), c.y);
}

// Smooth Kuwahara: four overlapping quadrants around the pixel, each summarised by its mean colour
// and variance; flat quadrants win, so regions flatten into brush-like patches while edges stay.
fn painterly(uv: vec2<f32>, texel: vec2<f32>) -> vec3<f32> {
    let step = max(1.0, look.paint.y / 3.0);
    var acc = vec3<f32>(0.0);
    var wsum = 0.0;
    for (var q = 0; q < 4; q++) {
        let sx = select(-1.0, 1.0, (q & 1) == 1);
        let sy = select(-1.0, 1.0, (q & 2) == 2);
        var m = vec3<f32>(0.0);
        var s = vec3<f32>(0.0);
        for (var j = 0; j < 4; j++) {
            for (var i = 0; i < 4; i++) {
                let o = vec2<f32>(f32(i) * sx, f32(j) * sy) * step;
                let c = to_gamma(textureSampleLevel(screen_tex, screen_samp, uv + o * texel, 0.0).rgb);
                m += c;
                s += c * c;
            }
        }
        m /= 16.0;
        let v = dot(max(s / 16.0 - m * m, vec3<f32>(0.0)), vec3<f32>(1.0));
        let w = 1.0 / (1.0 + pow(v * 400.0, 4.0));
        acc += m * w;
        wsum += w;
    }
    return acc / max(wsum, 1.0e-6);
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(screen_tex));
    let texel = 1.0 / vec2<f32>(size);
    let px = vec2<i32>(in.position.xy);
    var col = to_gamma(textureLoad(screen_tex, px, 0).rgb);

    let d = textureLoad(depth_tex, px, 0);
    let is_sky = d <= 0.0;
    let ndc = in.uv * 2.0 - 1.0;
    let dir = normalize(look.cam_fwd.xyz + look.cam_right.xyz * (ndc.x * look.cam_right.w) - look.cam_up.xyz * (ndc.y * look.cam_up.w));
    let near = look.misc.x;
    let dist = select(near / max(d, 1.0e-9) / max(dot(dir, look.cam_fwd.xyz), 0.05), 1.0e9, is_sky);

    // painterly brushwork (fades out in the far distance where it would only blur)
    if look.paint.x > 0.001 {
        let p = painterly(in.uv, texel);
        let k = look.paint.x * select(1.0 - 0.6 * smoothstep(400.0, 3000.0, dist), 0.7, is_sky);
        col = mix(col, p, clamp(k, 0.0, 1.0));
    }

    // ink outlines: Laplacian of the (reversed, 1/z) depth is zero on any plane, so only
    // silhouettes and convex creases ink, not the terrain's flat triangles
    if look.paint.z > 0.001 && !is_sky {
        let o = max(1, i32(round(look.cam_fwd.w)));
        let mx = size - vec2<i32>(1);
        let dl = textureLoad(depth_tex, clamp(px - vec2<i32>(o, 0), vec2<i32>(0), mx), 0);
        let dr = textureLoad(depth_tex, clamp(px + vec2<i32>(o, 0), vec2<i32>(0), mx), 0);
        let du = textureLoad(depth_tex, clamp(px - vec2<i32>(0, o), vec2<i32>(0), mx), 0);
        let dd = textureLoad(depth_tex, clamp(px + vec2<i32>(0, o), vec2<i32>(0), mx), 0);
        let lap = (4.0 * d - (dl + dr + du + dd)) / max(d, 1.0e-9);
        let fade = 1.0 - smoothstep(120.0, 1200.0, dist);
        let e = smoothstep(0.08, 0.35, lap) * fade;
        col = mix(col, col * vec3<f32>(0.2, 0.17, 0.22), clamp(e * look.paint.z * 1.4, 0.0, 0.9));
    }

    // colour haze and sky tint (the haze meets the sky at the horizon without a seam)
    let haze_g = to_gamma(look.haze.rgb);
    if is_sky {
        if look.sky.w > 0.001 {
            let sky_g = to_gamma(look.sky.rgb);
            let l = dot(col, LUMA);
            let tinted = sky_g * (l / max(dot(sky_g, LUMA), 1.0e-3));
            col = mix(col, tinted, look.sky.w * (0.4 + 0.6 * smoothstep(-0.05, 0.5, dir.y)));
        }
        let horizon = 1.0 - smoothstep(0.0, 0.3, dir.y);
        col = mix(col, haze_g, look.haze.w * horizon * 0.85);
    } else if look.haze.w > 0.001 {
        let h = look.haze.w * (1.0 - exp(-max(dist - look.extra.x, 0.0) / look.misc.w));
        col = mix(col, haze_g, clamp(h, 0.0, 0.92));
    }

    // contrast around display mid-grey
    col = max(vec3<f32>(0.45) + (col - vec3<f32>(0.45)) * look.extra.y, vec3<f32>(0.0));

    // split toning + lift
    let l = dot(col, LUMA);
    let ws = 1.0 - smoothstep(0.0, 0.5, l);
    let wh = smoothstep(0.5, 1.0, l);
    col *= mix(vec3<f32>(1.0), look.shadow.rgb, ws) * mix(vec3<f32>(1.0), look.highlight.rgb, wh);
    col = col * (1.0 - look.shadow.w) + vec3<f32>(look.shadow.w);

    // foliage hue: rotate greens towards yellow (-) or teal (+)
    if abs(look.misc.z) > 0.001 {
        var hsv = rgb_to_hsv(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)));
        let g = (1.0 - smoothstep(0.05, 0.13, abs(hsv.x - 0.3))) * smoothstep(0.05, 0.2, hsv.y);
        hsv.x = fract(hsv.x + look.misc.z * 0.07 * g);
        col = mix(col, hsv_to_rgb(hsv), g);
    }

    // vibrance: lift muted colours more than saturated ones
    if abs(look.highlight.w) > 0.001 {
        let mxc = max(col.r, max(col.g, col.b));
        let mnc = min(col.r, min(col.g, col.b));
        let sat = mxc - mnc;
        let lv = dot(col, LUMA);
        col = mix(vec3<f32>(lv), col, 1.0 + look.highlight.w * (1.0 - sat));
    }

    // film grain (stronger in the shadows, like film)
    if look.paint.w > 0.0001 {
        let n = hash12(in.position.xy + vec2<f32>(fract(look.misc.y * 7.13) * 917.0, fract(look.misc.y * 3.71) * 491.0)) - 0.5;
        col += vec3<f32>(n * look.paint.w * (0.6 + 0.6 * (1.0 - l)));
    }

    return vec4<f32>(to_linear(col), 1.0);
}
