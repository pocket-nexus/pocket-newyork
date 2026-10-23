//! What else moves, drawn from the game each frame: the sidewalks' pedestrians (`ny_sim::crowd`) as the crowd's baked
//! figures (`crowd-walk-*`, `crowd-far-walk-*`, `crowd-stand`), each at its own height and in its own coat (the
//! figures' coats are paint), and the highway's cars (`ny_sim::highway`) as the traffic's car models in their paint.

use ny_pack::Instance;
use ny_sim::highway::HighwayPose;
use ny_sim::math::V3;
use ny_sim::traffic::Kind;
use ny_sim::Game;

use crate::city::{self, CityGpu};

/// People nearer than this take the near figure; none is drawn past `PEOPLE_REACH`, nor more than `PEOPLE_MAX` of
/// them (the nearest). The far figure is 954 triangles, the near one 3 434.
pub const NEAR_PERSON: f32 = 15.0;
pub const PEOPLE_REACH: f32 = 70.0;
pub const PEOPLE_MAX: usize = 24;
/// Below this speed a person stands.
const STANDING: f32 = 0.3;

/// A linear colour as sRGB: what multiplies a probed colour (the tint's encoding, as the traffic's paint).
fn srgb(c: [f32; 3]) -> [f32; 3] {
    c.map(|x| if x <= 0.0031308 { x * 12.92 } else { 1.055 * libm::powf(x, 1.0 / 2.4) - 0.055 })
}

pub struct Crowd {
    walk: [Option<usize>; 8],
    far_walk: [Option<usize>; 8],
    stand: Option<usize>,
    far_stand: Option<usize>,
    /// The traffic's car models by the highway's kinds: taxi, sedan, SUV.
    cars: [Option<usize>; 3],
    near: Vec<(f32, u16)>,
    highway: Vec<HighwayPose>,
}

impl Crowd {
    pub fn new(city: &CityGpu) -> Crowd {
        Crowd {
            walk: core::array::from_fn(|k| city.model(&format!("crowd-walk-{k}"))),
            far_walk: core::array::from_fn(|k| city.model(&format!("crowd-far-walk-{k}"))),
            stand: city.model("crowd-stand"),
            far_stand: city.model("crowd-far-stand"),
            cars: ["taxi", "sedan", "suv"].map(|n| city.model(n)),
            near: Vec::with_capacity(ny_sim::crowd::NUM_PEDS),
            highway: Vec::new(),
        }
    }

    /// Pushes this frame's people and highway cars in view of `planes` onto `placed`: people within `PEOPLE_REACH`
    /// of `eye`, cars within `reach`.
    pub fn place(&mut self, game: &Game, eye: V3, planes: &[[f32; 4]; 6], reach: f32, placed: &mut Vec<(usize, Instance)>) {
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
                (true, true) => self.stand,
                (true, false) => self.far_stand,
                (false, true) => self.walk[frame],
                (false, false) => self.far_walk[frame],
            };
            let Some(m) = m else { continue };
            let mut at = city::placed(p.x, p.ground(), p.z, p.heading(), 0.0, 0.0, srgb(p.coat));
            for row in &mut at.rows {
                for v in &mut row[..3] {
                    *v *= p.scale;
                }
            }
            placed.push((m, at));
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
            placed.push((m, city::placed(c.x, c.y, c.z, c.yaw, c.pitch, 0.0, srgb(car.paint))));
        }
    }
}
