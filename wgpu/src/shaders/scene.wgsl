// The city's programs: the PS Vita's (vita/shaders/*.cg, frame.cg's functions first) in WGSL. The constants
// (UV_RANGE, LIGHT_SCALE, GLOW_RANGE, SHADOW_K, SHADOW_OUT, AROUND, SUN_BIAS) come before this file, from the
// crates the PS Vita's defines come from (`city.rs`).
//
// A vertex is read from a storage buffer by its index (a record of ny-pack's layouts is 20 or 16 bytes, its
// attributes on any byte, which a WebGPU vertex buffer does not take): `draw.src` names the buffer and the word
// its records start at, and the same for the instances. Matrices arrive in rows, as the PS Vita's do: `v * m` is
// `mul(M, v)`.

struct Frame {
    vp: mat4x4<f32>,
    // From world to the sun's map: its x, y (texture coordinates) and depth, to read it; and its clip space, to
    // draw into it.
    sun_read: mat4x4<f32>,
    sun_draw: mat4x4<f32>,
    // uFrame of frame.cg; [0].w is the draw's (Draw::origin.w).
    f: array<vec4<f32>, 6>,
    // uLook[0]: the hour's tone curve.
    tone: vec4<f32>,
    // rgb: the sun over LIGHT_SCALE (linear); w: 1 by night.
    sun: vec4<f32>,
    // x: the lamps' light (uLook[2].x); y: metres of height in one unit of the shadow window (SHADOW_UNIT).
    lamps: vec4<f32>,
    // From world x and z to the lamps' texture coordinates: x scale, x offset, z scale, z offset.
    lamp_map: vec4<f32>,
}

