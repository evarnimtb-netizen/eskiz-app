// Tile compositor.
//
// One dispatch processes a list of 256x256 canvas tiles ("slots" of a scratch accumulator).
// The accumulator holds the premultiplied backdrop in f32, so there is no 8-bit
// quantisation between layers. Layer tiles are premultiplied RGBA8 (Unorm) pages.
//
// Mode numbering is the position in `eskiz_core::BlendMode::ALL`.

struct Params {
    op: u32,
    blend: u32,
    mask_mode: u32,   // 0 = none, 1 = constant `mask_const`, 2 = mask page
    list_offset: u32,
    opacity: f32,
    mask_const: f32,
    canvas_w: u32,
    canvas_h: u32,
    bg: vec4<f32>,    // premultiplied background (alpha 0 when hidden)
    m0: vec4<f32>,    // display matrix rows (xyz), linear light: document space -> display space
    m1: vec4<f32>,
    m2: vec4<f32>,
    xform: u32,       // 1 = apply the display matrix in OP_OUTPUT
    pad0: u32,
    pad1: u32,
    pad2: u32,
}

struct Entry {
    slot: u32,
    layer_slice: u32,
    mask_slice: u32,
    tile: u32,        // tile_x | tile_y << 16
}

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var<storage, read> LIST: array<Entry>;
@group(0) @binding(2) var<storage, read_write> ACC: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> CLIP: array<vec4<f32>>;
@group(0) @binding(4) var LAYER: texture_2d_array<f32>;
@group(0) @binding(5) var MASK: texture_2d_array<f32>;
@group(0) @binding(6) var OUT: texture_storage_2d<rgba8unorm, write>;

const OP_INIT: u32 = 0u;
const OP_DRAW: u32 = 1u;
const OP_CLIP_CLEAR: u32 = 2u;
const OP_CLIP_BASE: u32 = 3u;
const OP_CLIP_APPLY: u32 = 4u;
const OP_CLIP_END: u32 = 5u;
const OP_OUTPUT: u32 = 6u;

// Luminance for Color / Luminosity / Hue / Saturation and ClipColor.
const LUM_W: vec3<f32> = vec3<f32>(0.30005, 0.58984, 0.10998);
// Luma for Lighter Color / Darker Color.
const LUMA_W: vec3<f32> = vec3<f32>(0.29907, 0.58691, 0.11401);

fn lum(c: vec3<f32>) -> f32 { return dot(c, LUM_W); }
fn luma(c: vec3<f32>) -> f32 { return dot(c, LUMA_W); }

fn b_screen(cb: f32, cs: f32) -> f32 { return cb + cs - cb * cs; }

fn b_hard_light(cb: f32, cs: f32) -> f32 {
    if (cs <= 0.5) { return cb * (2.0 * cs); }
    return b_screen(cb, 2.0 * cs - 1.0);
}

// Zero-divisor floor 2^-14: keeps dodge, burn and divide continuous.
const DIV_FLOOR: f32 = 0.00006103515625;

fn b_dodge(cb: f32, cs: f32) -> f32 {
    return min(cb / max(1.0 - cs, DIV_FLOOR), 1.0);
}

fn b_burn(cb: f32, cs: f32) -> f32 {
    return 1.0 - min((1.0 - cb) / max(cs, DIV_FLOOR), 1.0);
}

// Pegtop soft light, not the W3C form.
fn b_soft_light(cb: f32, cs: f32) -> f32 {
    if (cs <= 0.5) { return cb * cb * (1.0 - 2.0 * cs) + 2.0 * cb * cs; }
    return 2.0 * cb * (1.0 - cs) + sqrt(cb) * (2.0 * cs - 1.0);
}

