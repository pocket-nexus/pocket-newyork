//! What moves, as instances of the pack's models each frame: the traffic's cars (the reference's full cars near
//! the eye, its stand-ins past them), the player's vehicle or the player on foot, the helicopter's rotors, the
//! sidewalks' pedestrians (`ny_sim::crowd`) as the crowd's baked figures, each at its own height and in its own coat
//! (the figures' coats are paint), and the highway's cars (`ny_sim::highway`) in their paint.

use alloc::format;
use alloc::vec::Vec;

use ny_pack::Instance;
use ny_sim::highway::HighwayPose;
use ny_sim::math::V3;
use ny_sim::player::{MAIN_ROTOR_AT, TAIL_ROTOR_AT};
use ny_sim::traffic::Kind;
use ny_sim::Game;
use ny_view::Planes;

use crate::light::display;

/// Metres within which a car is the reference's full model.
pub const NEAR_CAR: f32 = 40.0;
/// People nearer than this take the near figure; none is drawn past `PEOPLE_REACH`, nor more than `PEOPLE_MAX` of
/// them (the nearest). The far figure is 954 triangles, the near one 3 434.
pub const NEAR_PERSON: f32 = 15.0;
pub const PEOPLE_REACH: f32 = 70.0;
pub const PEOPLE_MAX: usize = 24;
/// Below this speed a person stands.
const STANDING: f32 = 0.3;
/// A rotor drawn where it is, and behind each blade, fading, where it passed since the frame before: the main rotor
/// turns 46° a frame at 30 frames a second, half the way to the next blade, and its blades alone would seem to stand
/// or turn back.
pub const TRAIL: [u8; 8] = [255, 96, 72, 54, 40, 30, 22, 16];

/// An instance at `(x, y, z)` turned by yaw (about y), then pitch (about x), then roll (about z), as three.js's
/// 'YXZ' order composes them, with `tint` (linear, 1 = as the model has it).
pub fn placed(x: f32, y: f32, z: f32, yaw: f32, pitch: f32, roll: f32, tint: [f32; 3]) -> Instance {
    let (sy, cy) = (libm::sinf(yaw), libm::cosf(yaw));
    let (sp, cp) = (libm::sinf(pitch), libm::cosf(pitch));
    let (sr, cr) = (libm::sinf(roll), libm::cosf(roll));
    // R = Ry · Rx · Rz
    let r = [
        [cy * cr + sy * sp * sr, -cy * sr + sy * sp * cr, sy * cp],
        [cp * sr, cp * cr, -sp],
        [-sy * cr + cy * sp * sr, sy * sr + cy * sp * cr, cy * cp],
    ];
    let t = tint.map(|c| libm::roundf(c * 128.0).clamp(0.0, 255.0) as u8);
    Instance { rows: [[r[0][0], r[0][1], r[0][2], x], [r[1][0], r[1][1], r[1][2], y], [r[2][0], r[2][1], r[2][2], z]], tint: [t[0], t[1], t[2], 255] }
}

/// A part of `base`'s model turned about its own pivot: at `at` in `base`'s frame, turned by `angle` about its x
/// (`axis` 0) or its y (1), its tint `base`'s, `alpha` how much of it is drawn (under 255: seen through, the model's
/// whole group with it).
pub fn turned(base: &Instance, at: [f32; 3], axis: usize, angle: f32, alpha: u8) -> Instance {
    let (s, c) = (libm::sinf(angle), libm::cosf(angle));
    let local = if axis == 0 { [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]] } else { [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]] };
    let mut rows = [[0.0f32; 4]; 3];
    for (i, row) in rows.iter_mut().enumerate() {
        let b = base.rows[i];
        for j in 0..3 {
            row[j] = b[0] * local[0][j] + b[1] * local[1][j] + b[2] * local[2][j];
        }
        row[3] = b[0] * at[0] + b[1] * at[1] + b[2] * at[2] + b[3];
    }
    Instance { rows, tint: [base.tint[0], base.tint[1], base.tint[2], alpha] }
}

/// The models of what moves, by the names the export gave them, and what the frames before left.
pub struct Movers {
    /// The traffic's kinds (`ny_sim::traffic::Kind`'s order): their stand-ins, and the full cars near the eye.
    kinds: [Option<usize>; 5],
    kinds_near: [Option<usize>; 5],
    /// The player's vehicles (`ny_sim::Mode`'s order), and the helicopter's two rotors.
    player: [Option<usize>; 4],
    rotors: [Option<usize>; 2],
    /// The player on foot, posed by the export at eight points of its cycles; the cyclist's rider.
    walk: [Option<usize>; 8],
    run: [Option<usize>; 8],
    stand: Option<usize>,
    rider: [Option<usize>; 8],
    /// The sidewalks' people: near and far figures, walking and standing.
    crowd_walk: [Option<usize>; 8],
    crowd_far_walk: [Option<usize>; 8],
    crowd_stand: Option<usize>,
    crowd_far_stand: Option<usize>,
    /// The traffic's car models by the highway's kinds: taxi, sedan, SUV.
    cars: [Option<usize>; 3],
    near: Vec<(f32, u16)>,
    highway: Vec<HighwayPose>,
    /// The walker's place in its gait cycle (0..1).
    gait: f32,
    /// The rotors' angles the frame before drew.
    rotors_shown: [f32; 2],
}