struct Draw {
    // The batch's box: its corner, and w the haze per metre to its middle height (uFrame[0].w).
    origin: vec4<f32>,
    // Its size, and w what its picture is over (uLook[1].a).
    span: vec4<f32>,
    // rgb: the haze's colour towards the draw (uLook[2].yzw); w: how opaque a part seen through is.
    haze: vec4<f32>,
    // xy: the surface's roughness and metalness; z: 1 for a shape seen from both sides (OPEN).
    gloss: vec4<f32>,
    // A picture that repeats over the ground: repeats per metre, and their offset from the draw's corner.
    tile: vec4<f32>,
    // The box a car's pictures were drawn over: its corner, and 1 / its size.
    bake0: vec4<f32>,
    bake1: vec4<f32>,
    // The vertices' buffer (0: the pack's streams, 1: the cells' records, 2: this frame's models) and the word
    // they start at; the same for the instances.
    src: vec4<u32>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var heights: texture_2d<f32>;
@group(0) @binding(2) var sun_map: texture_depth_2d;
@group(0) @binding(3) var sun_compare: sampler_comparison;
@group(0) @binding(4) var lamp_tex: texture_2d<f32>;
@group(0) @binding(5) var around_tex: texture_2d<f32>;
@group(0) @binding(6) var clamped: sampler;
@group(0) @binding(7) var<storage, read> resident: array<u32>;
@group(0) @binding(8) var<storage, read> arena: array<u32>;
@group(0) @binding(9) var<storage, read> ring: array<u32>;
@group(1) @binding(0) var<uniform> draw: Draw;
@group(2) @binding(0) var pic_a: texture_2d<f32>;
@group(2) @binding(1) var pic_b: texture_2d<f32>;
@group(2) @binding(2) var repeat: sampler;

// ---- records

fn word(src: u32, i: u32) -> u32 {
    if (src == 1u) {
        return arena[i];
    }
    if (src == 2u) {
        return ring[i];
    }
    return resident[i];
}

fn byte(w: u32, k: u32) -> u32 {
    return (w >> (k * 8u)) & 255u;
}

fn snorm8(b: u32) -> f32 {
    return max(f32(bitcast<i32>(b << 24u) >> 24u) / 127.0, -1.0);
}

fn unorm8(b: u32) -> f32 {
    return f32(b) / 255.0;
}

fn snorm16(h: u32) -> f32 {
    return max(f32(bitcast<i32>(h << 16u) >> 16u) / 32767.0, -1.0);
}

// What every vertex layout begins with: the position over the batch's box (U16N × 3), the normal (S8N × 3) and the
// share of the sky it sees (U8N).
struct Head {
    pos: vec3<f32>,
    normal: vec3<f32>,
    ao: f32,
}

fn head(w0: u32, w1: u32, w2: u32) -> Head {
    var h: Head;
    h.pos = vec3<f32>(f32(w0 & 0xffffu), f32(w0 >> 16u), f32(w1 & 0xffffu)) / 65535.0;
    h.normal = vec3<f32>(snorm8(byte(w1, 2u)), snorm8(byte(w1, 3u)), snorm8(byte(w2, 0u)));
    h.ao = unorm8(byte(w2, 1u));
    return h;
}

// A WallVertex (and a tree's, a sign's): the head, then uv (S16N × 2) and its tint (U8N × 4).
struct WallV {
    h: Head,
    uv: vec2<f32>,
    tint: vec4<f32>,
}

fn wall_vertex(index: u32) -> WallV {
    let at = draw.src.y + index * 5u;
    let w2 = word(draw.src.x, at + 2u);
    let w3 = word(draw.src.x, at + 3u);
    let w4 = word(draw.src.x, at + 4u);
    var v: WallV;
    v.h = head(word(draw.src.x, at), word(draw.src.x, at + 1u), w2);
    v.uv = vec2<f32>(snorm16(w2 >> 16u), snorm16(w3 & 0xffffu));
    v.tint = vec4<f32>(unorm8(byte(w3, 2u)), unorm8(byte(w3, 3u)), unorm8(byte(w4, 0u)), unorm8(byte(w4, 1u)));
    return v;
}

// A SolidVertex: the head, then its colour, its glow (U8N × 4 each) and its gloss (U8N × 2).
struct SolidV {
    h: Head,
    color: vec4<f32>,
    glow: vec4<f32>,
    gloss: vec2<f32>,
}

fn solid_vertex(index: u32) -> SolidV {
    let at = draw.src.y + index * 5u;
    let w2 = word(draw.src.x, at + 2u);
    let w3 = word(draw.src.x, at + 3u);
    let w4 = word(draw.src.x, at + 4u);
    var v: SolidV;
    v.h = head(word(draw.src.x, at), word(draw.src.x, at + 1u), w2);
    v.color = vec4<f32>(unorm8(byte(w2, 2u)), unorm8(byte(w2, 3u)), unorm8(byte(w3, 0u)), unorm8(byte(w3, 1u)));
    v.glow = vec4<f32>(unorm8(byte(w3, 2u)), unorm8(byte(w3, 3u)), unorm8(byte(w4, 0u)), unorm8(byte(w4, 1u)));
    v.gloss = vec2<f32>(unorm8(byte(w4, 2u)), unorm8(byte(w4, 3u)));
    return v;
}

// A GroundVertex: the head, then its tint.
struct GroundV {
    h: Head,
    tint: vec4<f32>,
}

fn ground_vertex(index: u32) -> GroundV {
    let at = draw.src.y + index * 4u;
    let w2 = word(draw.src.x, at + 2u);
    let w3 = word(draw.src.x, at + 3u);
    var v: GroundV;
    v.h = head(word(draw.src.x, at), word(draw.src.x, at + 1u), w2);
    v.tint = vec4<f32>(unorm8(byte(w2, 2u)), unorm8(byte(w2, 3u)), unorm8(byte(w3, 0u)), unorm8(byte(w3, 1u)));
    return v;
}

// An Instance: the rows of its matrix, and its tint.
struct Inst {
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
    tint: vec4<f32>,
}

fn instance(index: u32) -> Inst {
    let at = draw.src.w + index * 13u;
    let s = draw.src.z;
    var i: Inst;
    i.r0 = vec4<f32>(bitcast<f32>(word(s, at)), bitcast<f32>(word(s, at + 1u)), bitcast<f32>(word(s, at + 2u)), bitcast<f32>(word(s, at + 3u)));
    i.r1 = vec4<f32>(bitcast<f32>(word(s, at + 4u)), bitcast<f32>(word(s, at + 5u)), bitcast<f32>(word(s, at + 6u)), bitcast<f32>(word(s, at + 7u)));
    i.r2 = vec4<f32>(bitcast<f32>(word(s, at + 8u)), bitcast<f32>(word(s, at + 9u)), bitcast<f32>(word(s, at + 10u)), bitcast<f32>(word(s, at + 11u)));
    i.tint = unpack4x8unorm(word(s, at + 12u));
    return i;
}

fn local_of(pos: vec3<f32>) -> vec3<f32> {
    return pos * draw.span.xyz + draw.origin.xyz;
}

fn placed(i: Inst, local: vec3<f32>) -> vec3<f32> {
    let p = vec4<f32>(local, 1.0);
    return vec3<f32>(dot(i.r0, p), dot(i.r1, p), dot(i.r2, p));
}

fn turned(i: Inst, n: vec3<f32>) -> vec3<f32> {
    return normalize(vec3<f32>(dot(i.r0.xyz, n), dot(i.r1.xyz, n), dot(i.r2.xyz, n)));
}

fn clip(world: vec3<f32>) -> vec4<f32> {
    return vec4<f32>(world, 1.0) * frame.vp;
}

// ---- frame.cg

// How much haze lies over a point `depth` from the eye: FogExp2 through a layer that thins with height, its
// density the mean between the eye's height and the draw's middle.
fn haze_of(depth: f32) -> f32 {
    var t = draw.origin.w * depth;
    t *= t;
    return min(1.0 - 1.0 / (1.0 + t + 0.5 * t * t), frame.f[3].x);
}

// What a surface reflects of the sky by how the eye sees it (Lagarde's Fresnel), and how much of its own colour it
// shows lit (a).
fn reflection(world: vec3<f32>, n: vec3<f32>, gloss: vec2<f32>) -> vec4<f32> {
    let to = frame.f[4].xyz - world;
    let v = to * inverseSqrt(dot(to, to));
    let nv = saturate(dot(n, v));
    let ry = 2.0 * nv * n.y - v.y;
    let open = saturate(ry * 2.5 + world.y * (1.0 / 120.0));
    let env = mix(frame.f[2].rgb * (LIGHT_SCALE * 0.15), frame.f[5].rgb, open);
    let f0 = mix(0.04, 0.3, gloss.y);
    let k = 1.0 - nv;
    let k2 = k * k;
    let f = f0 + (max(1.0 - gloss.x, f0) - f0) * (k2 * k2 * k);
    return vec4<f32>(env * f, 1.0 - gloss.y);
}

// Where a point is in the sun's map, and how much the map counts there (w).
fn sun_at(world: vec3<f32>) -> vec4<f32> {
    let s = vec4<f32>(world, 1.0) * frame.sun_read;
    let off = abs(s.xy - 0.5);
    return vec4<f32>(s.xyz, saturate((0.5 - max(off.x, off.y)) * 12.0));
}

fn hemi(n: vec3<f32>) -> vec3<f32> {
    return mix(frame.f[2].rgb, frame.f[1].rgb, n.y * 0.5 + 0.5);
}

fn shadow_map(world: vec3<f32>, n: vec3<f32>) -> vec2<f32> {
    return (world.xz + n.xz * SHADOW_OUT) * frame.f[3].y + frame.f[3].zw;
}

// The shadow window's height at `uv`, its four texels mixed as a linear filter mixes them.
fn shadow_height(uv: vec2<f32>) -> f32 {
    let size = vec2<f32>(textureDimensions(heights));
    let p = uv * size - 0.5;
    let at = floor(p);
    let f = p - at;
    let top = vec2<i32>(size) - 1;
    let a = clamp(vec2<i32>(at), vec2<i32>(0), top);
    let b = clamp(vec2<i32>(at) + 1, vec2<i32>(0), top);
    let h00 = textureLoad(heights, vec2<i32>(a.x, a.y), 0).r;
    let h10 = textureLoad(heights, vec2<i32>(b.x, a.y), 0).r;
    let h01 = textureLoad(heights, vec2<i32>(a.x, b.y), 0).r;
    let h11 = textureLoad(heights, vec2<i32>(b.x, b.y), 0).r;
    return mix(mix(h00, h10, f.x), mix(h01, h11, f.x), f.y);
}

// The height grid's shadow at a fragment.
fn grid_lit(height: f32, map: vec2<f32>) -> f32 {
    return saturate((height - shadow_height(map) * frame.lamps.y) * SHADOW_K + 1.0);
}

// How much sun reaches a fragment: the grid's shadow and, where the sun's map counts, whether anything nearer the
// sun stands over it there.
fn sun_lit(grid: f32, at: vec4<f32>) -> f32 {
    let seen = textureSampleCompareLevel(sun_map, sun_compare, at.xy, at.z - SUN_BIAS);
    return min(grid, mix(1.0, seen, at.w));
}

fn lamps(under: vec3<f32>, around: vec3<f32>, reach: f32) -> vec3<f32> {
    return (under * under + around * around * AROUND) * (frame.lamps.x * reach);
}

fn lamp_light(uv: vec2<f32>, reach: f32) -> vec3<f32> {
    return lamps(textureSampleLevel(lamp_tex, clamped, uv, 0.0).rgb, textureSampleLevel(around_tex, clamped, uv, 0.0).rgb, reach);
}

fn night() -> bool {
    return frame.sun.w > 0.5;
}

// Linear light to what the display shows, under `haze` of the draw's haze.
fn shown(c: vec3<f32>, haze: f32) -> vec3<f32> {
    let t = frame.tone;
    let toned = c * (t.x * c + t.y) / (c * (t.z * c + t.w) + 1.0);
    return mix(toned, draw.haze.rgb, haze);
}

// ---- the walls of facade boxes (wall_v.cg, wall_f.cg)

struct WallOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) map: vec2<f32>,
    @location(2) height: f32,
    @location(3) lamp: vec2<f32>,
    @location(4) spec: vec4<f32>,
    @location(5) light: vec4<f32>,
    @location(6) tint: vec4<f32>,
    @location(7) sun: vec4<f32>,
}

