//! What the street hides: a horizon around the eye, by direction, of how high nearby buildings surely stand, so that
//! the mid and far levels behind them (and near cells across a block) are not drawn.
//!
//! The occluders are the height grid's cells near the eye at 8 m, each at the lowest of what stands on it, so that a
//! cell never claims more than it covers. A cell raises the horizon over the directions it covers whole to the slope
//! (rise over run) of its top at its far side. A box wholly past the occluders' reach is seen when, in some direction
//! it spans, its top at its near side rises over the horizon there; a box nearer is seen (an occluder behind it would
//! hide it otherwise). Directions are diamond angles (monotonic in the true angle, 0..4, no
//! trigonometry), in `BINS` steps. From above the city every slope is negative and nothing is hidden.

use ny_sim::math::V3;

pub const BINS: usize = 720;
/// Metres around the eye whose cells stand in the horizon.
const REACH: f32 = 256.0;

/// The lowest height of each cell of a square of the city, row by row from its north-west corner.
pub struct Coarse {
    pub x0: f32,
    pub z0: f32,
    pub step: f32,
    pub side: usize,
    pub low: Vec<f32>,
}

pub struct Horizon {
    slope: [f32; BINS],
    eye: V3,
    /// Whether this frame's horizon hides anything (off: everything is seen).
    pub on: bool,
}

/// The diamond angle of `(x, z)`: 0..4 round the circle, monotonic in the true angle.
fn diamond(x: f32, z: f32) -> f32 {
    if z >= 0.0 {
        if x >= 0.0 {
            z / (x + z).max(1e-6)
        } else {
            1.0 - x / (z - x).max(1e-6)
        }
    } else if x < 0.0 {
        2.0 - z / (-x - z).max(1e-6)
    } else {
        3.0 + x / (x - z).max(1e-6)
    }
}

/// The directions a box spans seen from `eye`, as bins: its first and one past its last (the last may pass `BINS`,
/// round the circle), with the nearest and the farthest of its corners.
fn span(eye: V3, min: [f32; 2], max: [f32; 2]) -> (f32, f32, f32, f32) {
    let corners = [[min[0], min[1]], [max[0], min[1]], [max[0], max[1]], [min[0], max[1]]];
    let (cx, cz) = ((min[0] + max[0]) * 0.5 - eye.x, (min[1] + max[1]) * 0.5 - eye.z);
    let mid = diamond(cx, cz);
    let (mut lo, mut hi, mut far) = (0.0f32, 0.0f32, 0.0f32);
    for c in corners {
        let (dx, dz) = (c[0] - eye.x, c[1] - eye.z);
        let mut d = diamond(dx, dz) - mid;
        if d > 2.0 {
            d -= 4.0;
        } else if d < -2.0 {
            d += 4.0;
        }
        lo = lo.min(d);
        hi = hi.max(d);
        far = far.max(dx * dx + dz * dz);
    }
    let k = BINS as f32 / 4.0;
    let start = (mid + lo).rem_euclid(4.0) * k;
    // (nearest: from the eye to the box along the ground)
    let nx = (min[0] - eye.x).max(0.0).max(eye.x - max[0]);
    let nz = (min[1] - eye.z).max(0.0).max(eye.z - max[1]);
    (start, start + (hi - lo) * k, libm::sqrtf(nx * nx + nz * nz), libm::sqrtf(far))
}

impl Horizon {
    pub fn new() -> Horizon {
        Horizon { slope: [f32::NEG_INFINITY; BINS], eye: V3::ZERO, on: false }
    }

    /// The horizon around `eye` from the cells of `coarse` within `REACH`.
    pub fn build(&mut self, eye: V3, coarse: Option<&Coarse>) {
        self.slope = [f32::NEG_INFINITY; BINS];
        self.eye = eye;
        self.on = false;
        let Some(g) = coarse else { return };
        let r = (REACH / g.step).ceil() as i32;
        let (ci, cj) = (((eye.x - g.x0) / g.step).floor() as i32, ((eye.z - g.z0) / g.step).floor() as i32);
        for j in (cj - r).max(0)..=(cj + r).min(g.side as i32 - 1) {
            for i in (ci - r).max(0)..=(ci + r).min(g.side as i32 - 1) {
                let rise = g.low[j as usize * g.side + i as usize] - eye.y;
                // (a cell not a metre over the eye hides little; one under the eye, nothing above it)
                if rise < 1.0 {
                    continue;
                }
                let min = [g.x0 + i as f32 * g.step, g.z0 + j as f32 * g.step];
                let max = [min[0] + g.step, min[1] + g.step];
                let (start, end, near, far) = span(eye, min, max);
                if near < 1.0 || near > REACH {
                    continue;
                }
                let s = rise / far;
                // The bins it covers whole.
                let (first, last) = (libm::ceilf(start) as i32, libm::floorf(end) as i32);
                for b in first..last {
                    let at = b.rem_euclid(BINS as i32) as usize;
                    if self.slope[at] < s {
                        self.slope[at] = s;
                    }
                }
                self.on = true;
            }
        }
    }

    /// Whether anything of the box `min .. max` rises over the horizon.
    pub fn sees(&self, min: [f32; 3], max: [f32; 3]) -> bool {
        if !self.on {
            return true;
        }
        let e = self.eye;
        if e.x >= min[0] && e.x <= max[0] && e.z >= min[2] && e.z <= max[2] {
            return true;
        }
        let (start, end, near, _) = span(e, [min[0], min[2]], [max[0], max[2]]);
        if near < REACH {
            return true;
        }
        let top = (max[1] - e.y) / near;
        let (first, last) = (libm::floorf(start) as i32, libm::floorf(end) as i32);
        (first..=last).any(|b| self.slope[b.rem_euclid(BINS as i32) as usize] < top)
    }
}
