// Adjustment shaders. Each one mirrors the CPU reference in `eskiz-filters`, on premultiplied pixels.
//
// Stages (one dispatch each, one thread per canvas pixel):
//   gauss_v : SRC -> DST            vertical pass, clamp-to-edge
//   gauss_h : IN  -> DST            horizontal pass
//   adjust  : SRC -> DST            point operations and sharpen (P.dims.w selects)
//   finish  : SRC, IN, MASK -> OUT  out = mix(src, adjusted, mask * scale)
//
// SRC is the layer as RGBA8 (so a texel is byte / 255, the CPU `from_rgba8_premul` rule).

struct Params {
    dims: vec4<u32>,   // width, height, has_mask, kind
    iu: vec4<u32>,     // kind specific integers
    f0: vec4<f32>,
    f1: vec4<f32>,
    f2: vec4<f32>,
    f3: vec4<f32>,     // f3.w: mix scale applied to the mask in `finish`
}

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var SRC: texture_2d<f32>;
@group(0) @binding(2) var MASK: texture_2d<f32>;
@group(0) @binding(3) var IN: texture_2d<f32>;
@group(0) @binding(4) var DST: texture_storage_2d<rgba16float, write>;
@group(0) @binding(5) var OUT: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(6) var<storage, read> LUT: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read> PERM: array<u32>;

const KIND_HSB: u32 = 1u;
const KIND_OPACITY: u32 = 2u;
const KIND_CURVES: u32 = 3u;
const KIND_GRADIENT_MAP: u32 = 4u;
const KIND_SHARPEN: u32 = 5u;
const KIND_NOISE: u32 = 6u;

const LUMA_W: vec3<f32> = vec3<f32>(0.299, 0.586914, 0.114014);

fn sat1(x: f32) -> f32 { return clamp(x, 0.0, 1.0); }
fn sat3(x: vec3<f32>) -> vec3<f32> { return clamp(x, vec3<f32>(0.0), vec3<f32>(1.0)); }

fn unpremul(px: vec4<f32>) -> vec3<f32> {
    if (px.a <= 0.0) { return vec3<f32>(0.0); }
    return px.rgb / px.a;
}

fn premul(rgb: vec3<f32>, a: f32) -> vec4<f32> {
    return vec4<f32>(rgb * a, a);
}

fn lut_f(i: u32) -> f32 {
    let v = LUT[i >> 2u];
    switch (i & 3u) {
        case 0u: { return v.x; }
        case 1u: { return v.y; }
        case 2u: { return v.z; }
        default: { return v.w; }
    }
}

fn src_px(x: i32, y: i32) -> vec4<f32> {
    return textureLoad(SRC, vec2<i32>(x, y), 0);
}

fn src_clamped(x: i32, y: i32) -> vec4<f32> {
    let cx = clamp(x, 0, i32(P.dims.x) - 1);
    let cy = clamp(y, 0, i32(P.dims.y) - 1);
    return textureLoad(SRC, vec2<i32>(cx, cy), 0);
}

fn in_clamped(x: i32, y: i32) -> vec4<f32> {
    let cx = clamp(x, 0, i32(P.dims.x) - 1);
    let cy = clamp(y, 0, i32(P.dims.y) - 1);
    return textureLoad(IN, vec2<i32>(cx, cy), 0);
}

// ---------------------------------------------------------------------------------------
// Gaussian: weights w_0 .. w_{N-1} in LUT[0..N), N = P.iu.x. Vertical pass first.
// ---------------------------------------------------------------------------------------

@compute @workgroup_size(8, 8, 1)
fn gauss_v(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= P.dims.x || gid.y >= P.dims.y) { return; }
    let x = i32(gid.x);
    let y = i32(gid.y);
    var acc = src_px(x, y) * lut_f(0u);
    for (var i = 1u; i < P.iu.x; i++) {
        let d = i32(i);
        let a = src_clamped(x, y + d);
        let b = src_clamped(x, y - d);
        acc += lut_f(i) * (a + b);
    }
    textureStore(DST, vec2<i32>(x, y), acc);
}

