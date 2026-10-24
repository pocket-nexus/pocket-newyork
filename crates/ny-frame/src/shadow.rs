//! The window of the city's height grid (`HMAP`) the shadows are swept over: `SIDE` cells along a side, moved to
//! keep the eye near its middle, swept for the hour's sun (`ny_sim::shadow::sweep`), and the street's occluders read
//! from it (`horizon::Coarse`). A device reads the window's heights and shows the sweep as a texture.

use alloc::vec;

use ny_pack::Heights;
use ny_sim::math::V3;

use crate::horizon::Coarse;

/// Cells along a side of the window: 2 048 m of 2 m cells.
pub const SIDE: usize = 1024;
/// The window moves when the eye is this far from its middle.
pub const DRIFT: f32 = 320.0;
/// Cells of the window to a cell of the street's occluders (8 m).
pub const COARSE: usize = 4;

/// The window's north-west cell that puts `eye` in its middle.
pub fn window_at(head: &Heights, eye: V3) -> [i32; 2] {
    let half = (SIDE / 2) as f32 * head.step;
    [libm::floorf((eye.x - half - head.x0) / head.step) as i32, libm::floorf((eye.z - half - head.z0) / head.step) as i32]
}

/// The world x and z of a window's north-west corner.
pub fn origin(head: &Heights, at: [i32; 2]) -> (f32, f32) {
    (head.x0 + at[0] as f32 * head.step, head.z0 + at[1] as f32 * head.step)
}

/// Whether `eye` has left the middle of the window at `at`.
pub fn left(head: &Heights, at: [i32; 2], eye: V3) -> bool {
    let mid = [head.x0 + (at[0] as f32 + (SIDE / 2) as f32) * head.step, head.z0 + (at[1] as f32 + (SIDE / 2) as f32) * head.step];
    libm::fabsf(eye.x - mid[0]).max(libm::fabsf(eye.z - mid[1])) > DRIFT
}

/// Whether the sun towards `now` has moved from the one a sweep was for.
pub fn sun_moved(was: V3, now: V3) -> bool {
    was.dot(now) < 0.999_998_5
}

/// From world x and z to the window's texture coordinates: scale, then x and z offsets.
pub fn mapping(head: &Heights, at: [i32; 2]) -> [f32; 3] {
    let size = SIDE as f32 * head.step;
    let (x0, z0) = origin(head, at);
    [1.0 / size, -x0 / size, -z0 / size]
}

/// Metres of height in one unit of the swept texture as a program reads it (0..1).
pub fn unit(head: &Heights) -> f32 {
    65535.0 * head.unit
}

/// The street's occluders from a window's heights (`SIDE` × `SIDE`): each 8 m cell at the lowest of what stands on
/// it.
pub fn coarse(heights: &[u16], head: &Heights, at: [i32; 2]) -> Coarse {
    let side = SIDE / COARSE;
    let mut low = vec![0.0f32; side * side];
    for j in 0..side {
        for i in 0..side {
            let mut m = u16::MAX;
            for jj in 0..COARSE {
                for ii in 0..COARSE {
                    m = m.min(heights[(j * COARSE + jj) * SIDE + i * COARSE + ii]);
                }
            }
            low[j * side + i] = m as f32 * head.unit;
        }
    }
    let (x0, z0) = origin(head, at);
    Coarse { x0, z0, step: head.step * COARSE as f32, side, low }
}

/// Sweeps a window's heights (`SIDE` × `SIDE`) for the sun towards `light` into `out`.
pub fn sweep(heights: &[u16], out: &mut [u16], head: &Heights, light: V3) {
    ny_sim::shadow::sweep(heights, SIDE, SIDE, out, SIDE, light.x, light.y, light.z, head.step, head.unit);
}
