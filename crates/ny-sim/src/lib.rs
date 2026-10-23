//! What moves in Pocket New York, the same on every device.
//!
//! The reference is a city to go through four ways (`Mode`): on foot, in a taxi, on a bicycle and in a helicopter
//! (its controllers.js), seen through its cameras (cameras.js), under one of its four hours (lighting.js), with
//! traffic on its road network (traffic.js). A device steps a `Game` once a frame with the pad as an `Input`, and
//! draws the city from `Game::view`. What the interface shows of it is `Game::readout`.
//!
//! The crate builds without `std` (`default-features = false`): `core`, `alloc` and `libm`.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod game;
pub mod math;

pub use game::{Game, Input, Location, Readout, View};

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
