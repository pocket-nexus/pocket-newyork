//! The city in motion: the player in one of four modes, the camera on it, the hour, the traffic. A frame is the
//! reference's (js/main.js): the controllers, then the camera, then the traffic around the camera.
//!
//! This is the contract the device runtimes and the interface's flow (`ny-interface`) build against.

use crate::camera::{Cameras, CHASE};
use crate::math::*;
use crate::player::{Keys, Players};
use crate::crowd::{Crowd, Pedestrian};
use crate::highway::{Highway, HighwayPose, PlayerOnRoad};
use crate::traffic::{PlayerTraffic, Sight, Traffic, Vehicle};
use crate::world::World;
use crate::{Hour, Mode};
use alloc::string::String;
use libm::{cosf, sinf};

/// The pad, as the four modes read it (the reference's keys, as axes).
#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    /// Ahead (+1) or back (−1): walk, drive or reverse, pedal or brake, pitch the helicopter.
    pub ahead: f32,
    /// Right (+1) or left (−1): turn, steer, lean, yaw.
    pub turn: f32,
    /// Up (+1) or down (−1): the helicopter's climb; a jump on foot when above 0.5.
    pub rise: f32,
    /// Sprint, boost.
    pub boost: bool,
    /// The camera turned about the player (a drag in the reference), radians a second.
    pub orbit: [f32; 2],
}

/// What a device draws from.
#[derive(Clone, Copy, Debug, Default)]
pub struct View {
    pub eye: V3,
    pub look: V3,
    /// Vertical field of view, degrees.
    pub fov: f32,
}

/// Where the player is, as the reference names it (`getLocationLabel`): a landmark, Central Park, a waterfront
/// or the harbour, and the avenue and street nearest. Each is a line of the pack's `NAME` (`World::name`);
/// `World::label` writes the reference's text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Location {
    pub place: Option<u16>,
    pub avenue: Option<u16>,
    pub street: Option<u16>,
    /// The streets are "near" the place rather than its corner.
    pub near: bool,
}

/// The numbers the interface shows.
#[derive(Clone, Copy, Debug, Default)]
pub struct Readout {
    pub mode: Mode,
    pub hour: Hour,
    /// Miles an hour, as the reference shows them (it rounds them for the screen).
    pub mph: f32,
    /// Feet above the street.
    pub altitude_ft: f32,
    /// Radians from north (the grid's north, −z), clockwise.
    pub heading: f32,
    /// The player on the map: x and z, metres.
    pub at: [f32; 2],
    pub location: Location,
    /// Index of the camera preset (the reference's `CAMERA_PRESETS`).
    pub camera: u8,
}

/// The rooms' glow of each hour (`TIME_PRESETS[...].interiorGlow` in lighting.js), which sets the headlights.
pub const INTERIOR_GLOW: [f32; 4] = [0.48, 0.24, 1.25, 1.55];

pub struct Game {
    pub mode: Mode,
    pub hour: Hour,
    pub camera: u8,
    /// Seconds since the start.
    pub clock: f32,
    pub world: World,
    /// The four entities: a device draws each where it is, the parked ones too.
    pub players: Players,
    pub cameras: Cameras,
    pub traffic: Traffic,
    /// The sidewalks' pedestrians, and the highway's cars.
    pub crowd: Crowd,
    pub highway: Highway,
    /// Cars, people and the highway's cars (a device may turn them off).
    pub traffic_on: bool,
}

impl Game {
    /// A game over a stand-in world with no buildings and no streets (`World::stand_in`), for a tool that has no
    /// pack: the taxi at the reference's start on 5th Avenue by 37th Street, at golden hour.
    pub fn new() -> Game {
        Game::with_world(World::stand_in())
    }

    /// The game of a pack's world: in the taxi on 5th Avenue by 37th Street, the chase camera, golden hour.
    pub fn with_world(world: World) -> Game {
        let players = Players::new(&world);
        let traffic = Traffic::new(&world);
        let crowd = Crowd::new(&world);
        let highway = Highway::new(&world);
        let mut cameras = Cameras::default();
        cameras.set_preset(&world, CHASE, true);
        Game { mode: Mode::Drive, hour: Hour::Golden, camera: CHASE, clock: 0.0, world, players, cameras, traffic, crowd, highway, traffic_on: true }
    }