@compute @workgroup_size(8, 8, 1)
fn gauss_h(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= P.dims.x || gid.y >= P.dims.y) { return; }
    let x = i32(gid.x);
    let y = i32(gid.y);
    var acc = textureLoad(IN, vec2<i32>(x, y), 0) * lut_f(0u);
    for (var i = 1u; i < P.iu.x; i++) {
        let d = i32(i);
        let a = in_clamped(x + d, y);
        let b = in_clamped(x - d, y);
        acc += lut_f(i) * (a + b);
    }
    textureStore(DST, vec2<i32>(x, y), acc);
}

// ---------------------------------------------------------------------------------------
// HSB. f0 = (cos theta, sin theta, 1 + saturation, brightness).
// ---------------------------------------------------------------------------------------

fn hsb(c: vec3<f32>) -> vec3<f32> {
    let cos_t = P.f0.x;
    let sin_t = P.f0.y;
    let ms = P.f0.z;
    let bri = P.f0.w;
    let y = dot(c, LUMA_W);
    let u = (c.b - y) * 0.564941;
    let v = (c.r - y) * 0.712891;
    let y2 = y + bri;
    let u2 = ms * (cos_t * u - sin_t * v);
    let v2 = ms * (sin_t * u + cos_t * v);
    return sat3(vec3<f32>(y2 + 1.403 * v2, y2 - 0.344 * u2 - 0.714 * v2, y2 + 1.770 * u2));
}

// ---------------------------------------------------------------------------------------
// Curves. LUT holds 8 curves of 256 floats: target master, r, g, b, then the
// blend source master, r, g, b. iu.y = has blend source, f0.x = blend.
// ---------------------------------------------------------------------------------------

fn curve_lookup(curve: u32, c: f32) -> f32 {
    let u = clamp(c, 0.0, 1.0) + 0.5 / 256.0;
    let pos = clamp(u * 256.0 - 0.5, 0.0, 255.0);
    let i0f = floor(pos);
    let i0 = u32(i0f);
    let i1 = min(i0 + 1u, 255u);
    let t = pos - i0f;
    let a = lut_f(curve * 256u + i0);
    let b = lut_f(curve * 256u + i1);
    return a + (b - a) * t;
}

fn curve_channel(cset: u32, k: u32, c: f32) -> f32 {
    let m = curve_lookup(cset * 4u, c);
    return curve_lookup(cset * 4u + 1u + k, m);
}

fn curves(c: vec3<f32>) -> vec3<f32> {
    let base = vec3<f32>(curve_channel(0u, 0u, c.r), curve_channel(0u, 1u, c.g), curve_channel(0u, 2u, c.b));
    let blend = clamp(P.f0.x, 0.0, 1.0);
    if (P.iu.y != 0u && blend != 1.0) {
        let alt = vec3<f32>(curve_channel(1u, 0u, c.r), curve_channel(1u, 1u, c.g), curve_channel(1u, 2u, c.b));
        return sqrt(max(alt * alt * (1.0 - blend) + base * base * blend, vec3<f32>(0.0)));
    }
    return base;
}

// ---------------------------------------------------------------------------------------
// Gradient map. LUT[0 .. n0) is the start table, LUT[n0 .. n0 + n1) the target
// table (rgb in xyz). iu.y = n0, iu.z = n1. f0.x = blend.
// ---------------------------------------------------------------------------------------

fn table_lookup(offset: u32, n: u32, y: f32) -> vec3<f32> {
    let last = n - 1u;
    let pos = clamp(y, 0.0, 1.0) * f32(last);
    var i0 = u32(floor(pos));
    if (last >= 1u) { i0 = min(i0, last - 1u); } else { i0 = 0u; }
    let i1 = min(i0 + 1u, last);
    let t = pos - f32(i0);
    let a = LUT[offset + i0].xyz;
    let b = LUT[offset + i1].xyz;
    return a + (b - a) * t;
}

