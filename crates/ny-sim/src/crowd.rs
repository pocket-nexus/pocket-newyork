//! The sidewalks' pedestrians (traffic.js, its section 7 and the crowd of section 3): 340 people, each with the
//! build, coat and pace the reference gives them, who stroll a stretch of sidewalk beside a street, turning round at
//! its ends, or (one in six) walk back and forth over a signalized crosswalk, stepping off the curb on the WALK when
//! no vehicle is crossing or about to, and queueing at the curb behind whoever waits there. Those far from the
//! camera and out of its sight are put back around it, as the reference does.
//!
//! Where a track may lie is the reference's own test (`sidewalkTrack`, `crosswalkTrack`), recorded per edge and per
//! intersection by the export (`ny_pack::sim::WalkEdge`, `WalkNode`) at the steps of the offsets and lines it draws
//! from `Math.random`; this port draws from those steps. The walking itself is the reference's (`walk`), checked
//! against its run (`tests/oracle.rs`). The people are drawn from a few baked poses of one figure (`Pedestrian::gait`),
//! not from a rig of their own.

use crate::signal::Timing;
use crate::traffic::{Sight, Traffic, Walker};
use crate::world::World;
use alloc::vec::Vec;
use libm::{atan2f, floor, hypotf, sin, sqrtf};
use ny_pack::sim::{LINE_STEP, LINE_STEPS, OFF_STEP, OFF_STEPS};

pub const NUM_PEDS: usize = 340;
const PED_RADIUS: f32 = 320.0;
const NEAR_RADIUS: f32 = 350.0;
/// How far along a crosswalk track pedestrians wait to cross (the curb is 2 m along).
const KERB: f32 = 1.6;
/// Metres from one person standing in a queue to the next.
const QUEUE_GAP: f32 = 0.62;
const PED_ACCEL: f32 = 1.3;
const PED_BRAKE: f32 = 1.6;
/// Seconds turning round at the end of the track.
const TURN_TIME: f32 = 0.85;
/// Sidewalks stand this far above the road, crosswalks this far.
pub const SIDEWALK_Y: f32 = 0.22;
pub const CROSSWALK_Y: f32 = 0.012;

/// The coats of pedestrians.js (`COAT_COLORS`, `COAT_COLORS_F`).
const COATS: [u32; 8] = [0x2b3a52, 0x33332f, 0x8a6a44, 0x5a2f2f, 0x4f5238, 0x6a6a66, 0x222024, 0x7d3f22];
const COATS_F: [u32; 8] = [0x7a2630, 0xb89a72, 0x2e3e5c, 0x1f1d22, 0xc9c1b2, 0x5b6b4a, 0x8c5a7a, 0x3c3c40];

/// `fract(sin(i × 12.9898 + salt × 78.233) × 43758.5453)`, as the reference hashes a person's index.
fn hash(i: usize, salt: f64) -> f64 {
    let v = sin(i as f64 * 12.9898 + salt * 78.233) * 43758.5453;
    v - floor(v)
}

fn linear_hex(c: u32) -> [f32; 3] {
    let lin = |v: u32| {
        let s = v as f64 / 255.0;
        (if s <= 0.04045 { s / 12.92 } else { libm::pow((s + 0.055) / 1.055, 2.4) }) as f32
    };
    [lin((c >> 16) & 255), lin((c >> 8) & 255), lin(c & 255)]
}

