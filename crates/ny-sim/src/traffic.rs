//! Traffic on the reference's road network (traffic.js and road-network.js), as much of it as a handheld affords:
//! its fleet (200 cars of four liveries and 32 buses), its lanes and the curves that join them through an
//! intersection, its drivers (the Intelligent Driver Model with the reference's constants), its signals, its rule
//! that a car enters an intersection only when it can clear it and no crossing road holds the box, and its habit of
//! moving cars only where the camera cannot see them.
//!
//! Left out: lane changes (a car keeps its lane and turns where its lane may), pedestrians and the crosswalks they
//! hold, the buses' tail swing, the yielding of a left turn to a queue across, sparks. A car stops for the player
//! in its lane; the player is pushed off a car it drives into.

use crate::math::*;
use crate::signal::{Phase, Timing};
use crate::world::World;
use alloc::vec::Vec;
use libm::{atan2f, cosf, fabsf, hypotf, sinf, sqrtf};
use ny_pack::sim::{edge, turn, TrafficEdge};

pub const NUM_CARS: usize = 200;
pub const NUM_BUSES: usize = 32;
const LOOKAHEAD: f32 = 70.0;
const VEHICLE_RADIUS: f32 = 380.0;
const NEAR_RADIUS: f32 = 350.0;

/// What a device draws a vehicle as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Taxi,
    Sedan,
    Suv,
    /// A sedan in NYPD livery with its light bar.
    Nypd,
    Bus,
}

#[derive(Clone, Copy, Debug)]
pub struct Driver {
    pub a: f32,
    pub b: f32,
    pub t: f32,
    pub s0: f32,
    pub a_lat: f32,
}

const CAR_DRIVER: Driver = Driver { a: 2.0, b: 3.0, t: 1.2, s0: 2.0, a_lat: 3.5 };
const BUS_DRIVER: Driver = Driver { a: 1.1, b: 2.5, t: 1.5, s0: 2.5, a_lat: 2.0 };

const TAXI: u32 = 0xffbe14;
const NYPD: u32 = 0xf6f9fc;
const SEDANS: [u32; 5] = [0x1c2026, 0xcfd6de, 0x1d3b6c, 0x52575e, 0x6e1f24];
const SUVS: [u32; 5] = [0x16191e, 0xf0f3f7, 0x3a434d, 0x28382e, 0xbec6cf];
/// Buses carry no paint of their own: their livery is the model's (`createLoftedBusMaterial`).
const BUS: u32 = 0xffffff;

fn srgb_hex(c: u32) -> [f32; 3] {
    let lin = |v: u32| {
        let s = v as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            libm::powf((s + 0.055) / 1.055, 2.4)
        }
    };
    [lin((c >> 16) & 255), lin((c >> 8) & 255), lin(c & 255)]
}

/// A cubic Bézier from the end of one lane to the start of another, with its arc-length table and its sharpest
/// curvature (`makeCurve`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Curve {
    p: [[f32; 2]; 4],
    lut: [f32; 17],
    pub len: f32,
    pub curv: f32,
}

impl Curve {
    fn new(e: &TrafficEdge, lane_e: f32, f: &TrafficEdge, lane_f: f32) -> Curve {
        let x0 = e.sx + e.dx * e.len + e.rx * lane_e;
        let z0 = e.sz + e.dz * e.len + e.rz * lane_e;
        let x3 = f.sx + f.rx * lane_f;
        let z3 = f.sz + f.rz * lane_f;
        let cross = e.dx * f.dz - e.dz * f.dx;
        let (k0, k3) = if fabsf(cross) < 0.5 {
            let k = hypotf(x3 - x0, z3 - z0) / 3.0;
            (k, k)
        } else {
            let t = ((x3 - x0) * f.dz - (z3 - z0) * f.dx) / cross;
            (0.552 * fabsf(t), 0.552 * hypotf(x3 - (x0 + e.dx * t), z3 - (z0 + e.dz * t)))
        };
        let p = [[x0, z0], [x0 + e.dx * k0, z0 + e.dz * k0], [x3 - f.dx * k3, z3 - f.dz * k3], [x3, z3]];
        let mut c = Curve { p, lut: [0.0; 17], len: 0.0, curv: 0.0 };
        let (mut px, mut pz) = (x0, z0);
        for k in 0..=16 {
            let t = k as f32 / 16.0;
            let (x, z, dx, dz) = c.at_t(t);
            c.len += hypotf(x - px, z - pz);
            c.lut[k] = c.len;
            px = x;
            pz = z;
            let u = 1.0 - t;
            let ddx = 6.0 * u * (p[2][0] - 2.0 * p[1][0] + p[0][0]) + 6.0 * t * (p[3][0] - 2.0 * p[2][0] + p[1][0]);
            let ddz = 6.0 * u * (p[2][1] - 2.0 * p[1][1] + p[0][1]) + 6.0 * t * (p[3][1] - 2.0 * p[2][1] + p[1][1]);
            let l = hypotf(dx, dz).max(1e-6);
            c.curv = c.curv.max(fabsf(dx * ddz - dz * ddx) / (l * l * l).max(1e-6));
        }
        c
    }

