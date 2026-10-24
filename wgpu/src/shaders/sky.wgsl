// The sky (sky_v.cg, sky_f.cg): a dome around the eye under the hour's panorama of the reference's sky (by
// longitude and latitude, as the export drew it, graded as the display shows it), and the sun's disc.

struct Sky {
    // The view and projection with the eye at the origin, in rows.
    mvp: mat4x4<f32>,
    // xyz: towards the sun (or the moon); and its colour as the display shows it.
    sun: array<vec4<f32>, 2>,
    // x: which of the hours' panoramas.
    hour: vec4<f32>,
}

@group(0) @binding(0) var<uniform> sky: Sky;
@group(0) @binding(1) var panorama: texture_2d_array<f32>;
@group(0) @binding(2) var texels: sampler;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) dir: vec3<f32>,
    // the direction crossed with the sun's (linear in the direction, so exact across a triangle); w: on its side
    @location(2) off: vec4<f32>,
}

@vertex
fn vs_sky(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>) -> Out {
    let p = vec4<f32>(position, 1.0) * sky.mvp;
    var o: Out;
    // At the far plane, whatever the dome's radius (depth 0 is the farthest: the scene's depth runs from 1 near).
    o.position = vec4<f32>(p.xy, 0.0, p.w);
    o.uv = uv;
    o.dir = position;
    o.off = vec4<f32>(cross(position, sky.sun[0].xyz), dot(position, sky.sun[0].xyz));
    return o;
}

@fragment
fn fs_sky(i: Out) -> @location(0) vec4<f32> {
    var c = textureSample(panorama, texels, i.uv, i32(sky.hour.x)).rgb;
    // How far from the sun, as the square of the angle's sine (exact near the sun, where a cosine is not), on its
    // side of the sky only: a disc of some 0.6°, and a glow round it.
    let s2 = dot(i.off.xyz, i.off.xyz) / dot(i.dir, i.dir);
    let disc = saturate((1.2e-4 - s2) * 4.0e4);
    let glow = exp(-s2 * 400.0) * 0.3;
    c += sky.sun[1].rgb * 0.5 * ((disc + glow) * 2.0 * step(0.0, i.off.w));
    return vec4<f32>(c, 1.0);
}