/// A track: where it starts, its direction and length; on a crosswalk, its intersection (or -1), side and line.
#[derive(Clone, Copy, Debug, Default)]
pub struct Track {
    pub ax: f32,
    pub az: f32,
    pub ux: f32,
    pub uz: f32,
    pub len: f32,
    pub node: i32,
    pub side: u8,
    pub line: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Pedestrian {
    pub placed: bool,
    /// Walks back and forth over a signalized crosswalk.
    pub crosses: bool,
    pub track: Track,
    /// Metres along the track, and which way (1 or -1).
    pub t: f32,
    pub dir: f32,
    pub x: f32,
    pub z: f32,
    pub coord: f32,
    pub waiting: bool,
    /// Preferred speed, speed now, seconds left turning round.
    pub speed: f32,
    pub v: f32,
    pub pause: f32,
    /// The build and dress the reference gives the person: female, height against the baked figure's, coat (linear).
    pub female: bool,
    pub scale: f32,
    pub coat: [f32; 3],
    /// Where in its walking cycle (0..1), as its legs would be.
    pub gait: f32,
}

impl Pedestrian {
    /// The person `i` of the reference's crowd, not yet placed.
    pub fn new(i: usize) -> Pedestrian {
        let female = hash(i, 1.0) < 0.48;
        // The rig's seed picks the dress (pedestrians.js `lane(29)`): fract(sin((seed + 29) × 12.9898) × 43758.5453)
        let seed = i as f64 * 1.6180339 + 0.13;
        let lane = {
            let v = sin((seed + 29.0) * 12.9898) * 43758.5453;
            v - floor(v)
        };
        let palette = if female { &COATS_F } else { &COATS };
        let coat = linear_hex(palette[((lane * 8.0) as usize).min(7)]);
        Pedestrian {
            crosses: i.is_multiple_of(6),
            dir: 1.0,
            speed: (1.22 + (i % 5) as f64 * 0.1 + hash(i, 3.0) * 0.08) as f32,
            female,
            scale: ((if female { 0.91 } else { 0.97 }) + hash(i, 4.0) * 0.1) as f32,
            coat,
            ..Default::default()
        }
    }

    /// m along its way over the crosswalk (the curb it waits at is `KERB`).
    fn progress(&self) -> f32 {
        if self.dir > 0.0 {
            self.t
        } else {
            self.track.len - self.t
        }
    }

    /// Which way it faces: the reference's heading (0 faces +z).
    pub fn heading(&self) -> f32 {
        atan2f(self.track.ux * self.dir, self.track.uz * self.dir)
    }