    /// Point and (unnormalized) tangent at parameter t.
    fn at_t(&self, t: f32) -> (f32, f32, f32, f32) {
        let p = &self.p;
        let u = 1.0 - t;
        let x = u * u * u * p[0][0] + 3.0 * u * u * t * p[1][0] + 3.0 * u * t * t * p[2][0] + t * t * t * p[3][0];
        let z = u * u * u * p[0][1] + 3.0 * u * u * t * p[1][1] + 3.0 * u * t * t * p[2][1] + t * t * t * p[3][1];
        let dx = 3.0 * u * u * (p[1][0] - p[0][0]) + 6.0 * u * t * (p[2][0] - p[1][0]) + 3.0 * t * t * (p[3][0] - p[2][0]);
        let dz = 3.0 * u * u * (p[1][1] - p[0][1]) + 6.0 * u * t * (p[2][1] - p[1][1]) + 3.0 * t * t * (p[3][1] - p[2][1]);
        (x, z, dx, dz)
    }

    /// Position and unit tangent at arc length `s` (`curvePoint`).
    fn point(&self, s: f32) -> [f32; 4] {
        let mut k = 1;
        while k < 16 && self.lut[k] < s {
            k += 1;
        }
        let (s0, s1) = (self.lut[k - 1], self.lut[k]);
        let span = if s1 - s0 == 0.0 { 1.0 } else { s1 - s0 };
        let t = (k as f32 - 1.0 + ((s - s0) / span).clamp(0.0, 1.0)) / 16.0;
        let (x, z, dx, dz) = self.at_t(t);
        let l = hypotf(dx, dz);
        let l = if l == 0.0 { 1.0 } else { l };
        [x, z, dx / l, dz / l]
    }
}

/// A car or a bus. What a device draws: `kind`, `paint` (linear), `pos`, `yaw` (the reference's heading),
/// `pitch` and `roll`, `steer`, `wheel_spin`, `braking`, `blink` (-1 left, 1 right) and whether it is placed.
#[derive(Clone, Copy, Debug)]
pub struct Vehicle {
    pub kind: Kind,
    pub paint: [f32; 3],
    pub half: f32,
    pub half_width: f32,
    axle: f32,
    driver: Driver,
    v0: f32,
    edge: i32,
    lane: usize,
    s: f32,
    pub speed: f32,
    acc: f32,
    mov: i32,
    out: usize,
    conn: Option<Curve>,
    cs: f32,
    trail: Option<Curve>,
    committed: bool,
    gap: f32,
    leader_speed: f32,
    wait: f32,
    pub x: f32,
    pub z: f32,
    prev_x: f32,
    prev_z: f32,
    tx: f32,
    tz: f32,
    pub yaw: f32,
    yaw_rate: f32,
    pub steer: f32,
    pub pitch: f32,
    pub roll: f32,
    pitch_vel: f32,
    roll_vel: f32,
    pub blink: i8,
    blink_phase: f32,
    pub wheel_spin: f32,
}

impl Vehicle {
    fn new(kind: Kind, paint: u32, half: f32, half_width: f32, axle: f32, driver: Driver, v0: f32, phase: f32) -> Vehicle {
        Vehicle {
            kind,
            paint: srgb_hex(paint),
            half,
            half_width,
            axle,
            driver,
            v0,
            edge: -1,
            lane: 0,
            s: 0.0,
            speed: 0.0,
            acc: 0.0,
            mov: -1,
            out: 0,
            conn: None,
            cs: 0.0,
            trail: None,
            committed: false,
            gap: f32::INFINITY,
            leader_speed: 0.0,
            wait: 0.0,
            x: 0.0,
            z: 0.0,
            prev_x: 0.0,
            prev_z: 0.0,
            tx: 0.0,
            tz: 1.0,
            yaw: 0.0,
            yaw_rate: 0.0,
            steer: 0.0,
            pitch: 0.0,
            roll: 0.0,
            pitch_vel: 0.0,
            roll_vel: 0.0,
            blink: 0,
            blink_phase: phase,
            wheel_spin: 0.0,
        }
    }

    pub fn placed(&self) -> bool {
        self.edge >= 0
    }

    /// Where it is on the network: edge, lane, position along it, and the connector position when it crosses an
    /// intersection (for tests and tools).
    pub fn on(&self) -> (i32, usize, f32, Option<f32>, i32, usize) {
        (self.edge, self.lane, self.s, self.conn.map(|_| self.cs), self.mov, self.out)
    }

    pub fn is_bus(&self) -> bool {
        self.kind == Kind::Bus
    }

    /// Brake lights (`writeLampState`).
    pub fn braking(&self) -> bool {
        self.acc < -0.8 || self.speed < 0.3
    }

    /// The turn signal lit this moment: -1 left, 1 right, 0 none.
    pub fn signal_lit(&self, clock: f32) -> i8 {
        if self.blink != 0 && (clock + self.blink_phase) % 0.8 <= 0.4 {
            self.blink
        } else {
            0
        }
    }

