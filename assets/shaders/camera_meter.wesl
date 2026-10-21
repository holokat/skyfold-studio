// Camera light meter (camera_fx/meter.rs): one 16 × 16 workgroup reads a 64 × 64 grid of the HDR
// view (each point the mean of a 2 × 2 block) into a centre-weighted histogram of scene luminance
// (log2 cd/m²: the frame's exposure taken out) and the depth round the centre of the view, then
// writes the histogram's trimmed mean (10 … 90 %) and the median distance at the centre.

struct CameraMeter {
    // x: added to a pixel's log2 brightness gives log2 of its scene luminance (ev100 + log2 1.2),
    // y: near plane (m), z: view aspect (width / height), w: unused
    params: vec4<f32>,
}

@group(0) @binding(0) var hdr_tex: texture_2d<f32>;
@group(0) @binding(1) var depth_tex: texture_depth_2d;
@group(0) @binding(2) var<uniform> cm: CameraMeter;
// [scene log2 luminance, centre distance (median), centre distance (nearest), weight, 4 spare]
@group(0) @binding(3) var<storage, read_write> result: array<f32, 8>;

const GRID: u32 = 64u;
const BINS: u32 = 64u;
// the histogram spans log2 luminance -12 … +20 (a moonless night … the sun's disc), half a stop a bin
const MIN_LOG: f32 = -12.0;
const BIN_STOPS: f32 = 0.5;
// sub-bin positions are summed in 64ths of a bin (the mean isn't stuck to bin centres)
const SUB: f32 = 64.0;
const FOCUS_N: u32 = 25u;
// open sky reads as this far (m)
const FAR: f32 = 100000.0;
const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

var<workgroup> counts: array<atomic<u32>, 64>;
var<workgroup> sums: array<atomic<u32>, 64>;
var<workgroup> depths: array<f32, 25>;

@compute @workgroup_size(16, 16, 1)
fn meter(@builtin(local_invocation_index) li: u32, @builtin(local_invocation_id) lid: vec3<u32>) {
    if li < BINS {
        atomicStore(&counts[li], 0u);
        atomicStore(&sums[li], 0u);
    }
    workgroupBarrier();

    let dims = vec2<i32>(textureDimensions(hdr_tex));
    let aspect = cm.params.z;
    // each invocation reads a 4 × 4 block of the grid
    for (var k = 0u; k < 16u; k++) {
        let g = lid.xy * 4u + vec2<u32>(k % 4u, k / 4u);
        let uv = (vec2<f32>(g) + 0.5) / f32(GRID);
        let p = clamp(vec2<i32>(uv * vec2<f32>(dims)), vec2<i32>(0), max(dims - vec2<i32>(2), vec2<i32>(0)));
        let c = (textureLoad(hdr_tex, p, 0).rgb + textureLoad(hdr_tex, p + vec2<i32>(1, 0), 0).rgb + textureLoad(hdr_tex, p + vec2<i32>(0, 1), 0).rgb
            + textureLoad(hdr_tex, p + vec2<i32>(1, 1), 0).rgb) * 0.25;
        let lum = max(dot(max(c, vec3<f32>(0.0)), LUMA), 1.0e-9);
        let x = clamp((log2(lum) + cm.params.x - MIN_LOG) / BIN_STOPS, 0.0, f32(BINS) - 0.001);
        let bin = u32(x);
        // centre-weighted (4 at the corners … 16 in the middle): eyes adapt to what they look at
        let d = (uv - vec2<f32>(0.5)) * vec2<f32>(aspect, 1.0);
        let w = u32(round(4.0 + 12.0 * exp(-dot(d, d) / (2.0 * 0.3 * 0.3))));
        atomicAdd(&counts[bin], w);
        atomicAdd(&sums[bin], w * u32(fract(x) * SUB));
    }

    // what is at the centre of the view (for auto focus): a 5 × 5 patch 6 % of the height across
    if li < FOCUS_N {
        let o = vec2<f32>(f32(li % 5u) - 2.0, f32(li / 5u) - 2.0) * 0.015;
        let uv = vec2<f32>(0.5) + o * vec2<f32>(1.0 / aspect, 1.0);
        let ddims = vec2<i32>(textureDimensions(depth_tex));
        let p = clamp(vec2<i32>(uv * vec2<f32>(ddims)), vec2<i32>(0), ddims - vec2<i32>(1));
        let z = textureLoad(depth_tex, p, 0);
        // reverse-Z with an infinite far plane: depth = near / distance along the view
        depths[li] = select(FAR, min(cm.params.y / z, FAR), z > 0.0);
    }
    workgroupBarrier();

    if li == 0u {
        var total = 0u;
        for (var i = 0u; i < BINS; i++) {
            total += atomicLoad(&counts[i]);
        }
        let lo = f32(total) * 0.1;
        let hi = f32(total) * 0.9;
        var cum = 0.0;
        var acc = 0.0;
        var wsum = 0.0;
        for (var i = 0u; i < BINS; i++) {
            let n = f32(atomicLoad(&counts[i]));
            if n > 0.0 {
                let a = max(cum, lo);
                let b = min(cum + n, hi);
                if b > a {
                    let within = (f32(atomicLoad(&sums[i])) / n + 0.5) / SUB;
                    acc += (b - a) * (f32(i) + within);
                    wsum += b - a;
                }
                cum += n;
            }
        }
        result[0] = MIN_LOG + acc / max(wsum, 1.0e-6) * BIN_STOPS;

        // median and nearest of the centre patch (insertion sort of 25)
        var s: array<f32, 25>;
        for (var i = 0u; i < FOCUS_N; i++) {
            s[i] = depths[i];
        }
        for (var i = 1u; i < FOCUS_N; i++) {
            let v = s[i];
            var j = i;
            while j > 0u && s[j - 1u] > v {
                s[j] = s[j - 1u];
                j -= 1u;
            }
            s[j] = v;
        }
        result[1] = s[12];
        result[2] = s[0];
        result[3] = f32(total);
    }
}