impl Movers {
    /// `model(name)`: the index of the pack's model of that name.
    pub fn new(model: impl Fn(&str) -> Option<usize>) -> Movers {
        let eight = |prefix: &str| core::array::from_fn(|k| model(&format!("{prefix}-{k}")));
        Movers {
            kinds: ["taxi", "sedan", "suv", "nypd", "bus"].map(&model),
            // Near the eye the reference's full cars (it draws them within 90 m, CAR_PROXY_DISTANCE; 4 100 triangles
            // each, so here within 40 m), past that the stand-ins (500), both under the same pictures.
            kinds_near: ["taxi-near", "sedan-near", "suv-near", "nypd-near", "bus"].map(&model),
            player: [None, model("player-car"), model("player-cyclist"), model("player-helicopter")],
            rotors: [model("player-heli-rotor"), model("player-heli-tail")],
            walk: eight("person-walk"),
            run: eight("person-run"),
            stand: model("person-stand"),
            rider: eight("rider"),
            crowd_walk: eight("crowd-walk"),
            crowd_far_walk: eight("crowd-far-walk"),
            crowd_stand: model("crowd-stand"),
            crowd_far_stand: model("crowd-far-stand"),
            cars: ["taxi", "sedan", "suv"].map(&model),
            near: Vec::with_capacity(ny_sim::crowd::NUM_PEDS),
            highway: Vec::new(),
            gait: 0.0,
            rotors_shown: [0.0; 2],
        }
    }

    /// Pushes this frame's instances onto `out`: with `traffic`, the cars in view of `planes` within `reach` of
    /// `eye`, the sidewalks' people within `PEOPLE_REACH` and the highway's cars within `reach`; and always the
    /// player's own. `dt`: seconds since the frame before.
    #[allow(clippy::too_many_arguments)]
    pub fn place(&mut self, game: &Game, eye: V3, planes: &Planes, reach: f32, dt: f32, traffic: bool, out: &mut Vec<(usize, Instance)>) {
        if traffic {
            for v in game.vehicles().iter().filter(|v| v.placed()) {
                let Some(far) = self.kinds[v.kind as usize] else { continue };
                let (dx, dz) = (v.x - eye.x, v.z - eye.z);
                if dx * dx + dz * dz > reach * reach || !ny_view::visible(planes, [v.x - 7.0, -1.0, v.z - 7.0], [v.x + 7.0, 5.0, v.z + 7.0]) {
                    continue;
                }
                let m = if dx * dx + dz * dz < NEAR_CAR * NEAR_CAR { self.kinds_near[v.kind as usize].unwrap_or(far) } else { far };
                // Paint is linear; it multiplies colours that are sRGB.
                out.push((m, placed(v.x, 0.0, v.z, v.yaw, v.pitch, v.roll, display(v.paint))));
            }
            self.crowd(game, eye, planes, reach, out);
        }
        let pl = &game.players;
        match game.mode {
            ny_sim::Mode::Drive => {
                let c = &pl.car;
                if let Some(m) = self.player[1] {
                    out.push((m, placed(c.body.pos.x, c.body.pos.y + c.heave, c.body.pos.z, c.body.heading, c.pitch, c.roll, [1.0; 3])));
                }
            }
            ny_sim::Mode::Cycle => {
                let c = &pl.cyclist;
                let at = placed(c.body.pos.x, c.body.pos.y, c.body.pos.z, c.body.heading, 0.0, c.lean, [1.0; 3]);
                if let Some(m) = self.player[2] {
                    out.push((m, at));
                }
                let k = (crate::wrap(c.pedal_phase / core::f32::consts::TAU, 1.0) * 8.0) as usize % 8;
                if let Some(m) = self.rider[k] {
                    out.push((m, at));
                }
            }
            ny_sim::Mode::Fly => {
                let h = &pl.heli;
                let body = placed(h.body.pos.x, h.body.pos.y + h.bob, h.body.pos.z, h.body.heading, h.pitch, h.roll, [1.0; 3]);
                if let Some(m) = self.player[3] {
                    out.push((m, body));
                }
                for (k, (angle, at, axis)) in [(h.main_rotor, MAIN_ROTOR_AT, 1), (h.tail_rotor, TAIL_ROTOR_AT, 0)].into_iter().enumerate() {
                    let swept = (angle - self.rotors_shown[k]).clamp(0.0, core::f32::consts::FRAC_PI_2);
                    self.rotors_shown[k] = angle;
                    if let Some(m) = self.rotors[k] {
                        for (i, alpha) in TRAIL.into_iter().enumerate() {
                            out.push((m, turned(&body, at, axis, angle - swept * i as f32 / TRAIL.len() as f32, alpha)));
                        }
                    }
                }
            }
            ny_sim::Mode::Walk => {
                // The gait cycle as the reference's rig keeps it: walking, its cadence grows with speed
                // (speed^0.43 / 1.22 cycles a second); running, one and a half cycles a second.
                let p = &pl.person;
                let speed = libm::fabsf(p.body.speed);
                let running = speed > 3.0;
                self.gait = crate::wrap(self.gait + if running { 1.5 } else { libm::powf(speed.max(0.01), 0.43) / 1.22 } * dt, 1.0);
                let k = (self.gait * 8.0) as usize % 8;
                let m = if speed < 0.3 { self.stand } else if running { self.run[k] } else { self.walk[k] };
                if let Some(m) = m {
                    out.push((m, placed(p.body.pos.x, p.body.pos.y, p.body.pos.z, p.body.heading, 0.0, 0.0, [1.0; 3])));
                }
            }
        }
    }