    fn free_acc(&self) -> f32 {
        let r = self.speed / self.v0;
        self.driver.a * (1.0 - r * r * r * r)
    }

    fn brake(&self, gap: f32, speed: f32, s0: f32, t: f32) -> f32 {
        let (a, b) = (self.driver.a, self.driver.b);
        let desired = s0 + (self.speed * t + self.speed * (self.speed - speed) / (2.0 * sqrtf(a * b))).max(0.0);
        let r = desired / gap.max(0.1);
        -a * r * r
    }

    fn stop_at(&self, d: f32, s0: f32) -> f32 {
        self.brake(d, 0.0, s0, 0.0)
    }

    fn can_stop_before(&self, d: f32) -> bool {
        self.free_acc() + self.stop_at(d, 1.0) > -self.driver.b
    }
}

/// The player as the traffic sees it.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlayerTraffic {
    pub active: bool,
    pub x: f32,
    pub z: f32,
    pub vx: f32,
    pub vz: f32,
    pub half_w: f32,
    pub half_l: f32,
}

/// The camera, for what it can see: eye, unit direction, half the horizontal field of view.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sight {
    pub eye: V3,
    pub dir: V3,
    pub half_fov: f32,
}

impl Sight {
    /// Could the camera see a sphere of radius `r` about ground point (x, z)? (the frustum's side planes)
    pub fn sees(&self, x: f32, z: f32, r: f32) -> bool {
        let (dx, dz) = (x - self.eye.x, z - self.eye.z);
        let d = hypotf(dx, dz);
        if d <= r {
            return true;
        }
        let fl = hypotf(self.dir.x, self.dir.z);
        if fl < 1e-3 {
            return true; // looking straight down: everything near is in view
        }
        let cos_a = (dx * self.dir.x + dz * self.dir.z) / (d * fl);
        let a = libm::acosf(cos_a.clamp(-1.0, 1.0));
        a < self.half_fov + libm::asinf((r / d).min(1.0))
    }
}

#[derive(Clone, Copy)]
struct Entry {
    edge: u32,
    lane: u32,
    s: f32,
    v: u16,
}

pub struct Traffic {
    pub vehicles: Vec<Vehicle>,
    pub clock: f32,
    timing: Timing,
    ave: Phase,
    st: Phase,
    focus: [f32; 2],
    last_focus: [f32; 2],
    near_at: [f32; 2],
    near_edges: Vec<u32>,
    occ: Vec<Entry>,
    /// Intersections whose box a road holds this frame: (node, group).
    boxes: Vec<(u32, u32)>,
    rng: u32,
}

impl Traffic {
    /// The reference's fleet (`createTrafficSystem`): its liveries by the same rules, not yet placed.
    pub fn new(world: &World) -> Traffic {
        let mut vehicles = Vec::with_capacity(NUM_CARS + NUM_BUSES);
        let mut rng = 0x6e79_7466u32;
        for i in 0..NUM_CARS {
            let nypd = i % 10 == 6;
            let taxi = !nypd && i % 7 < 4;
            let suv = !nypd && !taxi && i % 2 == 0;
            let (kind, paint) = if taxi {
                (Kind::Taxi, TAXI)
            } else if suv {
                (Kind::Suv, SUVS[i % SUVS.len()])
            } else if nypd {
                (Kind::Nypd, NYPD)
            } else {
                (Kind::Sedan, SEDANS[i % SEDANS.len()])
            };
            let phase = next(&mut rng) * 0.8;
            vehicles.push(Vehicle::new(kind, paint, if suv { 2.3 } else { 2.25 }, if suv { 0.92 } else { 0.86 }, if suv { 1.4 } else { 1.38 }, CAR_DRIVER, 10.0 + (i % 5) as f32 * 0.9, phase));
        }
        for i in 0..NUM_BUSES {
            let phase = next(&mut rng) * 0.8;
            vehicles.push(Vehicle::new(Kind::Bus, BUS, 6.0, 1.3, 3.6, BUS_DRIVER, 8.0 + (i % 3) as f32, phase));
        }
        Traffic {
            vehicles,
            clock: 0.0,
            timing: Timing(world.info.signal),
            ave: Phase::Green,
            st: Phase::Red,
            focus: [0.0, 140.0],
            last_focus: [f32::INFINITY, f32::INFINITY],
            near_at: [f32::INFINITY, f32::INFINITY],
            near_edges: Vec::new(),
            occ: Vec::new(),
            boxes: Vec::new(),
            rng,
        }
    }

    fn phase_of(&self, e: &TrafficEdge) -> Phase {
        if e.group == 0 {
            self.ave
        } else {
            self.st
        }
    }

    fn rand(&mut self) -> f32 {
        next(&mut self.rng)
    }

    fn lane_offset(world: &World, e: &TrafficEdge, i: usize) -> f32 {
        world.lanes[e.lane_first as usize + i.min(e.lane_count.saturating_sub(1) as usize)]
    }