fn sep(mode: u32, cb: f32, cs: f32) -> f32 {
    switch (mode) {
        case 0u: { return cs; }
        case 1u: { return cb * cs; }
        case 2u: { return b_screen(cb, cs); }
        case 3u: { return min(cb + cs, 1.0); }
        case 4u: { return max(cb, cs); }
        case 5u: { return cb + cs - 2.0 * cb * cs; }
        case 6u: { return abs(cb - cs); }
        case 7u: { return max(cb - cs, 0.0); }
        case 8u: { return max(cb + cs - 1.0, 0.0); }
        case 9u: { return b_dodge(cb, cs); }
        case 10u: { return b_burn(cb, cs); }
        case 11u: { return b_hard_light(cs, cb); }
        case 12u: { return b_hard_light(cb, cs); }
        case 17u: { return b_soft_light(cb, cs); }
        case 18u: { return min(cb, cs); }
        case 19u: { if (cb + cs >= 1.0) { return 1.0; } return 0.0; }
        case 20u: {
            if (cs <= 0.5) { return b_burn(cb, 2.0 * cs); }
            return b_dodge(cb, 2.0 * cs - 1.0);
        }
        case 21u: { return clamp(cb + 2.0 * cs - 1.0, 0.0, 1.0); }
        case 22u: {
            if (cs <= 0.5) { return min(cb, 2.0 * cs); }
            return max(cb, 2.0 * cs - 1.0);
        }
        case 25u: { return min(cb / max(cs, DIV_FLOOR), 1.0); }
        default: { return cs; }
    }
}

fn clip_color(c: vec3<f32>) -> vec3<f32> {
    let l = lum(c);
    let n = min(c.r, min(c.g, c.b));
    let x = max(c.r, max(c.g, c.b));
    var o = c;
    if (n < 0.0 && l - n > 1e-12) {
        o = vec3<f32>(l) + (o - vec3<f32>(l)) * l / (l - n);
    }
    if (x > 1.0 && x - l > 1e-12) {
        o = vec3<f32>(l) + (o - vec3<f32>(l)) * (1.0 - l) / (x - l);
    }
    return o;
}

fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> {
    let d = l - lum(c);
    return clip_color(c + vec3<f32>(d));
}

fn sat_of(c: vec3<f32>) -> f32 {
    return max(c.r, max(c.g, c.b)) - min(c.r, min(c.g, c.b));
}

fn set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    let mx = max(c.r, max(c.g, c.b));
    let mn = min(c.r, min(c.g, c.b));
    if (mx > mn) { return (c - vec3<f32>(mn)) * s / (mx - mn); }
    return vec3<f32>(0.0);
}

// B(Cb, Cs) on straight colours in [0, 1].
fn blend_fn(mode: u32, cb: vec3<f32>, cs: vec3<f32>) -> vec3<f32> {
    switch (mode) {
        case 13u: { return set_lum(cs, lum(cb)); }                       // Color
        case 14u: { return set_lum(cb, lum(cs)); }                       // Luminosity
        case 15u: { return set_lum(set_sat(cs, sat_of(cb)), lum(cb)); }  // Hue
        case 16u: { return set_lum(set_sat(cb, sat_of(cs)), lum(cb)); }  // Saturation
        case 23u: {                                                      // Lighter Color
            if (luma(cs) >= luma(cb)) { return cs; }
            return cb;
        }
        case 24u: {                                                      // Darker Color
            if (luma(cs) <= luma(cb)) { return cs; }
            return cb;
        }
        default: {
            return vec3<f32>(sep(mode, cb.r, cs.r), sep(mode, cb.g, cs.g), sep(mode, cb.b, cs.b));
        }
    }
}

// Blended source-over then opacity mix. `d` is the premultiplied backdrop
// colour, `ab` its alpha, `cs` the straight source colour, `as_` its alpha, `o` the
// effective opacity (layer opacity x mask).
fn composite_px(acc: vec4<f32>, cs: vec3<f32>, as_: f32, o: f32, mode: u32) -> vec4<f32> {
    let d = acc.rgb;
    let ab = acc.a;
    var cb = vec3<f32>(0.0);
    if (ab > 0.0) { cb = clamp(d / ab, vec3<f32>(0.0), vec3<f32>(1.0)); }
    var b = cs;
    if (mode != 0u) { b = blend_fn(mode, cb, cs); }
    let co = as_ * ((1.0 - ab) * cs + ab * b) + (1.0 - as_) * d;
    let ao = as_ + (1.0 - as_) * ab;
    let out_rgb = d + o * (co - d);
    let out_a = ab + o * (ao - ab);
    return clamp(vec4<f32>(out_rgb, out_a), vec4<f32>(0.0), vec4<f32>(1.0));
}

