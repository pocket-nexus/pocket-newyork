// The glow of what is bright (bright_f.cg, blur_f.cg, glow_f.cg): a quarter-size copy of the frame keeping what is
// bright, blurred across and down, laid over the frame as it goes to the display.

struct Post {
    // bright: the threshold and the slope above it; blur: one texel of the direction it blurs in (xy); the frame:
    // the glow's strength (x).
    params: vec4<f32>,
}

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var glow: texture_2d<f32>;
@group(0) @binding(3) var texels: sampler;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// One triangle over the whole target; texture row 0 at its top.
@vertex
fn vs_post(@builtin(vertex_index) index: u32) -> Out {
    let p = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var o: Out;
    o.position = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}

// Four bilinear taps one source texel off the centre cover a 4 × 4 block; what is under the threshold goes.
@fragment
fn fs_bright(i: Out) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(source));
    var c = textureSample(source, texels, i.uv + vec2<f32>(-t.x, -t.y)).rgb;
    c += textureSample(source, texels, i.uv + vec2<f32>(t.x, -t.y)).rgb;
    c += textureSample(source, texels, i.uv + vec2<f32>(-t.x, t.y)).rgb;
    c += textureSample(source, texels, i.uv + vec2<f32>(t.x, t.y)).rgb;
    c *= 0.25;
    let l = dot(c, vec3<f32>(0.30, 0.59, 0.11));
    let k = saturate((l - post.params.x) * post.params.y);
    return vec4<f32>(c * k, 1.0);
}

// One direction of a Gaussian blur: five bilinear taps stand for nine texels.
@fragment
fn fs_blur(i: Out) -> @location(0) vec4<f32> {
    let d = post.params.xy;
    let c = textureSample(source, texels, i.uv).rgb * 0.2270
        + (textureSample(source, texels, i.uv + d * 1.3846).rgb + textureSample(source, texels, i.uv - d * 1.3846).rgb) * 0.3162
        + (textureSample(source, texels, i.uv + d * 3.2308).rgb + textureSample(source, texels, i.uv - d * 3.2308).rgb) * 0.0703;
    return vec4<f32>(c, 1.0);
}

// The frame, with what is bright in it spread around and laid over it as a screen (glow_f.cg): the glow, plus the
// frame under what is left of it (its least channel), so that it never darkens, lifts the dark round a lamp and adds
// next to nothing where the frame is near white already.
@fragment
fn fs_frame(i: Out) -> @location(0) vec4<f32> {
    let b = saturate(textureSample(glow, texels, i.uv).rgb * post.params.x);
    let c = textureSample(source, texels, i.uv).rgb;
    return vec4<f32>(b + c * (1.0 - min(b.r, min(b.g, b.b))), 1.0);
}