    fn out_of(world: &World, mov: i32, lane: usize) -> i32 {
        if mov < 0 {
            return -1;
        }
        let m = &world.moves[mov as usize];
        world.outs.get(m.out_first as usize + lane).copied().unwrap_or(-1) as i32
    }

    fn connector(world: &World, e: u32, i: usize, f: u32, j: usize) -> Curve {
        let (ee, ff) = (&world.edges[e as usize], &world.edges[f as usize]);
        Curve::new(ee, Self::lane_offset(world, ee, i), ff, Self::lane_offset(world, ff, j))
    }

    /// Where to go at the end of the edge: mostly straight on (buses keep to wide roads), only the moves its lane
    /// can make, drifting towards the camera when far from it (`chooseMove(v, true)`).
    fn choose_move(&mut self, world: &World, v: usize) -> i32 {
        let car = self.vehicles[v];
        let e = &world.edges[car.edge as usize];
        let to_focus = hypotf(self.focus[0] - car.x, self.focus[1] - car.z) > 250.0;
        let mut weights = [0.0f32; 16];
        let n = (e.move_count as usize).min(16);
        let mut total = 0.0;
        for (k, w) in weights.iter_mut().enumerate().take(n) {
            let mi = e.move_first as usize + k;
            let m = &world.moves[mi];
            let f = &world.edges[m.to as usize];
            let mut wt = if m.turn == turn::STRAIGHT { if car.is_bus() { 8.0 } else { 3.0 } } else { 1.0 };
            if car.is_bus() && m.turn != turn::STRAIGHT && f.lane_count < 2 {
                wt *= 0.05;
            }
            if Self::out_of(world, mi as i32, car.lane) < 0 {
                wt = 0.0;
            }
            if to_focus && f.dx * (self.focus[0] - car.x) + f.dz * (self.focus[1] - car.z) > 0.0 {
                wt *= 3.0;
            }
            *w = wt;
            total += wt;
        }
        if total <= 0.0 {
            return if n > 0 { e.move_first as i32 } else { -1 };
        }
        let mut r = self.rand() * total;
        let mut pick = -1;
        for (k, w) in weights.iter().enumerate().take(n) {
            if *w <= 0.0 {
                continue;
            }
            pick = (e.move_first as usize + k) as i32;
            r -= w;
            if r < 0.0 {
                break;
            }
        }
        pick
    }

    fn build_occupancy(&mut self, world: &World) {
        self.occ.clear();
        self.boxes.clear();
        for (i, v) in self.vehicles.iter().enumerate() {
            if v.edge < 0 {
                continue;
            }
            let e = &world.edges[v.edge as usize];
            match &v.conn {
                None => {
                    self.occ.push(Entry { edge: v.edge as u32, lane: v.lane as u32, s: v.s, v: i as u16 });
                    if v.committed && v.mov >= 0 {
                        let f = world.moves[v.mov as usize].to;
                        let c = Self::connector(world, v.edge as u32, v.lane, f, v.out);
                        self.occ.push(Entry { edge: f, lane: v.out as u32, s: v.s - e.len - c.len, v: i as u16 });
                        self.boxes.push((e.to, e.group));
                    }
                }
                Some(c) => {
                    self.occ.push(Entry { edge: v.edge as u32, lane: v.lane as u32, s: e.len + v.cs, v: i as u16 });
                    let f = world.moves[v.mov as usize].to;
                    self.occ.push(Entry { edge: f, lane: v.out as u32, s: v.cs - c.len, v: i as u16 });
                    self.boxes.push((e.to, e.group));
                }
            }
        }
        self.occ.sort_by(|a, b| (a.edge, a.lane).cmp(&(b.edge, b.lane)).then(a.s.partial_cmp(&b.s).unwrap_or(core::cmp::Ordering::Equal)));
    }

    fn lane_list(&self, edge: u32, lane: u32) -> &[Entry] {
        let from = self.occ.partition_point(|o| (o.edge, o.lane) < (edge, lane));
        let to = self.occ.partition_point(|o| (o.edge, o.lane) <= (edge, lane));
        &self.occ[from..to]
    }

    /// Braking for the vehicle ahead of `s` in a lane; remembers the nearest (`follow`).
    fn follow(&mut self, v: usize, edge: u32, lane: u32, s: f32) -> f32 {
        let ahead = self.lane_list(edge, lane).iter().find(|o| o.s > s && o.v as usize != v).copied();
        let Some(o) = ahead else { return 0.0 };
        let other = self.vehicles[o.v as usize];
        let me = &mut self.vehicles[v];
        let gap = o.s - other.half - s - me.half;
        if gap > LOOKAHEAD {
            return 0.0;
        }
        if gap < me.gap {
            me.gap = gap;
            me.leader_speed = other.speed;
        }
        me.brake(gap, other.speed, me.driver.s0, me.driver.t)
    }

