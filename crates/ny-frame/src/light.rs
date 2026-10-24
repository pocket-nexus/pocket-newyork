//! The hour's light as the scene programs take it: `uFrame` and `uLook` (`vita/shaders/frame.cg`), the haze by
//! direction, the sun's disc and the glow of what is bright.

use ny_pack::Preset;
use ny_sim::math::V3;

/// Light reaches the fragments as a colour, which holds 0..1: it is carried divided by this.
pub const LIGHT_SCALE: f32 = 2.0;
/// What every scene program takes per frame (`uFrame`, `shaders/frame.cg`).
pub type Frame = [f32; 24];
/// A share of the lamps' light around a point that a surface takes (`lamps` in frame.cg).
pub const AROUND: f32 = 0.5;
/// Depth along the sun a point may lie behind what its map holds and be lit.
pub const SUN_BIAS: f32 = 0.0004;
/// Metres under the shadow height over which the sun's light goes out, and how far out from a surface it reads.
pub const SHADOW_SOFT: f32 = 2.5;
pub const SHADOW_OUT: f32 = 2.5;

/// The haze's colour by direction (the sky just over the horizon, as the display shows it): towards the sun,
/// across from it and away from it, the sun's direction along the ground, and the eye and where it looks.
pub struct Haze {
    pub colors: [[f32; 3]; 3],
    pub sun: [f32; 2],
    pub eye: V3,
    pub ahead: V3,
    /// The haze per metre at the ground, and 1 / the height over which it thins by e.
    pub density: f32,
    pub thins: f32,
}

impl Haze {
    /// The haze per metre between the eye and a point at height `y`: the mean of exp(−h / H) over the heights
    /// between.
    pub fn density_to(&self, y: f32) -> f32 {
        let (a, b) = (y.max(0.0) * self.thins, self.eye.y.max(0.0) * self.thins);
        let mean = if libm::fabsf(b - a) > 0.002 { (libm::expf(-a) - libm::expf(-b)) / (b - a) } else { libm::expf(-a) };
        self.density * mean
    }

    /// The haze's colour towards `at`: a quadratic in the cosine of its angle along the ground to the sun's.
    pub fn towards(&self, at: V3) -> [f32; 3] {
        let (dx, dz) = (at.x - self.eye.x, at.z - self.eye.z);
        let len = libm::sqrtf(dx * dx + dz * dz);
        let (dx, dz) = if len > 1.0 { (dx / len, dz / len) } else { (self.ahead.x, self.ahead.z) };
        let c = dx * self.sun[0] + dz * self.sun[1];
        let [t, s, a] = self.colors;
        [0, 1, 2].map(|k| s[k] + (t[k] - a[k]) * 0.5 * c + ((t[k] + a[k]) * 0.5 - s[k]) * c * c)
    }
}

/// A linear colour as sRGB: what multiplies a probed colour or a picture, which are sRGB.
pub fn display(c: [f32; 3]) -> [f32; 3] {
    c.map(|x| if x <= 0.0031308 { x * 12.92 } else { 1.055 * libm::powf(x, 1.0 / 2.4) - 0.055 })
}

/// The hour's light as the scene programs take it (`shaders/frame.cg`): `uFrame` (the shadow window's mapping is
/// the caller's), `uLook`, the haze, and whether the rooms are lit.
pub fn frame_light(p: &Preset, lamp_unit: f32, eye: V3, ahead: V3) -> (Frame, [f32; 12], Haze, bool) {
    let k = 1.0 / LIGHT_SCALE;
    // Street lamps and lit signs come on with the dusk (street-details.js: (glow - 0.36) × 1.15).
    let glow = ((p.interior_glow - 0.36) * 1.15).clamp(0.0, 1.25);
    let d = p.sun_dir;
    let flat = libm::sqrtf(d[0] * d[0] + d[2] * d[2]).max(1e-4);
    let (s, g) = (p.light_sky, p.light_ground);
    let ih = 1.0 / p.fog_height.max(1.0);
    // What a surface reflects of the sky: between just over the horizon and a third of the way up.
    let env = [0, 1, 2].map(|k| (p.env_horizon[k] + p.env_up[k]) * 0.5);
    #[rustfmt::skip]
    let frame = [
        d[0], d[1], d[2], p.fog_density,
        s[0] * k, s[1] * k, s[2] * k, glow,
        g[0] * k, g[1] * k, g[2] * k, p.interior_glow,
        1.0, 0.0, 0.0, 0.0,
        eye.x, eye.y, eye.z, p.road_wetness,
        env[0], env[1], env[2], 0.0,
    ];
    let haze = Haze { colors: p.haze, sun: [d[0] / flat, d[2] / flat], eye, ahead, density: p.fog_density, thins: ih };
    let sun = p.light_sun.map(|x| x * k);
    // [2].x: the lamps' light, as a texel's full value stands for, as far as the lamps are on; at 0.55 of the pools
    // the reference's cobra heads leave, which is where its sidewalks under a lamp show at midnight beside these
    // (its ground darkens with the wet more than the probe's dry colours do).
    let look = [p.tone[0], p.tone[1], p.tone[2], p.tone[3], sun[0], sun[1], sun[2], 0.0, lamp_unit * glow * k * 0.55, 0.0, 0.0, 0.0];
    (frame, look, haze, p.interior_glow > 0.3)
}

/// The sun's disc in its own colour, brighter than the sky round it (the moon's, dimmer): towards it, then its
/// colour as the sky's programs take it.
pub fn sun_disc(p: &Preset) -> [f32; 8] {
    let c = p.light_sun;
    let top = c[0].max(c[1]).max(c[2]).max(1e-4);
    let k = if p.night > 0.5 { 0.55 } else { 1.6 };
    let d = p.sun_dir;
    [d[0], d[1], d[2], 0.0, c[0] / top * k, c[1] / top * k, c[2] / top * k, 0.0]
}

/// What is bright in the frame glows over it, as the reference's bloom does for the hour: its strength, and its
/// threshold where the display shows it (under it, the hazy sky by the horizon would bloom and wash the frame out).
pub fn bloom(p: &Preset) -> (f32, f32) {
    (p.bloom[0] * 1.5, p.bloom[1].clamp(0.6, 0.95))
}
