//! The signals (`signalPhase` and `walkSignal` in city-config.js, timed by `SIGNAL_TIMING`): one cycle for the
//! whole city, the avenues' green first.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Green,
    Amber,
    Red,
}

/// `SIGNAL_TIMING`: cycle, avenue amber, avenue red, street amber, walk (seconds).
#[derive(Clone, Copy, Debug)]
pub struct Timing(pub [f32; 5]);

impl Default for Timing {
    fn default() -> Timing {
        Timing([60.0, 26.0, 30.0, 56.0, 7.0])
    }
}

fn wrap(t: f32, cycle: f32) -> f32 {
    ((t % cycle) + cycle) % cycle
}

impl Timing {
    /// The phase for avenue (or cross street) traffic at `time`.
    pub fn phase(&self, time: f32, avenue: bool) -> Phase {
        let [cycle, avenue_amber, avenue_red, street_amber, _] = self.0;
        let t = wrap(time, cycle);
        if avenue {
            if t < avenue_amber {
                Phase::Green
            } else if t < avenue_red {
                Phase::Amber
            } else {
                Phase::Red
            }
        } else if t < avenue_red {
            Phase::Red
        } else if t < street_amber {
            Phase::Green
        } else {
            Phase::Amber
        }
    }

    /// Do pedestrians alongside avenue (or cross street) traffic have the WALK at `time`?
    pub fn walk(&self, time: f32, avenue: bool) -> bool {
        let [cycle, _, avenue_red, _, walk] = self.0;
        wrap(time - if avenue { 0.0 } else { avenue_red }, cycle) < walk
    }
}