@vertex
fn vs_wall(@builtin(vertex_index) index: u32) -> WallOut {
    let v = wall_vertex(index);
    let n = v.h.normal;
    let world = local_of(v.h.pos);
    var o: WallOut;
    o.position = clip(world);
    o.uv = v.uv * UV_RANGE;
    o.map = shadow_map(world, n);
    o.height = world.y;
    o.sun = sun_at(world);
    // (a wall takes the light on the ground before it, fading up its first storeys)
    o.lamp = (world.xz + n.xz * 2.0) * frame.lamp_map.xz + frame.lamp_map.yw;
    o.light = vec4<f32>(hemi(n) * v.h.ao, max(dot(n, frame.f[0].xyz), 0.0));
    o.tint = vec4<f32>(v.tint.rgb * 2.0, haze_of(o.position.w));
    o.spec = reflection(world, n, draw.gloss.xy);
    return o;
}

@fragment
fn fs_wall(i: WallOut) -> @location(0) vec4<f32> {
    let lit = sun_lit(grid_lit(i.height, i.map), i.sun);
    var light = i.light.rgb + frame.sun.rgb * (i.light.a * lit);
    let page = textureSample(pic_a, repeat, i.uv).rgb;
    let rooms = textureSample(pic_b, repeat, i.uv).rgb;
    if (night()) {
        light += lamp_light(i.lamp, saturate(1.0 - i.height * 0.12));
    }
    let a = page * i.tint.rgb;
    var c = a * a * light * (LIGHT_SCALE * i.spec.a) + i.spec.rgb;
    if (night()) {
        c += rooms * rooms * draw.span.w;
    }
    return vec4<f32>(shown(c, i.tint.a), 1.0);
}

