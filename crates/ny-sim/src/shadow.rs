//! Shadows of a city that is a field of heights.
//!
//! `heights` holds, for each cell of a grid, the top of whatever stands
//! there. For a direction of the light, `sweep` writes into each cell the
//! height below which a point over that cell is in shadow: the cell's own
//! top, or what the cell one step towards the light passes on, lowered by
//! the light's slope over that step. A renderer lights a point when it is
//! at or above that height. The step towards the light lands between two
//! cells, and the mix of the two softens a shadow's edge the farther it
//! lies from what casts it.
//!
//! Heights are `u16` in a unit the caller chooses (`unit` metres).

fn abs(x: f32) -> f32 {
    libm::fabsf(x)
}

fn sqrt(x: f32) -> f32 {
    libm::sqrtf(x)
}

/// Fills `out` (rows of `stride` cells, the first `w` used) from `heights` (`w` × `h`, rows along +z).
/// `(lx, ly, lz)`: unit vector towards the light, `ly` up. `step`: metres per cell.
#[allow(clippy::too_many_arguments)]
pub fn sweep(heights: &[u16], w: usize, h: usize, out: &mut [u16], stride: usize, lx: f32, ly: f32, lz: f32, step: f32, unit: f32) {
    let flat = sqrt(lx * lx + lz * lz);
    if flat < 1e-4 || w < 2 || h < 2 {
        // Light from straight above: nothing shades anything else.
        for z in 0..h {
            out[z * stride..z * stride + w].copy_from_slice(&heights[z * w..z * w + w]);
        }
        return;
    }
    let slope = ly.max(0.01) / flat;
    if abs(lx) >= abs(lz) {
        // One column towards the light per step; the row shifts by t.
        let t = lz / abs(lx);
        let drop = ((step * sqrt(1.0 + t * t) * slope / unit) as u32).min(65535);
        let f = (abs(t) * 256.0) as u32;
        let from_east = lx > 0.0;
        // A cell reads the column next to it in its own row and in the row t points to: that row goes first.
        for zi in 0..h {
            let z = if t >= 0.0 { h - 1 - zi } else { zi };
            let other = if t >= 0.0 { z + 1 } else { z.wrapping_sub(1) };
            let (row, other_row) = (z * stride, other.wrapping_mul(stride));
            for xi in 0..w {
                let x = if from_east { w - 1 - xi } else { xi };
                let own = heights[z * w + x] as u32;
                let up = if from_east { x + 1 } else { x.wrapping_sub(1) };
                let value = if up < w {
                    let a = out[row + up] as u32;
                    let b = if other < h { out[other_row + up] as u32 } else { 0 };
                    let passed = (a * (256 - f) + b * f) >> 8;
                    own.max(passed.saturating_sub(drop))
                } else {
                    own
                };
                out[row + x] = value as u16;
            }
        }
    } else {
        // One row towards the light per step; the column shifts by t.
        let t = lx / abs(lz);
        let drop = ((step * sqrt(1.0 + t * t) * slope / unit) as u32).min(65535);
        let f = (abs(t) * 256.0) as u32;
        let from_south = lz > 0.0;
        for zi in 0..h {
            let z = if from_south { h - 1 - zi } else { zi };
            let up = if from_south { z + 1 } else { z.wrapping_sub(1) };
            let row = z * stride;
            if up >= h {
                out[row..row + w].copy_from_slice(&heights[z * w..z * w + w]);
                continue;
            }
            let (before, after) = out.split_at_mut(if from_south { up * stride } else { row });
            let (this, above): (&mut [u16], &[u16]) = if from_south { (&mut before[row..row + w], &after[..w]) } else { (&mut after[..w], &before[up * stride..up * stride + w]) };
            let own = &heights[z * w..z * w + w];
            for x in 0..w {
                let other = if t >= 0.0 { x + 1 } else { x.wrapping_sub(1) };
                let a = above[x] as u32;
                let b = if other < w { above[other] as u32 } else { 0 };
                let passed = (a * (256 - f) + b * f) >> 8;
                this[x] = (own[x] as u32).max(passed.saturating_sub(drop)) as u16;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A tower in an empty field: its shadow reaches as far as its height over the light's slope, away from the light.
    #[test]
    fn a_tower_shades_the_ground_behind_it() {
        let (w, h) = (64usize, 48usize);
        let mut heights = vec![0u16; w * h];
        // 40 m tall at (32, 24); unit 0.5 m, cells of 2 m.
        heights[24 * w + 32] = 80;
        let mut out = vec![0u16; w * h];
        // Light from the east, 45 degrees up: the shadow runs 40 m west, twenty cells.
        let s = core::f32::consts::FRAC_1_SQRT_2;
        sweep(&heights, w, h, &mut out, w, s, s, 0.0, 2.0, 0.5);
        assert_eq!(out[24 * w + 32], 80);
        assert_eq!(out[24 * w + 31], 76);
        assert_eq!(out[24 * w + 22], 40);
        assert_eq!(out[24 * w + 12], 0);
        assert_eq!(out[24 * w + 33], 0, "nothing towards the light");
        assert_eq!(out[23 * w + 31], 0, "nothing beside the shadow");
        // Light from the north (-z): the shadow runs south.
        sweep(&heights, w, h, &mut out, w, 0.0, s, -s, 2.0, 0.5);
        assert_eq!(out[25 * w + 32], 76);
        assert_eq!(out[23 * w + 32], 0);
        // From the south-east, between the axes: the shadow lies north-west, and softens with distance.
        let d = 1.0 / sqrt(3.0);
        sweep(&heights, w, h, &mut out, w, d, d, d, 2.0, 0.5);
        assert!(out[23 * w + 31] > 60);
        assert!(out[19 * w + 27] > 20 && out[19 * w + 27] < 80);
        assert_eq!(out[25 * w + 33], 0);
    }
}