// sRGB transfer curve (IEC 61966-2-1), also used by Display P3.
fn srgb_decode(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

fn srgb_encode(l: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(l, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055), l * 12.92, l <= vec3<f32>(0.0031308));
}

// Document space -> display space for one premultiplied pixel; gamut-clipped per channel.
fn to_display(px: vec4<f32>) -> vec4<f32> {
    if (P.xform == 0u || px.a <= 0.0) { return px; }
    let lin = srgb_decode(clamp(px.rgb / px.a, vec3<f32>(0.0), vec3<f32>(1.0)));
    let out = vec3<f32>(dot(P.m0.xyz, lin), dot(P.m1.xyz, lin), dot(P.m2.xyz, lin));
    let enc = srgb_encode(clamp(out, vec3<f32>(0.0), vec3<f32>(1.0)));
    return vec4<f32>(enc * px.a, px.a);
}

fn mask_value(e: Entry, coord: vec2<i32>) -> f32 {
    if (P.mask_mode == 0u) { return 1.0; }
    if (P.mask_mode == 1u) { return P.mask_const; }
    return textureLoad(MASK, coord, i32(e.mask_slice), 0).r;
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= 256u || gid.y >= 256u) { return; }
    let e = LIST[P.list_offset + gid.z];
    let local = vec2<i32>(i32(gid.x), i32(gid.y));
    let idx = e.slot * 65536u + gid.y * 256u + gid.x;

    switch (P.op) {
        case 0u: { // OP_INIT
            ACC[idx] = P.bg;
        }
        case 1u: { // OP_DRAW
            let px = textureLoad(LAYER, local, i32(e.layer_slice), 0);
            let as_ = px.a;
            if (as_ <= 0.0) { return; }
            let cs = clamp(px.rgb / as_, vec3<f32>(0.0), vec3<f32>(1.0));
            let o = clamp(P.opacity, 0.0, 1.0) * mask_value(e, local);
            ACC[idx] = composite_px(ACC[idx], cs, as_, o, P.blend);
        }
        case 2u: { // OP_CLIP_CLEAR
            CLIP[idx] = vec4<f32>(0.0);
        }
        case 3u: { // OP_CLIP_BASE: straight colour and alpha x mask of the base
            let px = textureLoad(LAYER, local, i32(e.layer_slice), 0);
            if (px.a <= 0.0) { return; }
            let cs = clamp(px.rgb / px.a, vec3<f32>(0.0), vec3<f32>(1.0));
            CLIP[idx] = vec4<f32>(cs, px.a * mask_value(e, local));
        }
        case 4u: { // OP_CLIP_APPLY: group alpha is unchanged
            let g = CLIP[idx];
            if (g.a <= 0.0) { return; }
            let px = textureLoad(LAYER, local, i32(e.layer_slice), 0);
            if (px.a <= 0.0) { return; }
            let cs = clamp(px.rgb / px.a, vec3<f32>(0.0), vec3<f32>(1.0));
            let w = clamp(P.opacity, 0.0, 1.0) * mask_value(e, local) * px.a;
            let b = blend_fn(P.blend, g.rgb, cs);
            CLIP[idx] = vec4<f32>((1.0 - w) * g.rgb + w * b, g.a);
        }
        case 5u: { // OP_CLIP_END: composite the clip group with the base's blend and opacity
            let g = CLIP[idx];
            if (g.a <= 0.0) { return; }
            let o = clamp(P.opacity, 0.0, 1.0);
            ACC[idx] = composite_px(ACC[idx], g.rgb, g.a, o, P.blend);
        }
        default: { // OP_OUTPUT
            let tx = e.tile & 0xffffu;
            let ty = e.tile >> 16u;
            let x = tx * 256u + gid.x;
            let y = ty * 256u + gid.y;
            if (x < P.canvas_w && y < P.canvas_h) {
                let a = to_display(clamp(ACC[idx], vec4<f32>(0.0), vec4<f32>(1.0)));
                textureStore(OUT, vec2<i32>(i32(x), i32(y)), a);
            }
        }
    }
}