// ---- what is coloured by vertex (solid_v.cg, inst_v.cg, solid_f.cg): a shape of a cell or a block, a shape at
// each instance, a part seen through

struct SolidOut {
    @builtin(position) position: vec4<f32>,
    @location(0) map: vec2<f32>,
    @location(1) height: f32,
    @location(2) lamp: vec3<f32>,
    @location(3) spec: vec4<f32>,
    @location(4) light: vec4<f32>,
    @location(5) color: vec4<f32>,
    @location(6) glow: vec4<f32>,
    @location(7) sun: vec4<f32>,
}

fn solid_out(world: vec3<f32>, n: vec3<f32>, ao: f32, color: vec3<f32>, glow: vec4<f32>, always: f32, gloss: vec2<f32>, alpha: f32) -> SolidOut {
    var o: SolidOut;
    o.position = clip(world);
    o.map = shadow_map(world, n);
    o.height = world.y;
    o.sun = sun_at(world);
    // Seen from both sides: lit by the side that faces the light.
    var sun = max(dot(n, frame.f[0].xyz), 0.0);
    if (draw.gloss.z > 0.5) {
        sun = abs(dot(n, frame.f[0].xyz)) * 0.8 + 0.2;
    }
    o.light = vec4<f32>(hemi(n) * ao, sun);
    o.color = vec4<f32>(color, haze_of(o.position.w));
    // What glows: by night (w of f[1]), or always (alpha 1).
    let on = mix(frame.f[1].w, 1.0, step(0.998, always));
    o.glow = vec4<f32>(glow.rgb * (glow.a * GLOW_RANGE * on), alpha);
    o.spec = reflection(world, n, gloss);
    o.lamp = vec3<f32>(world.xz * frame.lamp_map.xz + frame.lamp_map.yw, saturate(1.2 - world.y * 0.1));
    return o;
}

