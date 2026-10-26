// Alien flora (`nature/alien.rs`): turns the green of a plant's colour to another hue, as much
// as the colour is green (bark, straw, rock and flowers keep theirs), with about its own value
// and a scaled saturation, the leaves' hue spread kept round the target.
// `p`: x the mode (its whole part) plus the target hue in turns, y the saturation in hundredths
// plus the amount as its fraction; (0, 0) is off and returns the colour unchanged.
// Modes: 0 recolour, 1 charred (burnt grey-black, bark blackened too), 2 frosted (pale icy, a
// glassy sheen: `alien_surface`), 3 glow (recoloured, glowing softly in the hue: `alien_surface`),
// 4 petrified (stone grey-ochre, the whole plant).

fn alien_hsv(h: f32, s: f32, v: f32) -> vec3<f32> {
    let k = clamp(abs(fract(vec3<f32>(h) + vec3<f32>(0.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0) - 1.0, vec3<f32>(0.0), vec3<f32>(1.0));
    return v * (1.0 - s + s * k);
}

fn alien_mode(p: vec2<f32>) -> u32 {
    return u32(clamp(floor(p.x), 0.0, 4.0));
}

// How much of a (gamma) colour turns: its greenness, and for the modes that change the whole
// plant (charred, frosted, petrified) part of the rest too.
fn alien_weight(s: vec3<f32>, p: vec2<f32>) -> f32 {
    let amount = fract(p.y);
    let mx = max(s.r, max(s.g, s.b));
    let mn = min(s.r, min(s.g, s.b));
    let chroma = mx - mn;
    if amount <= 0.0 || mx <= 1e-4 {
        return 0.0;
    }
    var green = 0.0;
    if chroma > 1e-4 {
        let lead = s.g - max(s.r, s.b) * 0.92;
        green = smoothstep(0.0, 0.35, lead / chroma) * smoothstep(0.005, 0.03, chroma);
    }
    var whole = array<f32, 5>(0.0, 0.6, 0.4, 0.0, 0.85);
    return amount * max(green, whole[alien_mode(p)]);
}

// `c`: linear RGB.
fn alien_tint(c: vec3<f32>, p: vec2<f32>) -> vec3<f32> {
    let amount = fract(p.y);
    if amount <= 0.0 {
        return c;
    }
    let sat = floor(p.y) / 100.0;
    let mode = alien_mode(p);
    let s = pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    let mx = max(s.r, max(s.g, s.b));
    let mn = min(s.r, min(s.g, s.b));
    let chroma = mx - mn;
    let k = alien_weight(s, p);
    if k <= 0.0 {
        return c;
    }
    let base_hue = fract(p.x);
    var t = vec3<f32>(0.0);
    if mode == 1u {
        // charred: burnt to a dull grey-black, a little of the hue left in it
        t = alien_hsv(base_hue, clamp(0.15 * sat, 0.0, 1.0), mx * 0.3 + 0.02);
    } else if mode == 2u {
        // frosted: pale, icy, the texture's light and dark kept faintly
        t = alien_hsv(base_hue, clamp(0.22 * sat, 0.0, 1.0), min(0.6 + mx * 0.45, 0.96));
    } else if mode == 4u {
        // petrified: stone, its grain still in it
        t = alien_hsv(base_hue, clamp(0.1 * sat, 0.0, 1.0), min(mx * 0.55 + 0.3, 0.8));
    } else {
        // the source hue (green is the largest channel wherever this matters)
        var h = 0.0;
        if mx == s.r {
            h = ((s.g - s.b) / chroma + 6.0) % 6.0;
        } else if mx == s.g {
            h = (s.b - s.r) / chroma + 2.0;
        } else {
            h = (s.r - s.g) / chroma + 4.0;
        }
        let hue = fract(p.x + (h / 6.0 - 0.27) * 0.5 + 1.0);
        // dark, dull needles still turn a clear colour; red reads darker than green at the same
        // value, so it is lifted a little
        t = alien_hsv(hue, clamp(max(chroma / mx, 0.4) * sat, 0.0, 1.0), min(mx * 1.15, 1.0));
    }
    return pow(mix(s, t, k), vec3<f32>(2.2));
}

// The surface a mode gives (`c`: the colour before `alien_tint`, linear): rgb the glow to add
// (display terms: as bright by night as by day, so glowing leaves read in the dark), w the
// roughness to move towards (negative: keep the material's).
fn alien_surface(c: vec3<f32>, p: vec2<f32>) -> vec4<f32> {
    if fract(p.y) <= 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, -1.0);
    }
    let mode = alien_mode(p);
    let s = pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    let k = alien_weight(s, p);
    if k <= 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, -1.0);
    }
    if mode == 3u {
        // glow: the hue, brighter where the leaf's texture is light
        let mx = max(s.r, max(s.g, s.b));
        let g = pow(alien_hsv(fract(p.x), 0.8, 1.0), vec3<f32>(2.2)) * (0.015 + 0.3 * mx * mx);
        return vec4<f32>(g * k, -1.0);
    }
    var r = array<f32, 5>(-1.0, 0.92, 0.22, -1.0, 0.96);
    return vec4<f32>(0.0, 0.0, 0.0, r[mode]);
}