    fn signal_ahead(&self, world: &World, v: &Vehicle, f: u32, d: f32) -> f32 {
        let e = &world.edges[f as usize];
        if world.nodes[e.to as usize].signal == 0 || d > LOOKAHEAD {
            return 0.0;
        }
        let phase = self.phase_of(e);
        if phase == Phase::Green || (phase == Phase::Amber && !v.can_stop_before(d)) {
            return 0.0;
        }
        v.stop_at(d, 1.0)
    }

    /// May v cross the intersection ahead (`mayEnter`, without the pedestrians and the queue across)?
    fn may_enter(&mut self, world: &World, v: usize, d: f32) -> bool {
        let car = self.vehicles[v];
        let e = world.edges[car.edge as usize];
        let n = &world.nodes[e.to as usize];
        let phase = if n.signal != 0 { self.phase_of(&e) } else { Phase::Green };
        let stoppable = car.can_stop_before(d) || (phase != Phase::Green && car.speed * car.speed < 2.0 * car.driver.b * d);
        if stoppable {
            self.vehicles[v].committed = false;
        }
        if self.vehicles[v].committed {
            return true;
        }
        let mut j = Self::out_of(world, car.mov, car.lane);
        if j < 0 {
            let m = self.choose_move(world, v);
            self.vehicles[v].mov = m;
            j = Self::out_of(world, m, car.lane);
            if j < 0 {
                return false;
            }
        }
        let mov = self.vehicles[v].mov;
        let m = world.moves[mov as usize];
        if phase == Phase::Red || (phase == Phase::Amber && stoppable) {
            return false;
        }
        if self.boxes.iter().any(|&(node, g)| node == e.to && g != e.group) {
            return false;
        }
        // A left turn waits for oncoming traffic that comes on (unless it has waited long)
        if m.turn == turn::LEFT && e.opposite >= 0 && self.vehicles[v].wait < 8.0 {
            let o = &world.edges[e.opposite as usize];
            for lane in 0..o.lane_count {
                if let Some(x) = self.lane_list(e.opposite as u32, lane).last() {
                    let ov = &self.vehicles[x.v as usize];
                    let left = ov.mov >= 0 && world.moves[ov.mov as usize].turn == turn::LEFT;
                    let dist = o.len - x.s - ov.half;
                    if !left && ov.speed > 0.5 && dist < (ov.speed * ov.speed) / 5.0 + ov.speed + 3.0 + 25.0 {
                        return false;
                    }
                }
            }
        }
        // A free place to clear the intersection to
        let f = &world.edges[m.to as usize];
        let mut free = f.len - 1.0;
        for o in self.lane_list(m.to, j as u32) {
            let ov = &self.vehicles[o.v as usize];
            if o.v as usize != v && o.s - ov.half < f.len {
                free -= 2.0 * ov.half + ov.driver.s0;
            }
        }
        if free < (2.0 * car.half + 0.5).min(f.len - 1.0) {
            return false;
        }
        let me = &mut self.vehicles[v];
        me.out = j as usize;
        if !stoppable {
            me.committed = true;
            self.boxes.push((e.to, e.group));
        }
        true
    }

    fn accelerate(&mut self, world: &World, v: usize, player: &PlayerTraffic) {
        let mut worst;
        let mut cap = f32::INFINITY;
        self.vehicles[v].gap = f32::INFINITY;
        let car = self.vehicles[v];
        let e = world.edges[car.edge as usize];
        match car.conn {
            None => {
                worst = self.follow(v, car.edge as u32, car.lane as u32, car.s);
                let d = e.len - car.s - car.half;
                if d < LOOKAHEAD {
                    let go = self.may_enter(world, v, d);
                    let car = self.vehicles[v];
                    let j = Self::out_of(world, car.mov, car.lane);
                    let f = if car.mov >= 0 { world.moves[car.mov as usize].to } else { car.edge as u32 };
                    let c = if j >= 0 { Some(Self::connector(world, car.edge as u32, car.lane, f, j as usize)) } else { None };
                    let vs = c.map(|c| car.s - e.len - c.len).unwrap_or(0.0);
                    if !go {
                        worst = worst.min(car.stop_at(d, 1.0));
                    } else if j >= 0 {
                        let ff = &world.edges[f as usize];
                        let fol = self.follow(v, f, j as u32, vs);
                        let car = self.vehicles[v];
                        worst = worst.min(fol).min(self.signal_ahead(world, &car, f, ff.len - vs - car.half));
                    }
                    let car = self.vehicles[v];
                    let mut vt = c.map(|c| sqrtf(car.driver.a_lat / c.curv.max(1e-3))).unwrap_or(f32::INFINITY);
                    if world.nodes[e.to as usize].signal == 0 && e.group != 0 {
                        vt = vt.min(5.0);
                    }
                    cap = sqrtf(vt * vt + 5.0 * d.max(0.0));
                }
            }
            Some(c) => {
                let f = world.moves[car.mov as usize].to;
                let ff = &world.edges[f as usize];
                let vs = car.cs - c.len;
                let a = self.follow(v, f, car.out as u32, vs);
                let b = self.follow(v, car.edge as u32, car.lane as u32, e.len + car.cs);
                let car = self.vehicles[v];
                worst = a.min(b).min(self.signal_ahead(world, &car, f, ff.len - vs - car.half));
                cap = sqrtf(car.driver.a_lat / c.curv.max(1e-3));
            }
        }
        // The player ahead in its lane
        let car = &mut self.vehicles[v];
        if player.active {
            let (dx, dz) = (player.x - car.x, player.z - car.z);
            if fabsf(dx) <= LOOKAHEAD + 10.0 && fabsf(dz) <= LOOKAHEAD + 10.0 {
                let (fx, fz) = (sinf(car.yaw), cosf(car.yaw));
                let ahead = dx * fx + dz * fz;
                let right = dz * fx - dx * fz;
                let pspeed = (player.vx * fx + player.vz * fz).max(0.0);
                if (0.0..=LOOKAHEAD).contains(&ahead) && fabsf(right) <= car.half_width + player.half_w + 0.4 {
                    let pg = ahead - car.half - player.half_l;
                    worst = worst.min(car.brake(pg, pspeed, car.driver.s0, car.driver.t));
                    if pg < car.gap {
                        car.gap = pg;
                        car.leader_speed = pspeed;
                    }
                }
            }
        }
        let acc = (car.free_acc() + worst).min((cap - car.speed) * 2.0);
        car.acc = acc.clamp(-9.0, car.driver.a);
    }

