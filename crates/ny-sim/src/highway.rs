//! The highway's traffic (traffic.js `createHighwayTraffic`): cars in every lane of the FDR Drive and the West Side
//! Highway, placed by the reference's seeded generator, each following the car ahead in its lane (the Intelligent
//! Driver Model with the reference's constants, round its lane as a ring), the player in a lane counting as a
//! leader; drawn within 1 600 m, those behind the camera past the near distance left out, dipping into the lane ends.
//!
//! Places and speeds are kept in doubles, as the reference's numbers are: the oracle (`tests/oracle.rs`) replays its
//! run of every car. Left out: the lamp flares and the headlights' spot lights.

use crate::math::V3;
use crate::traffic::Kind;
use crate::world::World;
use alloc::vec::Vec;
use libm::{atan, atan2, sqrt};

const A: f64 = 1.6;
const B: f64 = 2.8;
const T: f64 = 1.3;
const S0: f64 = 3.0;
pub const CAR_LEN: f64 = 4.6;
pub const NEAR_LOD: f32 = 170.0;
pub const MID_LOD: f32 = 450.0;
pub const FAR_CULL: f32 = 1600.0;
/// The dip at the lane ends: 2.4 m down over this many metres.
const RAMP: f64 = 32.0;

const TAXI: [u32; 1] = [0xffbe14];
const SEDAN: [u32; 7] = [0x1c2026, 0xcfd6de, 0x1d3b6c, 0x52575e, 0x6e1f24, 0x8a8f96, 0xe9ecef];
const SUV: [u32; 6] = [0x16191e, 0xf0f3f7, 0x3a434d, 0x28382e, 0xbec6cf, 0x5b2a2a];

/// sRGB hex to linear, as three.js's `Color` takes a hex.
fn linear_hex(c: u32) -> [f32; 3] {
    let lin = |v: u32| {
        let s = v as f64 / 255.0;
        (if s <= 0.04045 { s / 12.92 } else { libm::pow((s + 0.055) / 1.055, 2.4) }) as f32
    };
    [lin((c >> 16) & 255), lin((c >> 8) & 255), lin(c & 255)]
}

/// A lane: its points (`World::highway_pts`), the arc length at each and the heading there, its bounds on the ground.
pub struct Lane {
    pub first: usize,
    pub count: usize,
    pub speed: f64,
    pub cum: Vec<f64>,
    pub len: f64,
    pub yaw: Vec<f32>,
    pub bounds: [f32; 4],
    /// Its cars, in the order the generator placed them (each follows the next, the last the first).
    pub cars: Vec<u32>,
}

#[derive(Clone, Copy, Debug)]
pub struct HighwayCar {
    pub lane: u16,
    pub kind: Kind,
    /// Its paint, linear.
    pub paint: [f32; 3],
    /// Metres along its lane, metres a second, the acceleration it last chose.
    pub u: f64,
    pub v: f64,
    pub acc: f64,
    /// The segment of its lane it is on.
    pub seg: usize,
}

/// Where a car is drawn: its place, heading (about y) and pitch (about x, `YXZ`), and how near (0 near, 1 mid, 2 far).
#[derive(Clone, Copy, Debug)]
pub struct HighwayPose {
    pub car: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub lod: u8,
    /// Braking (its brake lights on).
    pub brake: bool,
}

/// The player for the highway's drivers: where it is and how it moves.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlayerOnRoad {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub vx: f32,
    pub vz: f32,
}

pub struct Highway {
    pub lanes: Vec<Lane>,
    pub cars: Vec<HighwayCar>,
}