@vertex
fn vs_solid(@builtin(vertex_index) index: u32) -> SolidOut {
    let v = solid_vertex(index);
    return solid_out(local_of(v.h.pos), v.h.normal, v.h.ao, v.color.rgb, v.glow, v.color.a, v.gloss, 1.0);
}

@vertex
fn vs_inst(@builtin(vertex_index) index: u32, @builtin(instance_index) which: u32) -> SolidOut {
    let v = solid_vertex(index);
    let i = instance(which);
    let world = placed(i, local_of(v.h.pos));
    // The tint is the instance's colour (its paint) over as much of the vertex as is paint: alpha 254 is all of it.
    let paint = saturate(v.color.a * 1.004);
    let color = saturate(v.color.rgb * mix(vec3<f32>(1.0), i.tint.rgb * 2.0, paint));
    // (a: how much of the instance is drawn, for a part seen through)
    return solid_out(world, turned(i, v.h.normal), v.h.ao, color, v.glow, v.color.a, v.gloss, i.tint.a);
}

fn solid_colour(i: SolidOut) -> vec3<f32> {
    let lit = sun_lit(grid_lit(i.height, i.map), i.sun);
    var light = i.light.rgb + frame.sun.rgb * (i.light.a * lit);
    if (night()) {
        light += lamp_light(i.lamp.xy, i.lamp.z);
    }
    let c = i.color.rgb * i.color.rgb * light * (LIGHT_SCALE * i.spec.a) + i.spec.rgb + i.glow.rgb;
    return shown(c, i.color.a);
}

@fragment
fn fs_solid(i: SolidOut) -> @location(0) vec4<f32> {
    return vec4<f32>(solid_colour(i), 1.0);
}

@fragment
fn fs_veil(i: SolidOut) -> @location(0) vec4<f32> {
    return vec4<f32>(solid_colour(i), draw.haze.w * i.glow.a);
}

// ---- the ground under its pictures (ground_v.cg, ground_f.cg)

struct GroundOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) map: vec2<f32>,
    @location(2) height: f32,
    @location(3) lamp: vec2<f32>,
    @location(4) spec: vec4<f32>,
    @location(5) mirror: vec3<f32>,
    @location(6) light: vec4<f32>,
    @location(7) tint: vec4<f32>,
    @location(8) sun: vec4<f32>,
}