    fn enter(&mut self, world: &World, v: usize) {
        if !self.may_enter(world, v, 0.0) {
            let car = self.vehicles[v];
            let j = Self::out_of(world, car.mov, car.lane);
            if car.speed < 4.0 || j < 0 {
                let e = &world.edges[car.edge as usize];
                let me = &mut self.vehicles[v];
                me.s = e.len - me.half;
                me.speed = 0.0;
                return;
            }
            self.vehicles[v].out = j as usize;
        }
        let car = self.vehicles[v];
        let e = &world.edges[car.edge as usize];
        let f = world.moves[car.mov as usize].to;
        let c = Self::connector(world, car.edge as u32, car.lane, f, car.out);
        let me = &mut self.vehicles[v];
        me.cs = me.s - e.len;
        me.conn = Some(c);
        self.boxes.push((e.to, e.group));
    }

    fn leave(&mut self, world: &World, v: usize) {
        let me = &mut self.vehicles[v];
        let c = me.conn.take().unwrap();
        me.s = me.cs - c.len;
        me.edge = world.moves[me.mov as usize].to as i32;
        me.lane = me.out;
        me.trail = Some(c);
        me.committed = false;
        let m = self.choose_move(world, v);
        self.vehicles[v].mov = m;
    }

    fn drive(&mut self, world: &World, v: usize, dt: f32, player: &PlayerTraffic) {
        self.accelerate(world, v, player);
        let me = &mut self.vehicles[v];
        let speed = me.speed;
        me.speed = (me.speed + me.acc * dt).max(0.0);
        let mut ds = me.speed * dt;
        if ds > me.gap - 0.3 {
            ds = (me.gap - 0.3).max(0.0);
            me.speed = me.speed.min(me.leader_speed);
        }
        me.acc = ((me.speed - speed) / dt).max(-9.0);
        me.wait = if me.speed < 0.5 { me.wait + dt } else { 0.0 };
        let e = &world.edges[me.edge as usize];
        match me.conn {
            None => {
                me.s += ds;
                if me.s + me.half >= e.len {
                    self.enter(world, v);
                }
            }
            Some(c) => {
                me.cs += ds;
                if me.cs >= c.len {
                    self.leave(world, v);
                }
            }
        }
    }

    fn lane_point(world: &World, e: u32, i: usize, s: f32) -> [f32; 4] {
        let ee = &world.edges[e as usize];
        let o = Self::lane_offset(world, ee, i);
        [ee.sx + ee.dx * s + ee.rx * o, ee.sz + ee.dz * s + ee.rz * o, ee.dx, ee.dz]
    }

    fn path_point(world: &World, v: &Vehicle, c: f32) -> [f32; 4] {
        let e = &world.edges[v.edge as usize];
        if let Some(conn) = &v.conn {
            if c < 0.0 {
                return Self::lane_point(world, v.edge as u32, v.lane, e.len + c);
            }
            if c > conn.len {
                return Self::lane_point(world, world.moves[v.mov as usize].to, v.out, c - conn.len);
            }
            return conn.point(c);
        }
        if c < 0.0 {
            if let Some(t) = &v.trail {
                return t.point(t.len + c);
            }
        }
        Self::lane_point(world, v.edge as u32, v.lane, c)
    }

    /// Position and heading from where the axles are along the path (`locate`, on the lane's centre).
    fn locate(world: &World, v: &mut Vehicle) {
        let c = if v.conn.is_some() { v.cs } else { v.s };
        let front = Self::path_point(world, v, c + v.axle);
        let rear = Self::path_point(world, v, c - v.axle);
        let l = hypotf(front[0] - rear[0], front[1] - rear[1]);
        let l = if l == 0.0 { 1.0 } else { l };
        v.tx = (front[0] - rear[0]) / l;
        v.tz = (front[1] - rear[1]) / l;
        v.x = (front[0] + rear[0]) / 2.0;
        v.z = (front[1] + rear[1]) / 2.0;
    }