    /// `selectMode`: the player moves the other entity, and a scenic camera hands over to the chase camera.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.players.active = mode;
        self.players.keys = Keys::default();
        if !self.cameras.is_follow() {
            self.set_camera_eased(CHASE, false);
        }
    }

    pub fn set_hour(&mut self, hour: Hour) {
        self.hour = hour;
    }

    /// `selectCamera(id, true)`: one of the reference's cameras, starting where it should be.
    pub fn set_camera(&mut self, preset: u8) {
        self.set_camera_eased(preset, true);
    }

    fn set_camera_eased(&mut self, preset: u8, snap: bool) {
        self.cameras.set_preset(&self.world, preset, snap);
        self.camera = self.cameras.preset;
    }

    /// Moves the player to the nearest clear spot around (x, z), facing `heading` (the reference's heading: 0
    /// faces south), with a fresh chase camera (`teleportTo`).
    pub fn teleport(&mut self, x: f32, z: f32, heading: f32) {
        self.players.teleport(&self.world, x, z, heading);
        self.cameras.reset(&self.world);
        self.camera = self.cameras.preset;
    }

    /// To one of the reference's teleports (`TELEPORT_LOCATIONS`, the pack's `TELE`, in its order).
    pub fn teleport_to(&mut self, place: usize) -> bool {
        let Some(t) = self.world.teleports.get(place).copied() else { return false };
        self.teleport(t.x, t.z, t.heading);
        true
    }

    pub fn step(&mut self, input: &Input, dt: f32) {
        let dt = dt.clamp(0.001, 0.05);
        self.clock += dt;
        self.players.active = self.mode;
        let walk = self.mode == Mode::Walk;
        self.players.keys = Keys {
            forward: input.ahead.max(0.0),
            backward: (-input.ahead).max(0.0),
            left: (-input.turn).max(0.0),
            right: input.turn.max(0.0),
            up: if walk { if input.rise > 0.5 { 1.0 } else { 0.0 } } else { input.rise.max(0.0) },
            down: (-input.rise).max(0.0),
            boost: input.boost,
        };
        self.players.update(&self.world, dt, self.clock);
        self.cameras.orbit(input.orbit, dt);
        self.cameras.update(&self.world, &self.players, dt, self.clock);
        self.camera = self.cameras.preset;
        if self.traffic_on {
            let body = *self.players.active_body();
            let (half_w, half_l) = match self.mode {
                Mode::Cycle => (0.35, 0.85),
                Mode::Walk => (0.32, 0.32),
                _ => (0.86, 2.18),
            };
            let mut vx = sinf(body.heading) * body.speed;
            let mut vz = cosf(body.heading) * body.speed;
            if self.mode == Mode::Drive {
                vx += self.players.car.vx;
                vz += self.players.car.vz;
            }
            let player = PlayerTraffic { active: self.mode != Mode::Fly && body.pos.y < 2.6, x: body.pos.x, z: body.pos.z, vx, vz, half_w, half_l };
            let v = self.view();
            let half_v = v.fov.to_radians() * 0.5;
            let half_fov = libm::atanf(libm::tanf(half_v) * (960.0 / 544.0));
            let sight = Sight { eye: v.eye, dir: (v.look - v.eye).normalize(), half_fov };
            let focus = [self.cameras.pos.x, self.cameras.pos.z];
            self.traffic.step(&self.world, dt, self.clock, focus, &sight, &player);
            self.crowd.step(&self.world, &mut self.traffic, dt, self.clock, focus, &sight);
            let on_road = PlayerOnRoad { x: body.pos.x, y: body.pos.y, z: body.pos.z, vx, vz };
            self.highway.step(&self.world, dt, if self.mode == Mode::Fly { None } else { Some(&on_road) });
            // The player is pushed off a car it ran into, and loses most of its speed
            if player.active {
                let b = self.players.body_mut(self.mode);
                let (mut x, mut z) = (b.pos.x, b.pos.z);
                if self.traffic.push_player(&mut x, &mut z, b.heading, half_w, half_l).is_some() {
                    b.pos.x = x;
                    b.pos.z = z;
                    b.speed *= 0.5;
                }
            }
        }
    }

    pub fn view(&self) -> View {
        View { eye: self.cameras.pos, look: self.cameras.look, fov: self.cameras.fov }
    }

    /// Where the label is read: the player on a follow camera, the camera itself on a scenic one.
    pub fn location(&self) -> Location {
        let at = if self.cameras.is_follow() { self.players.active_body().pos } else { self.cameras.pos };
        self.world.location(at.x, at.z)
    }

    /// The reference's text for `location()`.
    pub fn label(&self) -> String {
        self.world.label(&self.location())
    }

    pub fn readout(&self) -> Readout {
        let b = self.players.active_body();
        let tau = core::f32::consts::PI * 2.0;
        let mut heading = (core::f32::consts::PI - b.heading) % tau;
        if heading < 0.0 {
            heading += tau;
        }
        Readout {
            mode: self.mode,
            hour: self.hour,
            mph: libm::fabsf(b.speed) * 2.237,
            altitude_ft: (b.pos.y * 3.281).max(0.0),
            heading,
            at: [b.pos.x, b.pos.z],
            location: self.location(),
            camera: self.camera,
        }
    }

    /// The cars and buses, placed or not (`Vehicle::placed`).
    pub fn vehicles(&self) -> &[Vehicle] {
        &self.traffic.vehicles
    }

    /// The pedestrians, placed or not (`Pedestrian::placed`).
    pub fn pedestrians(&self) -> &[Pedestrian] {
        &self.crowd.peds
    }

    /// The highway's cars the camera draws this frame, nearest level of detail first in the reference's order
    /// (`Highway::drawn`).
    pub fn highway_cars(&self, out: &mut alloc::vec::Vec<HighwayPose>) {
        let v = self.view();
        self.highway.drawn(&self.world, v.eye, (v.look - v.eye).normalize(), out);
    }

    /// The taxi's last knock against a building hard enough to throw sparks, once: x, z, the push's normal (x, z)
    /// and its speed. A device that draws sparks takes it every frame (the reference's `lastCollisionSpark`).
    pub fn take_spark(&mut self) -> Option<[f32; 5]> {
        self.players.car.spark.take()
    }

    /// How bright the vehicles' headlights are at this hour (0 by day).
    pub fn headlights(&self) -> f32 {
        Traffic::beams(INTERIOR_GLOW[self.hour.index()])
    }
}