@vertex
fn vs_ground(@builtin(vertex_index) index: u32) -> GroundOut {
    let v = ground_vertex(index);
    let n = v.h.normal;
    let world = local_of(v.h.pos);
    var o: GroundOut;
    o.position = clip(world);
    o.uv = world.xz * draw.tile.x + draw.tile.yz;
    o.map = shadow_map(world, n);
    o.height = world.y;
    o.sun = sun_at(world);
    o.lamp = world.xz * frame.lamp_map.xz + frame.lamp_map.yw;
    o.light = vec4<f32>(hemi(n) * v.h.ao, max(dot(n, frame.f[0].xyz), 0.0));
    // Wet ground is darker (the reference's surfaces darken with uRoadWetness; their colours here were probed dry).
    o.tint = vec4<f32>(v.tint.rgb * (2.0 - 0.6 * frame.f[4].w), haze_of(o.position.w));
    o.spec = reflection(world, n, draw.gloss.xy);
    // The lamp a point mirrors stands on the eye's line through it, as far again as a lamp's height (8 m) is to the
    // eye's height over the point; it shows by Schlick's Fresnel, as wet and as smooth as the surface is.
    let to = frame.f[4].xyz - world;
    let above = max(to.y, 0.5);
    let lamp = world.xz - to.xz * (8.0 / above);
    let k = 1.0 - saturate(to.y * inverseSqrt(dot(to, to)));
    let k2 = k * k;
    let smoothness = saturate(1.0 - draw.gloss.x);
    o.mirror = vec3<f32>(lamp * frame.lamp_map.xz + frame.lamp_map.yw, frame.f[4].w * smoothness * smoothness * k2 * k2 * k);
    return o;
}

@fragment
fn fs_ground(i: GroundOut) -> @location(0) vec4<f32> {
    let lit = sun_lit(grid_lit(i.height, i.map), i.sun);
    var light = i.light.rgb + frame.sun.rgb * (i.light.a * lit);
    let picture = textureSample(pic_a, repeat, i.uv).rgb;
    if (night()) {
        // The lamps' pools on the ground.
        light += lamp_light(i.lamp, 1.0);
    }
    let a = picture * i.tint.rgb;
    var c = a * a * light * (LIGHT_SCALE * i.spec.a) + i.spec.rgb;
    if (night()) {
        // A wet street mirrors the lamps.
        let m = textureSampleLevel(lamp_tex, clamped, i.mirror.xy, 0.0).rgb;
        c += m * m * (frame.lamps.x * LIGHT_SCALE * i.mirror.z);
    }
    return vec4<f32>(shown(c, i.tint.a), 1.0);
}

// ---- trees: a crown of cards at each tree (leaf_v.cg, leaf_f.cg)

struct LeafOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) leaf: f32,
    @location(2) lamp: vec3<f32>,
    @location(3) light: vec4<f32>,
    @location(4) color: vec4<f32>,
    @location(5) sun: vec4<f32>,
}

@vertex
fn vs_leaf(@builtin(vertex_index) index: u32, @builtin(instance_index) which: u32) -> LeafOut {
    let v = wall_vertex(index);
    let i = instance(which);
    let world = placed(i, local_of(v.h.pos));
    let n = turned(i, v.h.normal);
    var o: LeafOut;
    o.position = clip(world);
    o.uv = v.uv * UV_RANGE;
    o.sun = sun_at(world);
    // As the reference lights a card (TreeNodeMaterial): by the side the eye sees, the sun on it and a quarter of
    // that through it from behind; the sky's light darkened deep in the crown, and its colour too.
    let to = frame.f[4].xyz - world;
    let seen = select(n, -n, dot(n, to) < 0.0);
    let s = dot(seen, frame.f[0].xyz);
    let ao = v.h.ao;
    let crown = saturate((ao - 0.55) * (1.0 / 0.45));
    // The reference's crowns shadow themselves; here the sun reaches the outer leaves, and little of it the inner.
    let reach = crown * crown * 0.8 + 0.2;
    o.light = vec4<f32>(hemi(seen) * (ao * (crown * 0.75 + 0.25)), (max(s, 0.0) + max(-s, 0.0) * 0.25) * ao * reach);
    // A card takes the tree's tint over the picture; bark keeps its colour (alpha: which it is).
    o.color = vec4<f32>(saturate(mix(v.tint.rgb, i.tint.rgb * 2.0, v.tint.a)), haze_of(o.position.w));
    // (a lamp lights the crown from below and within)
    o.lamp = vec3<f32>(world.xz * frame.lamp_map.xz + frame.lamp_map.yw, saturate(1.4 - world.y * 0.1) * ao * 0.5);
    o.leaf = v.tint.a;
    return o;
}