fn gradient_map(c: vec3<f32>) -> vec3<f32> {
    let y = dot(c, LUMA_W);
    let a = table_lookup(0u, P.iu.y, y);
    let b = table_lookup(P.iu.y, P.iu.z, y);
    let blend = clamp(P.f0.x, 0.0, 1.0);
    return sqrt(max(a * a * (1.0 - blend) + b * b * blend, vec3<f32>(0.0)));
}

// ---------------------------------------------------------------------------------------
// Noise. iu = (octaves, combine (0 add, 1 mix), kind (0 signed, 1 abs, 2 billowy),
// multichannel). f0 = (sample scale, amount, turbulence, norm lo), f1.x = norm hi.
// PERM holds the 512-entry permutation table.
// ---------------------------------------------------------------------------------------

const F2: f32 = 0.3660254;
const G2: f32 = 0.2113249;
const SIMPLEX_SCALE: f32 = 45.4545;

fn gradient(gi: u32) -> vec2<f32> {
    switch (gi) {
        case 0u: { return vec2<f32>(1.0, 2.0); }
        case 1u: { return vec2<f32>(-1.0, 2.0); }
        case 2u: { return vec2<f32>(1.0, -2.0); }
        case 3u: { return vec2<f32>(-1.0, -2.0); }
        case 4u: { return vec2<f32>(2.0, 1.0); }
        case 5u: { return vec2<f32>(-2.0, 1.0); }
        case 6u: { return vec2<f32>(2.0, -1.0); }
        default: { return vec2<f32>(-2.0, -1.0); }
    }
}

fn corner(gi: u32, px: f32, py: f32) -> f32 {
    let t = 0.5 - px * px - py * py;
    if (t < 0.0) { return 0.0; }
    let t2 = t * t;
    let g = gradient(gi);
    return t2 * t2 * (g.x * px + g.y * py);
}

fn simplex2d(x: f32, y: f32) -> f32 {
    let s = (x + y) * F2;
    let i = floor(x + s);
    let j = floor(y + s);
    let t = (i + j) * G2;
    let x0 = x - (i - t);
    let y0 = y - (j - t);
    var i1 = 0u;
    var j1 = 1u;
    if (x0 > y0) { i1 = 1u; j1 = 0u; }
    let x1 = x0 - f32(i1) + G2;
    let y1 = y0 - f32(j1) + G2;
    let x2 = x0 - 1.0 + 2.0 * G2;
    let y2 = y0 - 1.0 + 2.0 * G2;
    let ii = u32(((i32(i) % 256) + 256) % 256);
    let jj = u32(((i32(j) % 256) + 256) % 256);
    let gi0 = PERM[ii + PERM[jj]] % 8u;
    let gi1 = PERM[ii + i1 + PERM[jj + j1]] % 8u;
    let gi2 = PERM[ii + 1u + PERM[jj + 1u]] % 8u;
    return SIMPLEX_SCALE * (corner(gi0, x0, y0) + corner(gi1, x1, y1) + corner(gi2, x2, y2));
}

fn fbm(x: f32, y: f32, octaves: u32, kind: u32) -> f32 {
    var sum = 0.0;
    var freq = 1.0;
    var amp = 1.0;
    for (var o = 0u; o < octaves; o++) {
        let n = simplex2d(x * freq, y * freq);
        var c = n;
        if (kind == 1u) { c = abs(n); }
        if (kind == 2u) {
            let b = 1.0 - abs(n);
            c = b * b;
        }
        sum += amp * c;
        freq *= 2.0;
        amp *= 0.5;
    }
    return sum;
}

fn warp(p: vec2<f32>, turbulence: f32) -> vec2<f32> {
    let t = clamp(turbulence, 0.0, 1.0);
    if (t <= 0.0) { return p; }
    let q = vec2<f32>(2.0 * p.x, 2.0 * p.y);
    let wx = simplex2d(q.x + 142.421, q.y + 2134.32);
    let wy = simplex2d(q.x + -7.213, q.y + 142.47);
    let s = 0.32 * t;
    return vec2<f32>(p.x + s * wx, p.y + s * wy);
}