impl Default for Game {
    fn default() -> Game {
        Game::new()
    }
}

impl World {
    /// A world with no colliders, ground, names or roads: the four entities where controllers.js builds them
    /// (5th Avenue at 37th Street), the reference's cameras and signal timing.
    pub fn stand_in() -> World {
        use ny_pack::sim::{CameraPreset, EntityStart, SimWorld};
        let e = |x: f32, y: f32, z: f32, h: f32, d: f32, ht: f32, t: f32| EntityStart { pos: [x, y, z], heading: h, chase_distance: d, chase_height: ht, target_y: t, pad: 0.0 };
        let mut w = World {
            info: SimWorld { island: [-1.0e5, 1.0e5, -1.0e5, 1.0e5], sidewalk_height: 0.22, lawn_top: f32::NAN, cut_z: -1.0e5, signal: [60.0, 26.0, 30.0, 56.0, 7.0], ..Default::default() },
            entities: [e(-11.6, 0.22, 410.6, 0.0, 4.3, 1.95, 1.55), e(3.6, 0.0, 410.6, 0.0, 6.9, 2.12, 0.96), e(5.2, 0.0, 390.6, 0.0, 4.8, 2.05, 1.35), e(-110.0, 96.0, 338.6, 0.62, 11.4, 3.3, 1.55)],
            ..Default::default()
        };
        w.info.central_park = [0.0; 4];
        w.cameras = [56.0, 60.0, 68.0, 52.0, 48.0, 58.0, 50.0, 52.0, 52.0, 46.0, 50.0].iter().enumerate().map(|(i, &fov)| CameraPreset { fov, scenic: (i >= 3) as u32, id: 0, pad: 0 }).collect();
        w.index();
        w
    }
}
