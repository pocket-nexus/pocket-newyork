//! What moves in Pocket New York, the same on every device.
//!
//! The reference is a city to go through four ways (`Mode`): on foot, in a taxi, on a bicycle and in a helicopter
//! (its controllers.js), seen through its cameras (cameras.js), under one of its four hours (lighting.js), with
//! traffic on its road network (traffic.js). A device builds a `World` from the pack's sections (`ny_pack::sim`),
//! steps a `Game` once a frame with the pad as an `Input`, and draws the city from `Game::view`, the four entities
//! from `Game::players` and the traffic from `Game::vehicles`. What the interface shows of it is `Game::readout`
//! and `Game::label`.
//!
//! | Module | The reference's |
//! | --- | --- |
//! | `player` | controllers.js: the four entities, their collision against the colliders, teleports |
//! | `camera` | cameras.js: chase, action, pov and the scenic views |
//! | `location` | city-config.js `getLocationLabel`, `nearestRoads` |
//! | `signal` | city-config.js `signalPhase`, `walkSignal` |
//! | `traffic` | traffic.js and road-network.js, without lane changes; the crosswalks held both ways |
//! | `crowd` | traffic.js: the sidewalks' pedestrians and the crosswalks they cross |
//! | `highway` | traffic.js `createHighwayTraffic`: the FDR Drive's and the West Side Highway's cars |
//! | `world` | what they read: colliders, ground, shore, names, the road network |
//!
//! The crate builds without `std` (`default-features = false`): `core`, `alloc` and `libm`.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod camera;
pub mod crowd;
pub mod game;
pub mod highway;
pub mod location;
pub mod math;
pub mod player;
pub mod shadow;
pub mod signal;
pub mod traffic;
pub mod world;

pub use game::{Game, Input, Location, Readout, View};
pub use world::World;

/// How the player goes through the city: the reference's four entities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    /// On foot (`person`).
    Walk,
    /// The yellow taxi (`car`), where the reference starts.
    #[default]
    Drive,
    /// A bicycle (`cyclist`).
    Cycle,
    /// The sightseeing helicopter (`helicopter`).
    Fly,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Walk, Mode::Drive, Mode::Cycle, Mode::Fly];
    /// The reference's name for it.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Walk => "person",
            Mode::Drive => "car",
            Mode::Cycle => "cyclist",
            Mode::Fly => "helicopter",
        }
    }
}

/// The reference's hours (`TIME_PRESETS`), in the order of the pack's `PRST` table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Hour {
    /// Golden Hour, 17:45: where the reference starts.
    #[default]
    Golden,
    /// Sunny Midday, 13:30.
    Midday,
    /// Blue Hour, 19:15.
    Twilight,
    /// Manhattan Night, 23:00.
    Midnight,
}

impl Hour {
    pub const ALL: [Hour; 4] = [Hour::Golden, Hour::Midday, Hour::Twilight, Hour::Midnight];
    pub fn index(self) -> usize {
        self as usize
    }
}
