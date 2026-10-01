struct VertexInput {
    @builtin(vertex_index) vertex_index: u32,
    @location(0) top_left: vec3<f32>,
    @location(1) bottom_right: vec2<f32>,
    @location(2) tex_top_left: vec2<f32>,
    @location(3) tex_bottom_right: vec2<f32>,
    @location(4) color: vec4<f32>,
    @location(5) ramp: vec2<f32>,
    @location(6) end_color: vec4<f32>,
    // The glyph's own box before a spread grew the quad: left, top, right,
    // bottom. Only the effect entry points read it.
    @location(7) glyph_rect: vec4<f32>,
    // Pixels the coverage spreads by, and 1 when the spread is a blur.
    @location(8) spread: vec2<f32>,
}

struct Matrix {
    v: mat4x4<f32>,
}

@group(0) @binding(0)
var<uniform> ortho: Matrix;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_pos: vec2<f32>,
    @location(1) color: vec4<f32>,
}

struct Corner {
    pos: vec2<f32>,
    tex: vec2<f32>,
}

// The corner of the glyph quad this vertex is, with its place in the atlas.
fn corner(in: VertexInput) -> Corner {
    var out: Corner;

    var left: f32 = in.top_left.x;
    var right: f32 = in.bottom_right.x;
    var top: f32 = in.top_left.y;
    var bottom: f32 = in.bottom_right.y;

    switch (in.vertex_index) {
        case 0u: {
            out.pos = vec2<f32>(left, top);
            out.tex = in.tex_top_left;
            break;
        }
        case 1u: {
            out.pos = vec2<f32>(right, top);
            out.tex = vec2<f32>(in.tex_bottom_right.x, in.tex_top_left.y);
            break;
        }
        case 2u: {
            out.pos = vec2<f32>(left, bottom);
            out.tex = vec2<f32>(in.tex_top_left.x, in.tex_bottom_right.y);
            break;
        }
        case 3u: {
            out.pos = vec2<f32>(right, bottom);
            out.tex = in.tex_bottom_right;
            break;
        }
        default: {}
    }

    return out;
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let at = corner(in);
    let pos = at.pos;
    out.tex_pos = at.tex;

    out.clip_position = ortho.v * vec4<f32>(pos, in.top_left.z, 1.0);

    // The ramp runs down the section box, so the corner colors interpolate to
    // the same result a per fragment mix would give, for one instruction and
    // no extra value crossing between the stages. Flat text has both colors
    // equal and lands on its own color at every t.
    let span = max(in.ramp.y - in.ramp.x, 0.0001);
    let t = clamp((pos.y - in.ramp.x) / span, 0.0, 1.0);
    out.color = mix(in.color, in.end_color, t);

    return out;
}

@group(0) @binding(1)
var texture: texture_2d<f32>;
@group(0) @binding(2)
var tex_sampler: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var alpha: f32 = textureSample(texture, tex_sampler, in.tex_pos).r;

    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}

// How far the darkening entry point widens every glyph edge, in atlas
// pixels. 0.0 is identity.
override stem_px: f32 = 0.0;

// CoreText applies stem darkening when it rasterizes text, so glyphs on
// macOS carry more ink than the plain outline. Dilating the coverage by
// a fraction of a pixel approximates it: each fragment takes the
// maximum of its own coverage and four neighbor taps stem_px away, so
// every edge moves outward by that fraction, like the platform raster.
// The vertex quads are inflated to give the widened edge room.
@fragment
fn fs_main_darken(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = vec2<f32>(stem_px, stem_px) / vec2<f32>(textureDimensions(texture));
    let dx = vec2<f32>(texel.x, 0.0);
    let dy = vec2<f32>(0.0, texel.y);

    var alpha: f32 = textureSample(texture, tex_sampler, in.tex_pos).r;
    alpha = max(alpha, textureSample(texture, tex_sampler, in.tex_pos + dx).r);
    alpha = max(alpha, textureSample(texture, tex_sampler, in.tex_pos - dx).r);
    alpha = max(alpha, textureSample(texture, tex_sampler, in.tex_pos + dy).r);
    alpha = max(alpha, textureSample(texture, tex_sampler, in.tex_pos - dy).r);

    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}

