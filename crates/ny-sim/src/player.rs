//! The four ways through the city, as the reference's `ThirdPersonControllerManager` moves them (controllers.js):
//! a pedestrian, the yellow taxi, a cyclist and the sightseeing helicopter. Each function below is the reference's
//! of the same name, statement for statement; the numbers are its numbers. Headings follow the reference: a
//! heading `h` faces `(sin h, cos h)` in x and z, 0 faces south (+z), and turning left raises it.

use crate::math::*;
use crate::world::World;
use crate::Mode;
use alloc::vec::Vec;
use libm::{cosf, expf, fabsf, hypotf, sinf};

/// The reference's keys, each 0 (up) to 1 (held); a pad's stick gives values between.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Keys {
    pub forward: f32,
    pub backward: f32,
    pub left: f32,
    pub right: f32,
    pub up: f32,
    pub down: f32,
    pub boost: bool,
}

/// What every entity has.
#[derive(Clone, Copy, Debug, Default)]
pub struct Body {
    pub pos: V3,
    pub heading: f32,
    pub speed: f32,
    pub chase_distance: f32,
    pub chase_height: f32,
    /// `cameraTargetOffset.y`.
    pub target_y: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Person {
    pub body: Body,
    pub vertical_vel: f32,
    /// Metres walked, for the walk cycle a device draws.
    pub stride: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Car {
    pub body: Body,
    pub vx: f32,
    pub vz: f32,
    pub angular_vel: f32,
    pub steer: f32,
    /// The chassis: pitch and roll (radians) and heave (metres), and their rates.
    pub pitch: f32,
    pub roll: f32,
    pub heave: f32,
    pub pitch_vel: f32,
    pub roll_vel: f32,
    pub heave_vel: f32,
    pub wheel_spin: f32,
    /// The brake lights (`carState.x`).
    pub braking: bool,
    /// The last knock against a building fast enough to throw sparks: x, z, the push's normal, its speed.
    pub spark: Option<[f32; 5]>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Cyclist {
    pub body: Body,
    pub lean: f32,
    pub pedal_phase: f32,
    /// The wheels' turn (`frontWheel.rotation.x`).
    pub wheel: f32,
}

/// Where the helicopter's rotors turn in its frame (`mainRotorPivot` about its y, `tailRotorPivot` about its x), and
/// how fast, in radians a second.
pub const MAIN_ROTOR_AT: [f32; 3] = [0.0, 2.66, -0.35];
pub const TAIL_ROTOR_AT: [f32; 3] = [0.18, 2.02, -5.62];
pub const MAIN_ROTOR_SPEED: f32 = 24.0;
pub const TAIL_ROTOR_SPEED: f32 = 32.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct Heli {
    pub body: Body,
    pub vertical_speed: f32,
    pub pitch: f32,
    pub roll: f32,
    pub main_rotor: f32,
    pub tail_rotor: f32,
    /// The hover's bob (`tiltPivot.position.y`).
    pub bob: f32,
}

/// The four entities and which one the player moves.
#[derive(Clone, Debug, Default)]
pub struct Players {
    pub active: Mode,
    pub keys: Keys,
    pub person: Person,
    pub car: Car,
    pub cyclist: Cyclist,
    pub heli: Heli,
    near: Vec<u32>,
}

const SPAWN: [Mode; 4] = [Mode::Walk, Mode::Drive, Mode::Cycle, Mode::Fly];

impl Players {
    /// The entities where the reference builds them (`ENTS`), the taxi active.
    pub fn new(world: &World) -> Players {
        let mut p = Players { active: Mode::Drive, ..Default::default() };
        for (k, mode) in SPAWN.iter().enumerate() {
            let e = &world.entities[k];
            let b = Body { pos: v3(e.pos[0], e.pos[1], e.pos[2]), heading: e.heading, speed: 0.0, chase_distance: e.chase_distance, chase_height: e.chase_height, target_y: e.target_y };
            *p.body_mut(*mode) = b;
        }
        p
    }

    pub fn body(&self, mode: Mode) -> &Body {
        match mode {
            Mode::Walk => &self.person.body,
            Mode::Drive => &self.car.body,
            Mode::Cycle => &self.cyclist.body,
            Mode::Fly => &self.heli.body,
        }
    }

    pub fn body_mut(&mut self, mode: Mode) -> &mut Body {
        match mode {
            Mode::Walk => &mut self.person.body,
            Mode::Drive => &mut self.car.body,
            Mode::Cycle => &mut self.cyclist.body,
            Mode::Fly => &mut self.heli.body,
        }
    }

    pub fn active_body(&self) -> &Body {
        self.body(self.active)
    }

    /// `update(dt, elapsed)`: the rotors turn, the active entity moves, the idle pedestrian strolls.
    pub fn update(&mut self, world: &World, dt: f32, elapsed: f32) {
        self.heli.main_rotor += dt * MAIN_ROTOR_SPEED;
        self.heli.tail_rotor += dt * TAIL_ROTOR_SPEED;
        if self.active == Mode::Walk {
            self.update_person(world, dt);
        } else {
            // Keep the sidewalk pedestrian walking naturally when not actively controlled
            let p = &mut self.person.body;
            let walk = 1.48;
            p.speed = walk;
            p.pos.x += sinf(p.heading) * walk * dt;
            p.pos.z += cosf(p.heading) * walk * dt;
            self.person.stride += walk * dt;
            if p.pos.z < -340.0 {
                p.heading = 0.0;
            } else if p.pos.z > 340.0 {
                p.heading = core::f32::consts::PI;
            }
        }
        match self.active {
            Mode::Drive => self.update_car(world, dt),
            Mode::Cycle => self.update_cyclist(world, dt),
            Mode::Fly => {
                self.update_heli(dt);
                self.settle_heli(world, dt, elapsed);
            }
            Mode::Walk => {}
        }
    }

    fn update_person(&mut self, world: &World, dt: f32) {
        let k = self.keys;
        let max_walk = if k.boost { 8.2 } else { 4.4 };
        let turn_rate = 2.6;
        let mut target = 0.0;
        if k.forward != 0.0 {
            target = max_walk * k.forward;
        } else if k.backward != 0.0 {
            target = -max_walk * 0.65 * k.backward;
        }
        let p = &mut self.person;
        if k.left != 0.0 {
            p.body.heading += turn_rate * dt * k.left;
        }
        if k.right != 0.0 {
            p.body.heading -= turn_rate * dt * k.right;
        }
        p.body.speed += (target - p.body.speed) * (dt * 10.0).min(1.0);
        p.body.pos.x += sinf(p.body.heading) * p.body.speed * dt;
        p.body.pos.z += cosf(p.body.heading) * p.body.speed * dt;
        p.stride += fabsf(p.body.speed) * dt;
        let mut pos = p.body.pos;
        let y = pos.y + 1.0;
        resolve(world, Mode::Walk, &mut pos, 0.48, y, None, &mut self.near);
        let p = &mut self.person;
        p.body.pos = pos;
        let ground = world.ground(pos.x, pos.z, true);
        if k.up != 0.0 && p.body.pos.y <= ground + 0.02 {
            p.vertical_vel = 5.2;
        }
        p.vertical_vel -= 14.5 * dt;
        p.body.pos.y += p.vertical_vel * dt;
        if p.body.pos.y < ground {
            p.body.pos.y = ground;
            p.vertical_vel = 0.0;
        }
    }

    fn update_car(&mut self, world: &World, dt: f32) {
        let k = self.keys;
        let c = &mut self.car;
        let max_speed = if k.boost { 36.0 } else { 23.5 };
        let accel = if k.boost { 21.0 } else { 14.0 };
        let mut throttle = 0.0;
        if k.forward != 0.0 {
            throttle = k.forward;
        } else if k.backward != 0.0 {
            throttle = -k.backward;
        }
        if throttle != 0.0 {
            c.body.speed += throttle * accel * dt;
            c.body.speed = c.body.speed.min(max_speed).max(-max_speed * 0.45);
        } else {
            c.body.speed *= (1.0 - dt * 2.8).max(0.0);
            if fabsf(c.body.speed) < 0.08 {
                c.body.speed = 0.0;
            }
        }
        let mut steer_input = 0.0;
        if k.left != 0.0 {
            steer_input = k.left;
        }
        if k.right != 0.0 {
            steer_input = -k.right;
        }
        c.steer += (steer_input * 0.52 - c.steer) * (dt * 10.0).min(1.0);
        if fabsf(c.body.speed) > 0.2 {
            let dir = if c.body.speed >= 0.0 { 1.0 } else { -1.0 };
            let factor = (fabsf(c.body.speed) / 8.0).min(1.0);
            c.body.heading += c.steer * 2.4 * factor * dir * dt;
        }
        // Collision yaw spin and lateral tire slip, damped by the asphalt's grip
        let grip = expf(-dt * 6.8);
        c.body.heading += c.angular_vel * dt;
        c.angular_vel *= expf(-dt * 7.8);
        let (prev_x, prev_z) = (c.body.pos.x, c.body.pos.z);
        c.body.pos.x += (sinf(c.body.heading) * c.body.speed + c.vx) * dt;
        c.body.pos.z += (cosf(c.body.heading) * c.body.speed + c.vz) * dt;
        c.vx *= grip;
        c.vz *= grip;
        let mut pos = c.body.pos;
        let mut car = *c;
        resolve(world, Mode::Drive, &mut pos, 1.35, 1.0, Some(&mut car), &mut self.near);
        let c = &mut self.car;
        *c = car;
        c.body.pos = pos;
        c.body.pos.y = world.ground(pos.x, pos.z, false);

        // Spring-damper chassis: pitch, roll and heave
        let target_pitch = -throttle * 0.026;
        let target_roll = -c.steer * (c.body.speed / max_speed) * 0.092;
        let (spring, damp) = (68.0, 11.2);
        c.pitch_vel += ((target_pitch - c.pitch) * spring - c.pitch_vel * damp) * dt;
        c.roll_vel += ((target_roll - c.roll) * spring - c.roll_vel * damp) * dt;
        c.pitch = clamp(c.pitch + c.pitch_vel * dt, -0.22, 0.22);
        c.roll = clamp(c.roll + c.roll_vel * dt, -0.26, 0.26);
        c.heave_vel += (-c.heave * 85.0 - c.heave_vel * 12.0) * dt;
        c.heave = clamp(c.heave + c.heave_vel * dt, -0.08, 0.14);
        c.braking = throttle < 0.0 || fabsf(c.body.speed) < 0.6;

        let forward = (c.body.pos.x - prev_x) * sinf(c.body.heading) + (c.body.pos.z - prev_z) * cosf(c.body.heading);
        let raw = forward / 0.35;
        let step = if raw < 0.0 { -1.0 } else { 1.0 } * fabsf(raw).min(0.48);
        c.wheel_spin = (c.wheel_spin + step) % (core::f32::consts::PI * 2.0);
    }

    fn update_cyclist(&mut self, world: &World, dt: f32) {
        let k = self.keys;
        let b = &mut self.cyclist;
        let max_speed = if k.boost { 19.0 } else { 12.8 };
        let accel = if k.boost { 13.5 } else { 9.0 };
        let mut throttle = 0.0;
        if k.forward != 0.0 {
            throttle = k.forward;
        } else if k.backward != 0.0 {
            throttle = -0.6 * k.backward;
        }
        if throttle != 0.0 {
            b.body.speed += throttle * accel * dt;
            b.body.speed = b.body.speed.min(max_speed).max(-4.5);
        } else {
            b.body.speed *= (1.0 - dt * 2.4).max(0.0);
            if fabsf(b.body.speed) < 0.05 {
                b.body.speed = 0.0;
            }
        }
        let mut turn = 0.0;
        if k.left != 0.0 {
            turn = k.left;
        }
        if k.right != 0.0 {
            turn = -k.right;
        }
        if fabsf(b.body.speed) > 0.15 {
            let dir = if b.body.speed >= 0.0 { 1.0 } else { -1.0 };
            b.body.heading += turn * 2.15 * dir * dt;
        }
        b.body.pos.x += sinf(b.body.heading) * b.body.speed * dt;
        b.body.pos.z += cosf(b.body.heading) * b.body.speed * dt;
        let mut pos = b.body.pos;
        resolve(world, Mode::Cycle, &mut pos, 0.65, 1.0, None, &mut self.near);
        let b = &mut self.cyclist;
        b.body.pos = pos;
        b.body.pos.y = world.ground(pos.x, pos.z, false);
        let target_lean = -turn * (fabsf(b.body.speed) / 9.0).min(1.0) * 0.34;
        b.lean += (target_lean - b.lean) * (dt * 9.0).min(1.0);
        let wheel = (b.body.speed / 0.35) * dt;
        b.wheel += wheel;
        // The cranks turn with the rear wheel while pedalling and freewheel otherwise
        if throttle > 0.0 && wheel > 0.0 {
            b.pedal_phase += wheel / 3.2;
        }
    }

    fn update_heli(&mut self, dt: f32) {
        let k = self.keys;
        let h = &mut self.heli;
        let max_horiz = if k.boost { 52.0 } else { 32.0 };
        let max_vert = 16.0;
        let mut fwd = 0.0;
        if k.forward != 0.0 {
            fwd = k.forward;
        } else if k.backward != 0.0 {
            fwd = -k.backward;
        }
        let mut yaw = 0.0;
        if k.left != 0.0 {
            yaw = k.left;
        }
        if k.right != 0.0 {
            yaw = -k.right;
        }
        let mut vert = 0.0;
        if k.up != 0.0 {
            vert = k.up;
        } else if k.down != 0.0 {
            vert = -k.down;
        }
        if fwd != 0.0 {
            h.body.speed += fwd * 24.0 * dt;
            h.body.speed = h.body.speed.min(max_horiz).max(-max_horiz * 0.6);
        } else {
            h.body.speed *= (1.0 - dt * 1.8).max(0.0);
        }
        if vert != 0.0 {
            h.vertical_speed += vert * 22.0 * dt;
            h.vertical_speed = h.vertical_speed.min(max_vert).max(-max_vert);
        } else {
            h.vertical_speed *= (1.0 - dt * 3.5).max(0.0);
        }
        h.body.heading += yaw * 1.45 * dt;
        h.body.pos.x += sinf(h.body.heading) * h.body.speed * dt;
        h.body.pos.z += cosf(h.body.heading) * h.body.speed * dt;
        h.body.pos.y += h.vertical_speed * dt;
        h.body.pos.y = h.body.pos.y.min(380.0).max(0.25);
    }

    /// The rest of `updateHelicopter`: its collision, then its tilt and bob.
    fn settle_heli(&mut self, world: &World, dt: f32, elapsed: f32) {
        let k = self.keys;
        let mut pos = self.heli.body.pos;
        let y = pos.y;
        resolve(world, Mode::Fly, &mut pos, 4.5, y, None, &mut self.near);
        let h = &mut self.heli;
        h.body.pos = pos;
        let max_horiz = if k.boost { 52.0 } else { 32.0 };
        let mut yaw = 0.0;
        if k.left != 0.0 {
            yaw = k.left;
        }
        if k.right != 0.0 {
            yaw = -k.right;
        }
        let target_pitch = (h.body.speed / max_horiz) * 0.24;
        let target_roll = -yaw * 0.28;
        h.pitch += (target_pitch - h.pitch) * (dt * 5.0).min(1.0);
        h.roll += (target_roll - h.roll) * (dt * 5.0).min(1.0);
        h.bob = sinf(elapsed * 2.6) * 0.18;
    }

    /// `teleportActiveTo`: the active entity to the nearest clear spot around (x, z), facing `heading`; returns
    /// where it arrived.
    pub fn teleport(&mut self, world: &World, x: f32, z: f32, heading: f32) -> (f32, f32) {
        self.keys = Keys::default();
        let mode = self.active;
        let heli = mode == Mode::Fly;
        let altitude = if heli { self.heli.body.pos.y.min(380.0).max(92.0) } else { 0.0 };
        let heading = if heading.is_finite() { heading } else { self.active_body().heading };
        if let Some((dx, dz)) = self.find_teleport(world, x, z, heading, altitude) {
            let b = self.body_mut(mode);
            b.pos.x = dx;
            b.pos.z = dz;
            b.heading = heading;
            b.pos.y = if heli { altitude } else { world.ground(dx, dz, mode == Mode::Walk) };
        }
        self.body_mut(mode).speed = 0.0;
        match mode {
            Mode::Walk => self.person.vertical_vel = 0.0,
            Mode::Drive => {
                let c = &mut self.car;
                c.vx = 0.0;
                c.vz = 0.0;
                c.angular_vel = 0.0;
                c.steer = 0.0;
                c.pitch_vel = 0.0;
                c.roll_vel = 0.0;
                c.heave = 0.0;
                c.heave_vel = 0.0;
                c.pitch = 0.0;
                c.roll = 0.0;
                c.spark = None;
                c.braking = true;
            }
            Mode::Cycle => self.cyclist.lean = 0.0,
            Mode::Fly => {
                let h = &mut self.heli;
                h.vertical_speed = 0.0;
                h.pitch = 0.0;
                h.roll = 0.0;
                h.bob = 0.0;
            }
        }
        let b = self.active_body();
        (b.pos.x, b.pos.z)
    }

    /// `findTeleportPosition`: a fully clear arrival, testing whole footprints on rings of four metres.
    fn find_teleport(&mut self, world: &World, x: f32, z: f32, heading: f32, altitude: f32) -> Option<(f32, f32)> {
        if !x.is_finite() || !z.is_finite() {
            return None;
        }
        let mode = self.active;
        let heli = mode == Mode::Fly;
        let car = mode == Mode::Drive;
        let radius = if heli { 4.5 } else if mode == Mode::Walk { 0.48 } else { 0.65 };
        let (sin_h, cos_h) = (sinf(heading), cosf(heading));
        let ext_x = (if car { fabsf(cos_h) * 0.86 + fabsf(sin_h) * 2.18 } else { radius }) + 0.1;
        let ext_z = (if car { fabsf(sin_h) * 0.86 + fabsf(cos_h) * 2.18 } else { radius }) + 0.1;
        let reach = ext_x.max(ext_z);
        let margin = if heli { 6000.0 } else { 0.0 };
        let body_of = |m: Mode| -> [f32; 2] {
            match m {
                Mode::Walk => [0.48, 1.9],
                Mode::Drive => [2.4, 1.9],
                Mode::Cycle => [1.0, 1.9],
                Mode::Fly => [6.2, 3.2],
            }
        };
        // Other entities: x, z, radius, bottom, top
        let mut others: Vec<[f32; 5]> = Vec::new();
        for m in Mode::ALL {
            if m == mode {
                continue;
            }
            let b = self.body(m);
            others.push([b.pos.x, b.pos.z, body_of(m)[0], b.pos.y - if m == Mode::Fly { 0.7 } else { 0.0 }, b.pos.y + body_of(m)[1]]);
        }
        let me = *self.body(mode);
        let camera_back = cosf(0.09) * me.chase_distance;
        let camera_up = me.chase_height + sinf(0.09) * me.chase_distance * 0.85;
        let isl = world.info.island;
        let clamp_point = |px: f32, pz: f32| -> Option<(f32, f32)> {
            let pz = pz.min(isl[3] + margin).max(isl[2] + if heli { -6000.0 } else { 30.0 });
            let mut px = px.min(isl[1] + margin).max(isl[0] - margin);
            if !heli {
                let (w, e) = world.shore_at(pz);
                let (west, east) = (w + 8.0, e - 8.0);
                if west > east {
                    return None;
                }
                px = px.min(east).max(west);
            }
            Some((px, pz))
        };
        let mut near = core::mem::take(&mut self.near);
        let mut clear = |p: Option<(f32, f32)>| -> bool {
            let Some((px, pz)) = p else { return false };
            let base_y = if heli { altitude } else { world.ground(px, pz, mode == Mode::Walk) };
            let y = if heli { altitude } else if mode == Mode::Walk { base_y + 1.0 } else { 1.0 };
            let view_top = base_y + body_of(mode)[1].max(camera_up);
            for o in &others {
                if o[3] > view_top + 0.3 || o[4] < base_y - if heli { 0.7 } else { 0.0 } - 0.3 {
                    continue;
                }
                let (dx, dz) = (o[0] - px, o[1] - pz);
                let back = (-dx * sin_h - dz * cos_h).min(camera_back).max(0.0);
                if hypotf(dx + sin_h * back, dz + cos_h * back) < o[2] + body_of(mode)[0].max(0.5) + 0.1 {
                    return false;
                }
            }
            world.near_colliders(px, pz, reach, &mut near);
            for &ci in near.iter() {
                let c = &world.colliders[ci as usize];
                if y > c.max_y + 1.5 {
                    continue;
                }
                if px + ext_x <= c.min_x || px - ext_x >= c.max_x || pz + ext_z <= c.min_z || pz - ext_z >= c.max_z {
                    continue;
                }
                if c.obb != 0 {
                    let (dx, dz) = (px - c.cx, pz - c.cz);
                    let ls = sin_h * c.cs - cos_h * c.sn;
                    let lc = sin_h * c.sn + cos_h * c.cs;
                    let ex = (if car { fabsf(lc) * 0.86 + fabsf(ls) * 2.18 } else { radius }) + 0.1;
                    let ez = (if car { fabsf(ls) * 0.86 + fabsf(lc) * 2.18 } else { radius }) + 0.1;
                    if fabsf(dx * c.cs - dz * c.sn) >= c.hx + ex || fabsf(dx * c.sn + dz * c.cs) >= c.hz + ez {
                        continue;
                    }
                }
                return false;
            }
            true
        };
        let origin = clamp_point(x, z);
        let mut found = if clear(origin) { origin } else { None };
        if found.is_none() {
            let (cx, cz) = origin.unwrap_or((x.min(isl[1]).max(isl[0]), z.min(isl[3]).max(isl[2] + 30.0)));
            'rings: for ring in 1..=64 {
                let distance = ring as f32 * 4.0;
                for i in 0..32 {
                    let angle = i as f32 * core::f32::consts::PI / 16.0;
                    let p = clamp_point(cx + cosf(angle) * distance, cz + sinf(angle) * distance);
                    if clear(p) {
                        found = p;
                        break 'rings;
                    }
                }
            }
        }
        self.near = near;
        found
    }
}

/// `resolveHorizontalCollision`: keeps a body (a circle of `radius`, or the taxi's box) on the island and out of
/// the colliders below `y`, pushing it out along the least overlap, in the colliders' order. A taxi also takes the
/// knock: its speed, slip, spin, chassis and sparks.
pub fn resolve(world: &World, mode: Mode, pos: &mut V3, radius: f32, y: f32, mut car: Option<&mut Car>, near: &mut Vec<u32>) {
    let heli = mode == Mode::Fly;
    let isl = world.info.island;
    let m = if heli { 6000.0 } else { 0.0 };
    pos.x = pos.x.min(isl[1] + m).max(isl[0] - m);
    pos.z = pos.z.min(isl[3] + m).max(isl[2] + if heli { -6000.0 } else { 30.0 });
    if !heli && !world.shore.is_empty() {
        let (w, e) = world.shore_at(pos.z);
        let (min_x, max_x) = (w + 8.0, e - 8.0);
        if pos.x < min_x {
            pos.x = min_x;
            if let Some(c) = car.as_deref_mut() {
                if c.body.speed != 0.0 {
                    c.body.speed *= 0.4;
                }
            }
        } else if pos.x > max_x {
            pos.x = max_x;
            if let Some(c) = car.as_deref_mut() {
                if c.body.speed != 0.0 {
                    c.body.speed *= 0.4;
                }
            }
        }
    }
    let (sin_h, cos_h) = match car.as_deref() {
        Some(c) => (sinf(c.body.heading), cosf(c.body.heading)),
        None => (0.0, 1.0),
    };
    let is_car = car.is_some();
    let ext_x = if is_car { fabsf(cos_h) * 0.86 + fabsf(sin_h) * 2.18 } else { radius };
    let ext_z = if is_car { fabsf(sin_h) * 0.86 + fabsf(cos_h) * 2.18 } else { radius };
    let reach = ext_x.max(ext_z) + 6.0;
    let (mut gx, mut gz) = (pos.x, pos.z);
    world.near_colliders(gx, gz, reach, near);
    let mut i = 0usize;
    while i < near.len() {
        let ci = near[i];
        let c = &world.colliders[ci as usize];
        i += 1;
        if y > c.max_y + 1.5 {
            continue;
        }
        if !(pos.x + ext_x > c.min_x && pos.x - ext_x < c.max_x && pos.z + ext_z > c.min_z && pos.z - ext_z < c.max_z) {
            continue;
        }
        let (mut nx, mut nz) = (0.0f32, 0.0f32);
        if c.obb != 0 {
            // Oriented box: push out in its own frame
            let (dx, dz) = (pos.x - c.cx, pos.z - c.cz);
            let lx = dx * c.cs - dz * c.sn;
            let lz = dx * c.sn + dz * c.cs;
            let ls = sin_h * c.cs - cos_h * c.sn;
            let lc = sin_h * c.sn + cos_h * c.cs;
            let ex = if is_car { fabsf(lc) * 0.86 + fabsf(ls) * 2.18 } else { radius };
            let ez = if is_car { fabsf(ls) * 0.86 + fabsf(lc) * 2.18 } else { radius };
            if lx + ex <= -c.hx || lx - ex >= c.hx || lz + ez <= -c.hz || lz - ez >= c.hz {
                continue;
            }
            let o_l = lx + ex + c.hx;
            let o_r = c.hx - (lx - ex);
            let o_t = lz + ez + c.hz;
            let o_b = c.hz - (lz - ez);
            let mn = o_l.min(o_r).min(o_t).min(o_b);
            let (mut qx, mut qz, mut mx, mut mz) = (lx, lz, 0.0f32, 0.0f32);
            if mn == o_l {
                qx = -c.hx - ex;
                mx = -1.0;
            } else if mn == o_r {
                qx = c.hx + ex;
                mx = 1.0;
            } else if mn == o_t {
                qz = -c.hz - ez;
                mz = -1.0;
            } else {
                qz = c.hz + ez;
                mz = 1.0;
            }
            pos.x = c.cx + qx * c.cs + qz * c.sn;
            pos.z = c.cz - qx * c.sn + qz * c.cs;
            nx = mx * c.cs + mz * c.sn;
            nz = -mx * c.sn + mz * c.cs;
        } else {
            let o_l = (pos.x + ext_x) - c.min_x;
            let o_r = c.max_x - (pos.x - ext_x);
            let o_t = (pos.z + ext_z) - c.min_z;
            let o_b = c.max_z - (pos.z - ext_z);
            let mn = o_l.min(o_r).min(o_t).min(o_b);
            if mn == o_l {
                pos.x = c.min_x - ext_x;
                nx = -1.0;
            } else if mn == o_r {
                pos.x = c.max_x + ext_x;
                nx = 1.0;
            } else if mn == o_t {
                pos.z = c.min_z - ext_z;
                nz = -1.0;
            } else {
                pos.z = c.max_z + ext_z;
                nz = 1.0;
            }
        }
        if let Some(car) = car.as_deref_mut() {
            let total_vx = sin_h * car.body.speed + car.vx;
            let total_vz = cos_h * car.body.speed + car.vz;
            let vn = -(total_vx * nx + total_vz * nz);
            if vn > 0.5 {
                let head_on = fabsf(sin_h * nx + cos_h * nz);
                let side = cos_h * nx - sin_h * nz;
                car.vx += nx * vn * 0.42;
                car.vz += nz * vn * 0.42;
                if head_on > 0.72 {
                    car.body.speed = -car.body.speed * 0.25;
                } else {
                    car.body.speed *= 1.0 - 0.62 * head_on;
                    car.angular_vel += side * (if sin_h * nx + cos_h * nz >= 0.0 { -1.0 } else { 1.0 }) * (vn * 0.22).min(3.2);
                }
                car.pitch_vel += (sin_h * nx + cos_h * nz) * vn.min(14.0) * 0.16;
                car.roll_vel -= side * vn.min(14.0) * 0.22;
                car.heave_vel += (vn * 0.11).min(1.4);
                if vn > 2.2 {
                    car.spark = Some([pos.x - nx * ext_x * 0.92, pos.z - nz * ext_z * 0.92, nx, nz, vn]);
                }
            }
        }
        // Pushed far from where the candidates were gathered: gather them again, carrying on in the same order
        if fabsf(pos.x - gx) > 5.0 || fabsf(pos.z - gz) > 5.0 {
            gx = pos.x;
            gz = pos.z;
            world.near_colliders(gx, gz, reach, near);
            // carry on after `ci` in collider order
            i = near.partition_point(|&q| q <= ci);
        }
    }
}
