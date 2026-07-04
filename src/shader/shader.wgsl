struct VertexInput {
    @builtin(vertex_index) vertex_index: u32,
    @location(0) top_left: vec3<f32>,
    @location(1) bottom_right: vec2<f32>,
    @location(2) tex_top_left: vec2<f32>,
    @location(3) tex_bottom_right: vec2<f32>,
    @location(4) color: vec4<f32>,
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

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    var pos: vec2<f32>;
    var left: f32 = in.top_left.x;
    var right: f32 = in.bottom_right.x;
    var top: f32 = in.top_left.y;
    var bottom: f32 = in.bottom_right.y;

    switch (in.vertex_index) {
        case 0u: {
            pos = vec2<f32>(left, top);
            out.tex_pos = in.tex_top_left;
            break;
        }
        case 1u: {
            pos = vec2<f32>(right, top);
            out.tex_pos = vec2<f32>(in.tex_bottom_right.x, in.tex_top_left.y);
            break;
        }
        case 2u: {
            pos = vec2<f32>(left, bottom);
            out.tex_pos = vec2<f32>(in.tex_top_left.x, in.tex_bottom_right.y);
            break;
        }
        case 3u: {
            pos = vec2<f32>(right, bottom);
            out.tex_pos = in.tex_bottom_right;
            break;
        }
        default: {}
    }

    out.clip_position = ortho.v * vec4<f32>(pos, in.top_left.z, 1.0);
    out.color = in.color;
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

fn srgb_encode(c: f32) -> f32 {
    if c <= 0.0031308 {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

fn srgb_decode(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

// Browsers composite text in sRGB space. An sRGB render target blends
// in linear space, which makes dark text on light too thin and light
// text on dark too thick. Exact matching needs the destination pixel,
// which the pass cannot read, so the background is assumed to contrast
// with the glyph luminance. That is exact for the two dominant UI
// cases and close in between. Coverage is remapped so the linear blend
// lands where the sRGB space blend would.
@fragment
fn fs_main_gamma(in: VertexOutput) -> @location(0) vec4<f32> {
    let coverage: f32 = textureSample(texture, tex_sampler, in.tex_pos).r;

    let y = dot(in.color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let fg = srgb_encode(y);
    let bg = 1.0 - fg;
    let fg_lin = srgb_decode(fg);
    let bg_lin = srgb_decode(bg);
    let blend_lin = srgb_decode(mix(bg, fg, coverage));

    var alpha: f32 = coverage;
    if abs(fg_lin - bg_lin) > 0.001 {
        alpha = (blend_lin - bg_lin) / (fg_lin - bg_lin);
    }

    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
