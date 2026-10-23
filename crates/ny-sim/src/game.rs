//! The city in motion: the player in one of four modes, the camera on it, the hour, the traffic.
//!
//! This is the contract the device runtimes and the interface's flow (`ny-interface`) build against. The bodies
//! here are a stand-in (a free camera that moves by the pad) until the reference's controllers, cameras and
//! traffic are ported behind the same functions.

use crate::math::*;
use crate::{Hour, Mode};

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

/// Where the player is, as the reference names it (`getLocationLabel`): a landmark or district (index into
/// the world's places), and the avenue and street nearest (indices into its roads).
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
    /// Miles an hour, as the reference shows them.
    pub mph: f32,
    /// Feet above the street.
    pub altitude_ft: f32,
    /// Radians from north, clockwise.
    pub heading: f32,
    /// The player on the map: x and z, metres.
    pub at: [f32; 2],
    pub location: Location,
    /// Index of the camera preset (the reference's `CAMERA_PRESETS`).
    pub camera: u8,
}

pub struct Game {
    pub mode: Mode,
    pub hour: Hour,
    pub camera: u8,
    /// Seconds since the start.
    pub clock: f32,
    eye: V3,
    yaw: f32,
    pitch: f32,
    speed: f32,
}

impl Game {
    /// A game at the reference's start: in the taxi on 5th Avenue by 37th Street, heading north, at golden hour.
    pub fn new() -> Game {
        Game { mode: Mode::Drive, hour: Hour::Golden, camera: 0, clock: 0.0, eye: v3(3.6, 8.0, 380.0), yaw: 0.0, pitch: 0.0, speed: 0.0 }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    pub fn set_hour(&mut self, hour: Hour) {
        self.hour = hour;
    }

    /// Moves the player to a place of the world (`world.json` `places`): x, z and the heading to face.
    pub fn teleport(&mut self, x: f32, z: f32, heading: f32) {
        self.eye = v3(x, self.eye.y, z);
        self.yaw = heading;
    }

    pub fn step(&mut self, input: &Input, dt: f32) {
        self.clock += dt;
        self.yaw += input.turn * 1.6 * dt;
        self.pitch = clamp(self.pitch + input.orbit[1] * dt, -1.2, 1.2);
        let top = if input.boost { 40.0 } else { 14.0 };
        self.speed = input.ahead * top;
        let fwd = v3(libm::sinf(self.yaw), 0.0, -libm::cosf(self.yaw));
        self.eye = self.eye + fwd * (self.speed * dt) + v3(0.0, input.rise * 10.0 * dt, 0.0);
        self.eye.y = self.eye.y.max(1.6);
    }

    pub fn view(&self) -> View {
        let dir = v3(libm::sinf(self.yaw) * libm::cosf(self.pitch), libm::sinf(self.pitch), -libm::cosf(self.yaw) * libm::cosf(self.pitch));
        View { eye: self.eye, look: self.eye + dir, fov: 56.0 }
    }

    pub fn readout(&self) -> Readout {
        Readout { mode: self.mode, hour: self.hour, mph: self.speed.abs() * 2.237, altitude_ft: self.eye.y * 3.281, heading: self.yaw, at: [self.eye.x, self.eye.z], location: Location::default(), camera: self.camera }
    }
}

impl Default for Game {
    fn default() -> Game {
        Game::new()
    }
}