@fragment
fn fs_leaf(i: LeafOut) -> @location(0) vec4<f32> {
    let t = textureSample(pic_a, repeat, i.uv);
    // Bark reads white and opaque: its colour is its own.
    let leaf = mix(vec4<f32>(1.0), t, i.leaf);
    if (leaf.a < 0.5) {
        discard;
    }
    // (a leaf reads the sun's map alone; past the map a tree is lit)
    let lit = sun_lit(1.0, i.sun);
    var light = i.light.rgb + frame.sun.rgb * (i.light.a * lit);
    if (night()) {
        light += lamp_light(i.lamp.xy, i.lamp.z);
    }
    let a = leaf.rgb * i.color.rgb;
    let c = a * a * light * LIGHT_SCALE;
    return vec4<f32>(shown(c, i.color.a), 1.0);
}

// ---- signs and billboards (picture_v.cg, picture_f.cg)

struct PictureOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) map: vec2<f32>,
    @location(2) height: f32,
    @location(3) spec: vec4<f32>,
    @location(4) light: vec4<f32>,
    @location(5) haze: f32,
    @location(6) sun: vec4<f32>,
}

fn picture_out(world: vec3<f32>, n: vec3<f32>, v: WallV) -> PictureOut {
    var o: PictureOut;
    o.position = clip(world);
    o.uv = v.uv * UV_RANGE;
    o.map = shadow_map(world, n);
    o.height = world.y;
    o.sun = sun_at(world);
    let lit = vec4<f32>(hemi(n) * v.h.ao, max(dot(n, frame.f[0].xyz), 0.0));
    // Unlit: the picture as it is, day and night; and it reflects nothing either.
    let unlit = vec4<f32>(1.0 / LIGHT_SCALE, 1.0 / LIGHT_SCALE, 1.0 / LIGHT_SCALE, 0.0);
    let u = step(0.5, v.tint.a);
    o.light = mix(lit, unlit, u);
    o.haze = haze_of(o.position.w);
    o.spec = mix(reflection(world, n, draw.gloss.xy), vec4<f32>(0.0, 0.0, 0.0, 1.0), u);
    return o;
}

@vertex
fn vs_picture(@builtin(vertex_index) index: u32) -> PictureOut {
    let v = wall_vertex(index);
    return picture_out(local_of(v.h.pos), v.h.normal, v);
}

@vertex
fn vs_picture_inst(@builtin(vertex_index) index: u32, @builtin(instance_index) which: u32) -> PictureOut {
    let v = wall_vertex(index);
    let i = instance(which);
    return picture_out(placed(i, local_of(v.h.pos)), turned(i, v.h.normal), v);
}

@fragment
fn fs_picture(i: PictureOut) -> @location(0) vec4<f32> {
    let lit = sun_lit(grid_lit(i.height, i.map), i.sun);
    let light = i.light.rgb + frame.sun.rgb * (i.light.a * lit);
    let p = textureSample(pic_a, repeat, i.uv).rgb;
    let c = p * p * light * (draw.span.w * LIGHT_SCALE * i.spec.a) + i.spec.rgb;
    return vec4<f32>(shown(c, i.haze), 1.0);
}

// ---- a car's part under pictures of the car seen along each of its axes (box_v.cg, box_f.cg)

struct BoxOut {
    @builtin(position) position: vec4<f32>,
    @location(0) at: vec3<f32>,
    @location(1) map: vec2<f32>,
    @location(2) height: f32,
    @location(3) spec: vec4<f32>,
    @location(4) facing: vec3<f32>,
    @location(5) light: vec4<f32>,
    @location(6) paint: vec4<f32>,
    @location(7) lamp: vec3<f32>,
    @location(8) sun: vec4<f32>,
}