fn normalised_noise(p: vec2<f32>, origin: vec2<f32>) -> f32 {
    let q = warp(vec2<f32>(p.x + origin.x, p.y + origin.y), P.f0.z);
    let raw = fbm(q.x, q.y, P.iu.x, P.iu.z);
    return (raw - P.f0.w) / (P.f1.x - P.f0.w) - 0.5;
}

fn noise(px: vec4<f32>, x: u32, y: u32) -> vec4<f32> {
    let w = f32(P.dims.x);
    let h = f32(P.dims.y);
    let fscale = P.f0.x;
    let amount = P.f0.y;
    let uv = vec2<f32>((f32(x) + 0.5) / w, (f32(y) + 0.5) / h);
    let p = vec2<f32>((uv.x - 0.5) * w * fscale, (uv.y - 0.5) * h * fscale);
    var n = vec3<f32>(0.0);
    if (P.iu.w != 0u) {
        n = vec3<f32>(
            normalised_noise(p, vec2<f32>(0.1254, 1421.2134)),
            normalised_noise(p, vec2<f32>(410.312, 35.132)),
            normalised_noise(p, vec2<f32>(8.4512, 4104.3218)),
        );
    } else {
        n = vec3<f32>(normalised_noise(p, vec2<f32>(0.0)));
    }
    let alpha = px.a;
    let rgb = unpremul(px);
    if (P.iu.y == 0u) {
        return premul(sat3(3.0 * amount * (2.0 * n) + rgb), alpha);
    }
    let k = max(3.0 * amount - 1.0, 0.0) * 5.0 + 1.0;
    let t = sat1(3.0 * amount);
    let shaped = k * n + vec3<f32>(0.5);
    let out_rgb = rgb * (1.0 - t) + shaped * t;
    let out_alpha = alpha * (1.0 - t) + t;
    return premul(out_rgb, out_alpha);
}

// ---------------------------------------------------------------------------------------

@compute @workgroup_size(8, 8, 1)
fn adjust(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= P.dims.x || gid.y >= P.dims.y) { return; }
    let x = i32(gid.x);
    let y = i32(gid.y);
    let px = src_px(x, y);
    var o = px;
    switch (P.dims.w) {
        case 1u: { o = premul(hsb(unpremul(px)), px.a); }
        case 2u: { o = px * clamp(P.f0.x, 0.0, 1.0); }
        case 3u: { o = premul(curves(unpremul(px)), px.a); }
        case 4u: { o = premul(gradient_map(unpremul(px)), px.a); }
        case 5u: {
            let k = clamp(P.f0.x, 0.0, 1.0);
            let c = unpremul(px);
            let l = unpremul(src_clamped(x - 1, y));
            let r = unpremul(src_clamped(x + 1, y));
            let u = unpremul(src_clamped(x, y - 1));
            let d = unpremul(src_clamped(x, y + 1));
            let rgb = sat3(c * (4.0 * k + 1.0) - k * (l + r + u + d));
            o = premul(rgb, px.a);
        }
        case 6u: { o = noise(px, gid.x, gid.y); }
        default: {}
    }
    textureStore(DST, vec2<i32>(x, y), o);
}

@compute @workgroup_size(8, 8, 1)
fn finish(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= P.dims.x || gid.y >= P.dims.y) { return; }
    let c = vec2<i32>(i32(gid.x), i32(gid.y));
    let s = textureLoad(SRC, c, 0);
    let a = textureLoad(IN, c, 0);
    var m = 1.0;
    if (P.dims.z != 0u) { m = clamp(textureLoad(MASK, c, 0).r, 0.0, 1.0); }
    let t = m * P.f3.w;
    textureStore(OUT, c, clamp(s + (a - s) * t, vec4<f32>(0.0), vec4<f32>(1.0)));
}