fn ease(x: f64) -> f64 {
    let t = (x / RAMP).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn ease_slope(x: f64) -> f64 {
    if x <= 0.0 || x >= RAMP {
        return 0.0;
    }
    let t = x / RAMP;
    (6.0 * t * (1.0 - t)) / RAMP
}

impl Highway {
    /// The reference's lanes and fleet (`createHighwayTraffic`), from its seed.
    pub fn new(world: &World) -> Highway {
        let mut seed: f64 = 20240917.0;
        let mut rand = || {
            seed = (seed * 16807.0) % 2147483647.0;
            seed / 2147483647.0
        };
        let mut lanes = Vec::with_capacity(world.highway.len());
        let mut cars = Vec::new();
        for (li, l) in world.highway.iter().enumerate() {
            let pts = &world.highway_pts[l.first as usize..(l.first + l.count) as usize];
            let n = pts.len();
            let mut cum = Vec::with_capacity(n);
            cum.push(0.0f64);
            for i in 1..n {
                let (a, b) = (pts[i - 1], pts[i]);
                let d = sqrt((b[0] as f64 - a[0] as f64).powi(2) + (b[1] as f64 - a[1] as f64).powi(2) + (b[2] as f64 - a[2] as f64).powi(2));
                cum.push(cum[i - 1] + d);
            }
            let len = cum[n.saturating_sub(1)];
            let yaw = (0..n)
                .map(|i| {
                    let (a, b) = (pts[i.saturating_sub(1)], pts[(i + 1).min(n - 1)]);
                    atan2(b[0] as f64 - a[0] as f64, b[2] as f64 - a[2] as f64) as f32
                })
                .collect();
            let mut bounds = [f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY];
            for p in pts {
                bounds = [bounds[0].min(p[0]), bounds[1].max(p[0]), bounds[2].min(p[2]), bounds[3].max(p[2])];
            }
            let speed = l.speed as f64;
            let mut lane_cars = Vec::new();
            let mut u = rand() * 30.0;
            while u < len - 12.0 {
                let h = rand();
                let (kind, palette): (Kind, &[u32]) = if h < 0.32 {
                    (Kind::Taxi, &TAXI)
                } else if h < 0.72 {
                    (Kind::Sedan, &SEDAN)
                } else {
                    (Kind::Suv, &SUV)
                };
                let paint = linear_hex(palette[(rand() * palette.len() as f64) as usize % palette.len()]);
                lane_cars.push(cars.len() as u32);
                cars.push(HighwayCar { lane: li as u16, kind, paint, u, v: speed, acc: 0.0, seg: 0 });
                u += 24.0 + rand() * 50.0;
            }
            lanes.push(Lane { first: l.first as usize, count: n, speed, cum, len, yaw, bounds, cars: lane_cars });
        }
        Highway { lanes, cars }
    }

    /// The player's place and speed along `lane` if it is in it (within 2 m of its centre line and near its height,
    /// not on its stretches underground), as the reference's `playerInLane`.
    fn player_in_lane(world: &World, lane: &Lane, p: &PlayerOnRoad) -> Option<(f64, f64)> {
        let [x0, x1, z0, z1] = lane.bounds;
        if p.x < x0 - 3.0 || p.x > x1 + 3.0 || p.z < z0 - 3.0 || p.z > z1 + 3.0 {
            return None;
        }
        let pts = &world.highway_pts[lane.first..lane.first + lane.count];
        for i in 0..pts.len().saturating_sub(1) {
            let (a, b) = (pts[i], pts[i + 1]);
            if a[1] < -1.0 && b[1] < -1.0 {
                continue;
            }
            let (ax, az) = (a[0] as f64, a[2] as f64);
            let (sx, sz) = (b[0] as f64 - ax, b[2] as f64 - az);
            let h2 = sx * sx + sz * sz;
            if h2 == 0.0 {
                continue;
            }
            let (px, pz) = (p.x as f64, p.z as f64);
            let len = lane.cum[i + 1] - lane.cum[i];
            let t = ((px - ax) * sx + (pz - az) * sz) / h2;
            if !(0.0..=1.0).contains(&t) {
                continue;
            }
            let y = a[1] as f64 + (b[1] as f64 - a[1] as f64) * t;
            if (p.y as f64 - y).abs() >= 2.6 {
                continue;
            }
            if ((pz - az) * sx - (px - ax) * sz).abs() / sqrt(h2) < 2.0 {
                return Some((lane.cum[i] + t * len, (((p.vx as f64 * sx + p.vz as f64 * sz) / h2) * len).max(0.0)));
            }
        }
        None
    }

    /// One frame of every lane's traffic (`update`, step 1), and every car's segment (step 2's walk along its lane).
    pub fn step(&mut self, world: &World, dt: f32, player: Option<&PlayerOnRoad>) {
        let dt = dt as f64;
        let brake_term = 2.0 * sqrt(A * B);
        for li in 0..self.lanes.len() {
            let p_in = player.and_then(|p| Self::player_in_lane(world, &self.lanes[li], p));
            let lane = &self.lanes[li];
            let n = lane.cars.len();
            for k in 0..n {
                let c = self.cars[lane.cars[k] as usize];
                let mut gap = f64::INFINITY;
                let mut dv = 0.0;
                if n > 1 {
                    let lead = self.cars[lane.cars[(k + 1) % n] as usize];
                    gap = lead.u - c.u;
                    if gap < 0.0 {
                        gap += lane.len;
                    }
                    gap -= CAR_LEN;
                    dv = c.v - lead.v;
                }
                if let Some((pu, pv)) = p_in {
                    if pu > c.u && pu - c.u - CAR_LEN < gap {
                        gap = pu - c.u - CAR_LEN;
                        dv = c.v - pv;
                    }
                }
                let s_star = S0 + (c.v * T + (c.v * dv) / brake_term).max(0.0);
                let free = (c.v / lane.speed).powi(4);
                let acc = (A * (1.0 - free - (s_star / gap.max(0.1)).powi(2))).max(-9.0);
                self.cars[lane.cars[k] as usize].acc = acc;
            }
            for &ci in &lane.cars {
                let c = &mut self.cars[ci as usize];
                c.v = (c.v + c.acc * dt).max(0.0);
                c.u += c.v * dt;
                if c.u >= lane.len {
                    c.u -= lane.len;
                    c.seg = 0;
                }
            }
        }
        for c in &mut self.cars {
            let cum = &self.lanes[c.lane as usize].cum;
            while c.seg + 2 < cum.len() && cum[c.seg + 1] < c.u {
                c.seg += 1;
            }
        }
    }

    /// Where car `i` is drawn: place, heading, pitch (`update`, step 2), and how far into its lane's end dip.
    pub fn pose(&self, world: &World, i: usize) -> (V3, f32, f32, f64) {
        let c = &self.cars[i];
        let lane = &self.lanes[c.lane as usize];
        let s = c.seg;
        let seg_len = lane.cum[s + 1] - lane.cum[s];
        let t = ((c.u - lane.cum[s]) / seg_len).clamp(0.0, 1.0);
        let (a, b) = (world.highway_pts[lane.first + s], world.highway_pts[lane.first + s + 1]);
        let lerp = |k: usize| a[k] as f64 + (b[k] as f64 - a[k] as f64) * t;
        let to_end = lane.len - c.u;
        let dip = if c.u < to_end { c.u } else { to_end };
        let y = lerp(1) - 2.4 * (1.0 - ease(dip));
        let slope = (b[1] as f64 - a[1] as f64) / seg_len + 2.4 * ease_slope(dip) * if c.u < to_end { 1.0 } else { -1.0 };
        let tau = core::f64::consts::PI * 2.0;
        let mut d_yaw = lane.yaw[s + 1] as f64 - lane.yaw[s] as f64;
        if d_yaw > core::f64::consts::PI {
            d_yaw -= tau;
        } else if d_yaw < -core::f64::consts::PI {
            d_yaw += tau;
        }
        let yaw = lane.yaw[s] as f64 + d_yaw * t;
        (V3 { x: lerp(0) as f32, y: y as f32, z: lerp(2) as f32 }, yaw as f32, -atan(slope) as f32, dip)
    }

    /// The cars the camera at `eye`, looking along `ahead`, draws (`update`, step 2): within `FAR_CULL`, those past
    /// the near distance not well behind it, none in the Battery Park Underpass; in the reference's order.
    pub fn drawn(&self, world: &World, eye: V3, ahead: V3, out: &mut Vec<HighwayPose>) {
        out.clear();
        // (a car further than this from its segment's start is out of reach wherever along it: the lanes' longest
        // segment is under 300 m)
        let reach = (FAR_CULL + 300.0) * (FAR_CULL + 300.0);
        for i in 0..self.cars.len() {
            let c = &self.cars[i];
            let a = world.highway_pts[self.lanes[c.lane as usize].first + c.seg];
            let (ax, az) = (a[0] - eye.x, a[2] - eye.z);
            if ax * ax + az * az > reach {
                continue;
            }
            let (p, yaw, pitch, _) = self.pose(world, i);
            let (dx, dy, dz) = (p.x - eye.x, p.y - eye.y, p.z - eye.z);
            let d2 = dx * dx + dy * dy + dz * dz;
            if d2 > FAR_CULL * FAR_CULL {
                continue;
            }
            let lod = if d2 < NEAR_LOD * NEAR_LOD { 0 } else if d2 < MID_LOD * MID_LOD { 1 } else { 2 };
            if lod > 0 && dx * ahead.x + dz * ahead.z < -40.0 {
                continue;
            }
            if p.y < -3.6 {
                continue;
            }
            out.push(HighwayPose { car: i as u32, x: p.x, y: p.y, z: p.z, yaw, pitch, lod, brake: self.cars[i].acc < -1.2 });
        }
    }
}