    /// Heading, steering, turn signals, body pitch and roll (`pose`).
    fn pose(world: &World, v: &mut Vehicle, dt: f32) {
        let yaw = atan2f(v.tx, v.tz);
        let tau = core::f32::consts::PI * 2.0;
        let mut d = yaw - v.yaw;
        d -= libm::floorf(d / tau + 0.5) * tau;
        v.yaw = yaw;
        v.yaw_rate += (d / dt - v.yaw_rate) * (dt * 8.0).min(1.0);
        let steer = libm::atanf((2.0 * v.axle * v.yaw_rate) / v.speed.max(0.5)).clamp(-0.6, 0.6);
        v.steer += (steer - v.steer) * (dt * 10.0).min(1.0);
        let e = &world.edges[v.edge as usize];
        let t = if v.mov >= 0 { world.moves[v.mov as usize].turn } else { turn::STRAIGHT };
        v.blink = if t != turn::STRAIGHT && (v.conn.is_some() || e.len - v.s < 35.0) { if t == turn::RIGHT { 1 } else { -1 } } else { 0 };
        v.pitch_vel += ((-0.008 * v.acc - v.pitch) * 68.0 - v.pitch_vel * 10.8) * dt;
        v.pitch = (v.pitch + v.pitch_vel * dt).clamp(-0.22, 0.22);
        v.roll_vel += ((0.01 * v.speed * v.yaw_rate - v.roll) * 68.0 - v.roll_vel * 10.8) * dt;
        v.roll = (v.roll + v.roll_vel * dt).clamp(-0.26, 0.26);
        let radius = if v.is_bus() { 0.52 } else { 0.35 };
        let fwd = (v.x - v.prev_x) * sinf(v.yaw) + (v.z - v.prev_z) * cosf(v.yaw);
        let raw = fwd / radius;
        v.wheel_spin = (v.wheel_spin + if raw < 0.0 { -1.0 } else { 1.0 } * fabsf(raw).min(0.48)) % tau;
    }

    fn lane_clear(&self, edge: u32, lane: u32, s: f32, margin: f32) -> bool {
        self.lane_list(edge, lane).iter().all(|o| {
            let ov = &self.vehicles[o.v as usize];
            fabsf(o.s - s) >= ov.half + margin + if o.s < s { 2.0 * ov.speed } else { 0.0 }
        })
    }

    fn place(&mut self, world: &World, v: usize, e: u32, lane: usize, s: f32) {
        {
            let me = &mut self.vehicles[v];
            me.edge = e as i32;
            me.lane = lane;
            me.s = s;
            me.conn = None;
            me.trail = None;
            me.committed = false;
            me.wait = 0.0;
            me.speed = me.v0 * 0.5;
            me.acc = 0.0;
            me.pitch = 0.0;
            me.pitch_vel = 0.0;
            me.roll = 0.0;
            me.roll_vel = 0.0;
            Self::locate(world, me);
            me.prev_x = me.x;
            me.prev_z = me.z;
            me.yaw = atan2f(me.tx, me.tz);
            me.yaw_rate = 0.0;
            me.steer = 0.0;
        }
        let m = self.choose_move(world, v);
        self.vehicles[v].mov = m;
        self.occ.push(Entry { edge: e, lane: lane as u32, s, v: v as u16 });
        self.occ.sort_by(|a, b| (a.edge, a.lane).cmp(&(b.edge, b.lane)).then(a.s.partial_cmp(&b.s).unwrap_or(core::cmp::Ordering::Equal)));
    }

    /// A free spot of a random lane `r_min` .. `r_max` metres from the focus, out of view if `hidden`
    /// (`spawnVehicle`).
    fn spawn(&mut self, world: &World, v: usize, r_min: f32, r_max: f32, hidden: bool, sight: &Sight, player: &PlayerTraffic) -> bool {
        for _ in 0..12 {
            if self.near_edges.is_empty() {
                return false;
            }
            let k = (self.rand() * self.near_edges.len() as f32) as usize % self.near_edges.len();
            let ei = self.near_edges[k];
            let e = world.edges[ei as usize];
            let car = self.vehicles[v];
            if (car.is_bus() && e.flags & edge::AVE == 0) || e.len < 2.0 * car.half + 10.0 || e.lane_count == 0 || e.move_count == 0 {
                continue;
            }
            let lane = (self.rand() * e.lane_count as f32) as usize % e.lane_count as usize;
            let s = car.half + 5.0 + self.rand() * (e.len - 2.0 * car.half - 10.0);
            let o = Self::lane_offset(world, &e, lane);
            let (x, z) = (e.sx + e.dx * s + e.rx * o, e.sz + e.dz * s + e.rz * o);
            let d = hypotf(x - self.focus[0], z - self.focus[1]);
            if d < r_min || d > r_max || (hidden && sight.sees(x, z, car.half + 4.0)) {
                continue;
            }
            if player.active && hypotf(x - player.x, z - player.z) < 25.0 {
                continue;
            }
            if !self.lane_clear(ei, lane as u32, s, car.half + 8.0) {
                continue;
            }
            self.place(world, v, ei, lane, s);
            return true;
        }
        false
    }