// An outline or a soft shadow: the coverage of a glyph spread over the
// pixels around it. The quad arrives grown by the spread.
//
// Eight float components go to the fragment stage and not one more. An A7
// GPU draws nothing from a shader that passes nine, see docs/ios.md in
// hilen. So the color and the spread, which are the same on the whole
// quad, travel packed into 2 floats, whole numbers below 2^24, which a
// float holds exactly.
struct EffectOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_pos: vec2<f32>,
    // Pixels from this fragment to the edges of the glyph's own box: to the
    // left and the top edge in xy, to the right and the bottom edge in zw.
    // Negative outside the box.
    @location(1) inside: vec4<f32>,
    // x: red, green and blue, 8 bits each. y: alpha in 8 bits, then the
    // spread in eighths of a pixel in 7 bits, then 1 bit for a blur.
    @location(2) @interpolate(flat) packed: vec2<f32>,
}

@vertex
fn vs_effect(in: VertexInput) -> EffectOutput {
    var out: EffectOutput;

    let at = corner(in);
    out.tex_pos = at.tex;
    out.clip_position = ortho.v * vec4<f32>(at.pos, in.top_left.z, 1.0);
    out.inside = vec4<f32>(at.pos - in.glyph_rect.xy, in.glyph_rect.zw - at.pos);

    let color = vec4<u32>(round(clamp(in.color, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0));
    let reach = u32(round(clamp(in.spread.x, 0.0, 15.875) * 8.0));
    let soft = u32(in.spread.y > 0.5);
    out.packed = vec2<f32>(
        f32(color.r * 65536u + color.g * 256u + color.b),
        f32(color.a + reach * 256u + soft * 32768u),
    );

    return out;
}

@fragment
fn fs_effect(in: EffectOutput) -> @location(0) vec4<f32> {
    let rgb_bits = u32(in.packed.x + 0.5);
    let rest = u32(in.packed.y + 0.5);
    let rgb = vec3<f32>(
        f32((rgb_bits >> 16u) & 255u),
        f32((rgb_bits >> 8u) & 255u),
        f32(rgb_bits & 255u),
    ) / 255.0;
    let alpha = f32(rest & 255u) / 255.0;
    let reach = f32((rest >> 8u) & 127u) / 8.0;
    let soft = (rest >> 15u) == 1u;

    let texel = vec2<f32>(1.0) / vec2<f32>(textureDimensions(texture));
    // The blur is a gaussian whose sigma is half the radius, like the blur
    // of a CSS text shadow, cut off at 2 sigma.
    let sigma = max(reach * 0.5, 0.001);
    let steps = i32(ceil(reach));

    var widest = 0.0;
    var sum = 0.0;
    var weights = 0.0;
    for (var y = -steps; y <= steps; y++) {
        for (var x = -steps; x <= steps; x++) {
            let offset = vec2<f32>(f32(x), f32(y));
            let distance = length(offset);

            // A sample outside the glyph's own box would read the glyph
            // packed next to it in the atlas.
            let edges = vec4<f32>(in.inside.xy + offset, in.inside.zw - offset);
            var coverage = 0.0;
            if all(edges >= vec4<f32>(0.0)) {
                coverage = textureSampleLevel(texture, tex_sampler, in.tex_pos + offset * texel, 0.0).r;
            }

            // The widening is the strongest coverage within the reach, with
            // the rim of the disc faded over a pixel so the edge is smooth.
            widest = max(widest, coverage * clamp(reach + 0.5 - distance, 0.0, 1.0));

            let weight = exp(-distance * distance / (2.0 * sigma * sigma));
            sum += coverage * weight;
            weights += weight;
        }
    }

    let spread = select(widest, sum / weights, soft);
    return vec4<f32>(rgb, alpha * spread);
}