@vertex
fn vs_box(@builtin(vertex_index) index: u32, @builtin(instance_index) which: u32) -> BoxOut {
    let v = solid_vertex(index);
    let i = instance(which);
    let local = local_of(v.h.pos);
    let world = placed(i, local);
    let n = turned(i, v.h.normal);
    var o: BoxOut;
    o.position = clip(world);
    o.at = (local - draw.bake0.xyz) * draw.bake1.xyz;
    o.facing = v.h.normal;
    o.map = shadow_map(world, n);
    o.height = world.y;
    o.sun = sun_at(world);
    o.light = vec4<f32>(hemi(n) * v.h.ao, max(dot(n, frame.f[0].xyz), 0.0));
    // The instance's paint, over as much of a texel as the pictures say is paint.
    o.paint = vec4<f32>(i.tint.rgb * 2.0, haze_of(o.position.w));
    o.spec = reflection(world, n, v.gloss);
    o.lamp = vec3<f32>(world.xz * frame.lamp_map.xz + frame.lamp_map.yw, saturate(1.2 - world.y * 0.1));
    return o;
}

@fragment
fn fs_box(i: BoxOut) -> @location(0) vec4<f32> {
    // The view the surface faces most (the atlas: two across, four down; ±x, then ±y, then ±z), across and down it.
    let f = abs(i.facing);
    let at = clamp(i.at, vec3<f32>(0.004), vec3<f32>(0.996));
    var uv: vec2<f32>;
    if (f.x >= f.y && f.x >= f.z) {
        uv = vec2<f32>(at.z * 0.5 + select(0.5, 0.0, i.facing.x > 0.0), at.y * 0.25);
    } else if (f.y >= f.z) {
        uv = vec2<f32>(at.z * 0.5 + select(0.5, 0.0, i.facing.y > 0.0), at.x * 0.25 + 0.25);
    } else {
        uv = vec2<f32>(at.x * 0.5 + select(0.5, 0.0, i.facing.z > 0.0), at.y * 0.25 + 0.5);
    }
    let t = textureSample(pic_a, repeat, uv);
    let a = t.rgb * mix(vec3<f32>(1.0), i.paint.rgb, t.a);
    let lit = sun_lit(grid_lit(i.height, i.map), i.sun);
    var light = i.light.rgb + frame.sun.rgb * (i.light.a * lit);
    if (night()) {
        light += lamp_light(i.lamp.xy, i.lamp.z);
    }
    // (the glow picture's alpha: how much of the sky this texel mirrors)
    let e = textureSample(pic_b, repeat, uv);
    var c = a * a * light * (LIGHT_SCALE * i.spec.a) + i.spec.rgb * e.a;
    c += e.rgb * e.rgb * draw.span.w;
    return vec4<f32>(shown(c, i.paint.a), 1.0);
}

// ---- the sun's depth map (sun_v.cg, sun_f.cg): a shape's own points, a shape at instances, a tree's cut cards

@vertex
fn vs_sun(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let at = draw.src.y + index * 5u;
    let h = head(word(draw.src.x, at), word(draw.src.x, at + 1u), 0u);
    return vec4<f32>(local_of(h.pos), 1.0) * frame.sun_draw;
}

@vertex
fn vs_sun_inst(@builtin(vertex_index) index: u32, @builtin(instance_index) which: u32) -> @builtin(position) vec4<f32> {
    let at = draw.src.y + index * 5u;
    let h = head(word(draw.src.x, at), word(draw.src.x, at + 1u), 0u);
    return vec4<f32>(placed(instance(which), local_of(h.pos)), 1.0) * frame.sun_draw;
}

struct SunLeafOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec3<f32>,
}

@vertex
fn vs_sun_leaf(@builtin(vertex_index) index: u32, @builtin(instance_index) which: u32) -> SunLeafOut {
    let v = wall_vertex(index);
    var o: SunLeafOut;
    o.position = vec4<f32>(placed(instance(which), local_of(v.h.pos)), 1.0) * frame.sun_draw;
    o.uv = vec3<f32>(v.uv * UV_RANGE, v.tint.a);
    return o;
}

@fragment
fn fs_sun_leaf(i: SunLeafOut) {
    // A tree's card is cut where its leaves' picture is under half.
    let a = mix(1.0, textureSample(pic_a, repeat, i.uv.xy).a, i.uv.z);
    if (a < 0.5) {
        discard;
    }
}