    /// The sidewalks' people in view within `PEOPLE_REACH` of `eye` (the nearest `PEOPLE_MAX`), and the highway's
    /// cars within `reach`.
    fn crowd(&mut self, game: &Game, eye: V3, planes: &Planes, reach: f32, out: &mut Vec<(usize, Instance)>) {
        self.near.clear();
        for (i, p) in game.pedestrians().iter().enumerate() {
            if !p.placed {
                continue;
            }
            let (dx, dz) = (p.x - eye.x, p.z - eye.z);
            let d2 = dx * dx + dz * dz;
            if d2 > PEOPLE_REACH * PEOPLE_REACH || !ny_view::visible(planes, [p.x - 1.0, p.ground(), p.z - 1.0], [p.x + 1.0, p.ground() + 2.0, p.z + 1.0]) {
                continue;
            }
            self.near.push((d2, i as u16));
        }
        self.near.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        for &(d2, i) in self.near.iter().take(PEOPLE_MAX) {
            let p = &game.pedestrians()[i as usize];
            let near = d2 < NEAR_PERSON * NEAR_PERSON;
            let frame = (p.gait * 8.0) as usize % 8;
            let m = match (p.v < STANDING, near) {
                (true, true) => self.crowd_stand,
                (true, false) => self.crowd_far_stand,
                (false, true) => self.crowd_walk[frame],
                (false, false) => self.crowd_far_walk[frame],
            };
            let Some(m) = m else { continue };
            let mut at = placed(p.x, p.ground(), p.z, p.heading(), 0.0, 0.0, display(p.coat));
            for row in &mut at.rows {
                for v in &mut row[..3] {
                    *v *= p.scale;
                }
            }
            out.push((m, at));
        }
        game.highway_cars(&mut self.highway);
        for c in &self.highway {
            let (dx, dz) = (c.x - eye.x, c.z - eye.z);
            if dx * dx + dz * dz > reach * reach || !ny_view::visible(planes, [c.x - 5.0, c.y - 1.0, c.z - 5.0], [c.x + 5.0, c.y + 3.0, c.z + 5.0]) {
                continue;
            }
            let car = &game.highway.cars[c.car as usize];
            let k = match car.kind {
                Kind::Taxi => 0,
                Kind::Sedan => 1,
                _ => 2,
            };
            let Some(m) = self.cars[k] else { continue };
            out.push((m, placed(c.x, c.y, c.z, c.yaw, c.pitch, 0.0, display(car.paint))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_part_turned_by_nothing_stands_at_its_pivot_in_the_whole() {
        let base = placed(10.0, 2.0, -5.0, 0.7, 0.1, -0.2, [1.0, 0.5, 0.25]);
        let part = turned(&base, [0.0, 2.66, -0.35], 1, 0.0, 128);
        for i in 0..3 {
            for j in 0..3 {
                assert!((part.rows[i][j] - base.rows[i][j]).abs() < 1e-6);
            }
            let at = base.rows[i][1] * 2.66 - base.rows[i][2] * 0.35 + base.rows[i][3];
            assert!((part.rows[i][3] - at).abs() < 1e-5);
        }
        assert_eq!(part.tint, [128, 64, 32, 128]);
    }
}