    /// The height it walks at: the sidewalk, or the road over the middle of a crosswalk (`pedGround`).
    pub fn ground(&self) -> f32 {
        if self.track.node >= 0 && self.t > 2.0 && self.t < self.track.len - 2.0 {
            CROSSWALK_Y
        } else {
            SIDEWALK_Y
        }
    }
}

pub struct Crowd {
    pub peds: Vec<Pedestrian>,
    /// Who waits at (or is just short of) each crosswalk's curb: (node, person).
    curb: Vec<(u32, u16)>,
    /// The edge arriving over each crosswalk: (node, side) → edge (`n.approach`).
    approach: Vec<(u32, u8, u32)>,
    near_at: [f32; 2],
    near_edges: Vec<u32>,
    near_signals: Vec<u32>,
    last_focus: [f32; 2],
    timing: Timing,
    ave_walk: bool,
    st_walk: bool,
    rng: u32,
}

/// xorshift32 to 0..1, for what the reference draws from `Math.random`.
fn next(s: &mut u32) -> f32 {
    let mut x = *s;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *s = x;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

impl Crowd {
    pub fn new(world: &World) -> Crowd {
        let mut approach = Vec::new();
        if !world.walk_edges.is_empty() {
            for (i, e) in world.edges.iter().enumerate() {
                let side = world.walk_edges[i].in_side;
                // (the last edge over a side wins, as the reference's loop leaves it)
                if let Some(a) = approach.iter_mut().find(|a: &&mut (u32, u8, u32)| a.0 == e.to && a.1 == side) {
                    a.2 = i as u32;
                } else {
                    approach.push((e.to, side, i as u32));
                }
            }
            approach.sort_unstable_by_key(|a| (a.0, a.1));
        }
        Crowd {
            peds: (0..NUM_PEDS).map(Pedestrian::new).collect(),
            curb: Vec::new(),
            approach,
            near_at: [f32::INFINITY; 2],
            near_edges: Vec::new(),
            near_signals: Vec::new(),
            last_focus: [f32::INFINITY; 2],
            timing: Timing(world.info.signal),
            ave_walk: true,
            st_walk: false,
            rng: 0x7065_6473,
        }
    }

    fn rand(&mut self) -> f32 {
        next(&mut self.rng)
    }

    fn approach_of(&self, node: u32, side: u8) -> Option<u32> {
        let i = self.approach.binary_search_by_key(&(node, side), |a| (a.0, a.1)).ok()?;
        Some(self.approach[i].2)
    }

    /// Along the sidewalk on either side of edge `e`, between its crosswalks (`sidewalkTrack`).
    fn sidewalk_track(&mut self, world: &World, e: u32) -> Option<Track> {
        let (ee, we) = (&world.edges[e as usize], &world.walk_edges[e as usize]);
        let right = self.rand() < 0.5;
        let r = self.rand();
        let k = ((r * 2.6 / OFF_STEP).round() as u32).min(OFF_STEPS - 1);
        if we.walk[if right { 0 } else { 1 }] & (1u64 << k) == 0 {
            return None;
        }
        let off = (if right { 1.0 } else { -1.0 }) * (we.half + 1.4 + k as f32 * OFF_STEP);
        Some(Track { ax: ee.sx + ee.rx * off, az: ee.sz + ee.rz * off, ux: ee.dx, uz: ee.dz, len: ee.len, node: -1, side: 0, line: 0.0 })
    }

    /// Over one of the crosswalks of intersection `n`, curb to curb plus 2 m of sidewalk at each end, on its own line
    /// (`crosswalkTrack`).
    fn crosswalk_track(&mut self, world: &World, n: u32) -> Option<Track> {
        let side = ((self.rand() * 4.0) as usize).min(3);
        let wn = &world.walk_nodes[n as usize];
        if wn.road[side] < 0 {
            return None;
        }
        let node = &world.nodes[n as usize];
        let across_avenue = side >= 2;
        let sign = if side.is_multiple_of(2) { 1.0 } else { -1.0 };
        let own = across_avenue || self.rand() < 0.5;
        let r = self.rand();
        let k = ((r * 0.55 / LINE_STEP).round() as u32).min(LINE_STEPS - 1);
        let bit = if own { k } else { LINE_STEPS + k };
        if wn.lines[side] & (1 << bit) == 0 {
            return None;
        }
        let line = (if own { sign } else { -sign }) * (0.5 + k as f32 * LINE_STEP);
        let (across, half) = (wn.across[side], wn.half[side]);
        let (ax, az) = if across_avenue { (across - half - 2.0, node.z + sign * (node.hz + 1.25) + line) } else { (node.x + sign * (node.hx + 1.25) + line, across - half - 2.0) };
        let (ux, uz) = if across_avenue { (1.0, 0.0) } else { (0.0, 1.0) };
        Some(Track { ax, az, ux, uz, len: 2.0 * half + 4.0, node: n as i32, side: side as u8, line })
    }

    /// Puts person `i` on a random sidewalk or crosswalk `r_min`..`r_max` metres from `focus`, out of view if
    /// `hidden` (`spawnPed`).
    #[allow(clippy::too_many_arguments)]
    fn spawn(&mut self, world: &World, i: usize, r_min: f32, r_max: f32, hidden: bool, focus: [f32; 2], sight: &Sight) -> bool {
        for _ in 0..10 {
            let crosswalk = self.peds[i].crosses && !self.near_signals.is_empty();
            let track = if crosswalk {
                let r = self.rand();
                let n = self.near_signals[((r * self.near_signals.len() as f32) as usize).min(self.near_signals.len() - 1)];
                self.crosswalk_track(world, n)
            } else if !self.near_edges.is_empty() {
                let r = self.rand();
                let e = self.near_edges[((r * self.near_edges.len() as f32) as usize).min(self.near_edges.len() - 1)];
                self.sidewalk_track(world, e)
            } else {
                None
            };
            let Some(track) = track else { continue };
            let t = if crosswalk {
                if self.rand() < 0.5 {
                    self.rand() * KERB
                } else {
                    track.len - self.rand() * KERB
                }
            } else {
                self.rand() * track.len
            };
            let (x, z) = (track.ax + track.ux * t, track.az + track.uz * t);
            let d = hypotf(x - focus[0], z - focus[1]);
            if d < r_min || d > r_max || (hidden && sight.sees(x, z, 2.0)) {
                continue;
            }
            if crosswalk {
                let n = track.node as u32;
                if self.curb.iter().any(|&(cn, q)| cn == n && hypotf(self.peds[q as usize].x - x, self.peds[q as usize].z - z) < QUEUE_GAP) {
                    continue;
                }
            }
            let p = &mut self.peds[i];
            p.track = track;
            p.t = t;
            p.dir = if t < track.len * 0.5 { 1.0 } else { -1.0 };
            p.x = x;
            p.z = z;
            p.v = if crosswalk { 0.0 } else { p.speed };
            p.pause = 0.0;
            p.placed = true;
            if crosswalk {
                self.at_curb(i);
            }
            return true;
        }
        false
    }

    fn at_curb(&mut self, i: usize) {
        let p = &self.peds[i];
        if p.progress() > KERB + 0.5 {
            return;
        }
        self.curb.push((p.track.node as u32, i as u16));
    }

    fn walk_on(&self, side: u8) -> bool {
        if side >= 2 {
            self.st_walk
        } else {
            self.ave_walk
        }
    }

    /// May person `i` step off the curb onto its crosswalk? On the WALK, once no vehicle is crossing it or committed
    /// to, and none is too close to stop (`mayCross`).
    fn may_cross(&self, world: &World, traffic: &Traffic, i: usize) -> bool {
        let p = &self.peds[i];
        let (n, side) = (p.track.node as u32, p.track.side);
        if !self.walk_on(side) || traffic.crossing_at(n, side) {
            return false;
        }
        match self.approach_of(n, side) {
            Some(e) => traffic.approach_can_stop(world, e),
            None => true,
        }
    }

    /// Walks person `i` along its track (`walk`): easing to a stop at its ends and turning round on the spot; on a
    /// crosswalk, slowing to a stop at the curb until it may cross, or behind whoever stands ahead in its line.
    fn walk(&mut self, world: &World, traffic: &mut Traffic, i: usize, dt: f32) {
        let p = self.peds[i];
        let crossing = p.track.node >= 0;
        let (may, room) = if crossing {
            let may = self.may_cross(world, traffic, i);
            // room to walk before standing: up to the curb unless it may cross, and up to the back of whoever is ahead
            let s = p.progress();
            let mut room = if s <= KERB && !may { KERB - s } else { f32::INFINITY };
            for &(n, qi) in &self.curb {
                let q = &self.peds[qi as usize];
                if n != p.track.node as u32 || qi as usize == i || q.track.node != p.track.node || q.track.side != p.track.side || q.dir != p.dir || (q.track.line - p.track.line).abs() > 0.55 {
                    continue;
                }
                let gap = q.progress() - s;
                if gap > 0.0 {
                    room = room.min((gap - QUEUE_GAP).max(0.0));
                }
            }
            (may, room)
        } else {
            (true, f32::INFINITY)
        };
        let walk_on = crossing && self.walk_on(p.track.side);
        let q = &mut self.peds[i];
        Self::advance(q, dt, crossing, may, room);
        let len = q.track.len;
        if crossing && ((q.t > KERB && q.t < len - KERB) || (q.waiting && walk_on)) {
            q.coord = if q.track.side >= 2 { q.x } else { q.z };
            traffic.walkers.push(Walker { node: q.track.node as u32, side: q.track.side, coord: q.coord, dir: q.dir, speed: q.speed, waiting: q.waiting });
        }
        // The legs' cycle: walking, it quickens with speed (speed^0.43 / 1.22 cycles a second, as the player's rig)
        if q.v > 0.05 {
            q.gait = (q.gait + libm::powf(q.v, 0.43) / 1.22 * dt).rem_euclid(1.0);
        }
    }

    /// The step along the track itself (`walk`), given whether it may step off the curb (`may`) and the room before
    /// whoever stands ahead (`room`).
    pub fn advance(p: &mut Pedestrian, dt: f32, crossing: bool, may: bool, room: f32) {
        if p.pause > 0.0 {
            p.pause = (p.pause - dt).max(0.0);
        }
        let ahead = if p.dir > 0.0 { p.track.len - p.t } else { p.t };
        let mut v_max = if p.pause > 0.0 { 0.0 } else { p.speed.min(sqrtf(2.0 * PED_BRAKE * ahead)) };
        if crossing {
            v_max = v_max.min(sqrtf(2.0 * PED_BRAKE * room));
        }
        p.v += (-2.0 * PED_BRAKE * dt).max((PED_ACCEL * dt).min(v_max - p.v));
        let mut t = p.t + p.dir * p.v * dt;
        let len = p.track.len;
        let steps_off = crossing && if p.dir > 0.0 { p.t <= KERB && t > KERB } else { p.t >= len - KERB && t < len - KERB };
        p.waiting = steps_off && !may;
        if p.waiting {
            t = p.t;
            p.v = 0.0;
        }
        if if p.dir > 0.0 { t >= len } else { t <= 0.0 } {
            t = if p.dir > 0.0 { len } else { 0.0 };
            p.dir = -p.dir;
            p.v = 0.0;
            p.pause = TURN_TIME;
        }
        p.t = t;
        p.x = p.track.ax + p.track.ux * t;
        p.z = p.track.az + p.track.uz * t;
    }

    /// One frame (traffic.js `update`, step 2), after the vehicles': the WALK at `clock`, the people around `focus`,
    /// what the camera sees. Refills `traffic.walkers` for the vehicles' next frame.
    pub fn step(&mut self, world: &World, traffic: &mut Traffic, dt: f32, clock: f32, focus: [f32; 2], sight: &Sight) {
        if world.walk_edges.is_empty() {
            return;
        }
        self.ave_walk = self.timing.walk(clock, true);
        self.st_walk = self.timing.walk(clock, false);
        let jump = hypotf(focus[0] - self.last_focus[0], focus[1] - self.last_focus[1]) > 140.0;
        self.last_focus = focus;
        if hypotf(focus[0] - self.near_at[0], focus[1] - self.near_at[1]) >= 40.0 {
            self.near_at = focus;
            self.near_edges.clear();
            self.near_signals.clear();
            for (i, e) in world.edges.iter().enumerate() {
                if hypotf(e.sx + e.dx * e.len * 0.5 - focus[0], e.sz + e.dz * e.len * 0.5 - focus[1]) < NEAR_RADIUS {
                    self.near_edges.push(i as u32);
                }
            }
            for (i, n) in world.nodes.iter().enumerate() {
                if n.signal != 0 && hypotf(n.x - focus[0], n.z - focus[1]) < NEAR_RADIUS {
                    self.near_signals.push(i as u32);
                }
            }
        }
        traffic.walkers.clear();
        self.curb.clear();
        for i in 0..self.peds.len() {
            if self.peds[i].placed && self.peds[i].track.node >= 0 {
                self.at_curb(i);
            }
        }
        let mut budget = if jump { usize::MAX } else { 6 };
        for i in 0..self.peds.len() {
            let p = self.peds[i];
            let d = if p.placed { hypotf(p.x - focus[0], p.z - focus[1]) } else { f32::INFINITY };
            if !p.placed || (jump && d > NEAR_RADIUS) {
                self.spawn(world, i, 0.0, NEAR_RADIUS, false, focus, sight);
            } else if budget > 0 && (d > 1000.0 || (d > PED_RADIUS && !sight.sees(p.x, p.z, 2.0))) && self.spawn(world, i, 60.0, 260.0, true, focus, sight) {
                budget -= 1;
            }
            if !self.peds[i].placed {
                continue;
            }
            self.walk(world, traffic, i, dt);
        }
    }
}