    /// One frame (`update`): the signals at `clock`, the cars around `focus`, the player, what the camera sees.
    pub fn step(&mut self, world: &World, dt: f32, clock: f32, focus: [f32; 2], sight: &Sight, player: &PlayerTraffic) {
        if world.edges.is_empty() {
            return;
        }
        self.clock = clock;
        self.ave = self.timing.phase(clock, true);
        self.st = self.timing.phase(clock, false);
        self.focus = focus;
        let jump = hypotf(focus[0] - self.last_focus[0], focus[1] - self.last_focus[1]) > 140.0;
        self.last_focus = focus;
        if hypotf(focus[0] - self.near_at[0], focus[1] - self.near_at[1]) >= 40.0 {
            self.near_at = focus;
            self.near_edges.clear();
            for (i, e) in world.edges.iter().enumerate() {
                if hypotf(e.sx + e.dx * e.len * 0.5 - focus[0], e.sz + e.dz * e.len * 0.5 - focus[1]) < NEAR_RADIUS {
                    self.near_edges.push(i as u32);
                }
            }
        }
        self.build_occupancy(world);
        let mut budget = if jump { usize::MAX } else { 4 };
        for i in 0..self.vehicles.len() {
            let v = self.vehicles[i];
            let d = if v.placed() { hypotf(v.x - focus[0], v.z - focus[1]) } else { f32::INFINITY };
            if !v.placed() || (jump && d > NEAR_RADIUS) {
                self.spawn(world, i, 0.0, NEAR_RADIUS, false, sight, player);
            } else if budget > 0 && (d > 1200.0 || (d > VEHICLE_RADIUS && !sight.sees(v.x, v.z, v.half + 2.0))) && self.spawn(world, i, 100.0, 300.0, true, sight, player) {
                budget -= 1;
            }
            if !self.vehicles[i].placed() || self.vehicles[i].mov < 0 {
                continue;
            }
            {
                let me = &mut self.vehicles[i];
                me.prev_x = me.x;
                me.prev_z = me.z;
            }
            self.drive(world, i, dt, player);
            let me = &mut self.vehicles[i];
            Self::locate(world, me);
            Self::pose(world, me, dt);
        }
    }

    /// Pushes a player body out of every vehicle it overlaps: `half_w`, `half_l` and its heading make its box.
    /// Returns the push (x, z) and how hard it was struck (metres a second against the push).
    pub fn push_player(&self, x: &mut f32, z: &mut f32, heading: f32, half_w: f32, half_l: f32) -> Option<(f32, f32)> {
        let mut pushed = None;
        for v in &self.vehicles {
            if !v.placed() || hypotf(v.x - *x, v.z - *z) > v.half + half_l + 2.0 {
                continue;
            }
            // Separating axes of the two boxes on the ground
            let axes = [(sinf(v.yaw), cosf(v.yaw)), (cosf(v.yaw), -sinf(v.yaw)), (sinf(heading), cosf(heading)), (cosf(heading), -sinf(heading))];
            let (dx, dz) = (*x - v.x, *z - v.z);
            let mut best = (f32::INFINITY, 0.0, 0.0);
            let mut apart = false;
            for (ax, az) in axes {
                let rv = v.half * fabsf(sinf(v.yaw) * ax + cosf(v.yaw) * az) + v.half_width * fabsf(cosf(v.yaw) * ax - sinf(v.yaw) * az);
                let rp = half_l * fabsf(sinf(heading) * ax + cosf(heading) * az) + half_w * fabsf(cosf(heading) * ax - sinf(heading) * az);
                let dist = dx * ax + dz * az;
                let overlap = rv + rp - fabsf(dist);
                if overlap <= 0.0 {
                    apart = true;
                    break;
                }
                if overlap < best.0 {
                    let s = if dist < 0.0 { -1.0 } else { 1.0 };
                    best = (overlap, ax * s, az * s);
                }
            }
            if apart {
                continue;
            }
            *x += best.1 * best.0;
            *z += best.2 * best.0;
            pushed = Some((best.1, best.2));
        }
        pushed
    }

    /// The headlights' strength at an interior glow (`beamScale`): off by day, on at dusk and by night.
    pub fn beams(interior_glow: f32) -> f32 {
        ((interior_glow - 0.60) * 1.45).clamp(0.0, 1.28)
    }
}

/// xorshift32, to 0..1: the reference's traffic draws from `Math.random`; this one repeats run to run.
fn next(s: &mut u32) -> f32 {
    let mut x = *s;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *s = x;
    (x >> 8) as f32 / (1u32 << 24) as f32
}
