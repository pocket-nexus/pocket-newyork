//! What a frame draws: every place of the pack at the level of detail its distance asks for, culled to the view.
//!
//! A region beyond the mid reach draws its far batches whole. Closer, each of its blocks is drawn by itself: a
//! block beyond the mid reach as its runs of the region's far batches, a block beyond the near reach by its mid
//! batches whole, and a block the eye is near by its cells: a cell within the near reach at the near level (and its
//! small things within the detail reach), the others as runs of the block's mid batches.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use ny_pack::{kind, Batch, Block, Cell, City, Region, NO_SPANS};

pub struct Tables<'a> {
    pub city: &'a City,
    pub cells: &'a [Cell],
    pub blocks: &'a [Block],
    pub regions: &'a [Region],
    pub batches: &'a [Batch],
    pub spans: &'a [u32],
}

/// Distances, metres, at which each level gives way to the next.
#[derive(Clone, Copy, Debug)]
pub struct Reach {
    pub detail: f32,
    pub near: f32,
    pub mid: f32,
}

/// A draw: a batch, and the range of its indices (or, for an instanced batch, of its instances) to draw. `cell`:
/// the cell whose record of `NEAR` holds its data, `RESIDENT` for the pack's own streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item {
    pub batch: u32,
    pub from: u32,
    pub to: u32,
    pub cell: u32,
}

pub const RESIDENT: u32 = u32::MAX;

/// Places drawn at each level: near cells, mid blocks, far regions, detail cells.
#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    pub places: [u32; 4],
}

/// The six planes of a view, `a x + b y + c z + d >= 0` inside.
pub type Planes = [[f32; 4]; 6];

fn dist(p: [f32; 3], min: [f32; 3], max: [f32; 3]) -> f32 {
    let d = |k: usize| (min[k] - p[k]).max(0.0).max(p[k] - max[k]);
    let (x, y, z) = (d(0), d(1), d(2));
    sqrt(x * x + y * y + z * z)
}

fn sqrt(x: f32) -> f32 {
    // Newton from a bit-level guess: enough for distances.
    if x <= 0.0 {
        return 0.0;
    }
    let mut g = f32::from_bits((x.to_bits() >> 1) + 0x1fbd_1df5);
    g = 0.5 * (g + x / g);
    0.5 * (g + x / g)
}

/// Whether a box is at least partly inside the planes.
pub fn visible(planes: &Planes, min: [f32; 3], max: [f32; 3]) -> bool {
    planes.iter().all(|p| {
        let x = if p[0] >= 0.0 { max[0] } else { min[0] };
        let y = if p[1] >= 0.0 { max[1] } else { min[1] };
        let z = if p[2] >= 0.0 { max[2] } else { min[2] };
        p[0] * x + p[1] * y + p[2] * z + p[3] >= 0.0
    })
}

/// The extent of what a batch draws, for a range: the end of its indices or instances.
fn whole(b: &Batch) -> u32 {
    if kind::instanced(b.kind) {
        b.inst_count
    } else {
        b.idx_count
    }
}

/// Appends the runs of `chosen` subs of every batch in `first .. first + count`.
fn runs(t: &Tables, first: u32, count: u32, chosen: &[bool], out: &mut Vec<Item>) {
    for bi in first..first + count {
        let b = &t.batches[bi as usize];
        if b.spans == NO_SPANS {
            continue;
        }
        let s = &t.spans[b.spans as usize..b.spans as usize + chosen.len() + 1];
        let mut k = 0;
        while k < chosen.len() {
            if !chosen[k] {
                k += 1;
                continue;
            }
            let start = k;
            while k < chosen.len() && chosen[k] {
                k += 1;
            }
            let (from, to) = (s[start], s[k]);
            if to > from {
                out.push(Item { batch: bi, from, to, cell: RESIDENT });
            }
        }
    }
}

fn all(t: &Tables, first: u32, count: u32, cell: u32, out: &mut Vec<Item>) {
    for bi in first..first + count {
        let w = whole(&t.batches[bi as usize]);
        if w > 0 {
            out.push(Item { batch: bi, from: 0, to: w, cell });
        }
    }
}

/// Chooses this frame's draws. `ready(cell)`: whether a cell's record of `NEAR` is in memory; a cell within the
/// near reach whose record is not is drawn at the mid level, and `wanted` hears of it (with its distance), so the
/// runtime reads it.
pub fn select(t: &Tables, planes: &Planes, eye: [f32; 3], reach: &Reach, ready: &dyn Fn(u32) -> bool, wanted: &mut dyn FnMut(u32, f32), out: &mut Vec<Item>) -> Counts {
    let c = t.city;
    let mut counts = Counts::default();
    let (bc, rb) = (c.block_cells, c.region_blocks);
    let (bw, rw) = (c.cell * bc as f32, c.cell * (bc * rb) as f32);
    let (y0, y1) = (c.y_min, c.y_max);
    let (bxn, bzn) = (c.blocks_x(), c.blocks_z());
    for rz in 0..c.regions_z() {
        for rx in 0..c.regions_x() {
            let r = &t.regions[(rz * c.regions_x() + rx) as usize];
            let rmin = [c.x0 + rx as f32 * rw, y0, c.z0 + rz as f32 * rw];
            let rmax = [rmin[0] + rw, y1, rmin[2] + rw];
            if !visible(planes, rmin, rmax) {
                continue;
            }
            if dist(eye, rmin, rmax) >= reach.mid {
                if r.far.count > 0 {
                    counts.places[2] += 1;
                    all(t, r.far.first, r.far.count, RESIDENT, out);
                }
                continue;
            }
            let mut far_blocks = [false; 64];
            let mut any_far = false;
            for j in 0..rb {
                for i in 0..rb {
                    let (bx, bz) = (rx * rb + i, rz * rb + j);
                    if bx >= bxn || bz >= bzn {
                        continue;
                    }
                    let bmin = [c.x0 + bx as f32 * bw, y0, c.z0 + bz as f32 * bw];
                    let bmax = [bmin[0] + bw, y1, bmin[2] + bw];
                    if !visible(planes, bmin, bmax) {
                        continue;
                    }
                    let d = dist(eye, bmin, bmax);
                    if d >= reach.mid {
                        far_blocks[(j * rb + i) as usize] = true;
                        any_far = true;
                        continue;
                    }
                    let block = &t.blocks[(bz * bxn + bx) as usize];
                    if d >= reach.near {
                        if block.mid.count > 0 {
                            counts.places[1] += 1;
                            all(t, block.mid.first, block.mid.count, RESIDENT, out);
                        }
                        continue;
                    }
                    let mut mid_cells = [false; 64];
                    let mut any_mid = false;
                    for cj in 0..bc {
                        for ci in 0..bc {
                            let (cx, cz) = (bx * bc + ci, bz * bc + cj);
                            if cx >= c.cells_x || cz >= c.cells_z {
                                continue;
                            }
                            let cmin = [c.x0 + cx as f32 * c.cell, y0, c.z0 + cz as f32 * c.cell];
                            let cmax = [cmin[0] + c.cell, y1, cmin[2] + c.cell];
                            let dc = dist(eye, cmin, cmax);
                            if dc >= reach.near {
                                mid_cells[(cj * bc + ci) as usize] = true;
                                any_mid = true;
                                continue;
                            }
                            let ci_ = cz * c.cells_x + cx;
                            let cell = &t.cells[ci_ as usize];
                            if cell.near.count + cell.detail.count == 0 {
                                mid_cells[(cj * bc + ci) as usize] = true;
                                any_mid = true;
                                continue;
                            }
                            if !ready(ci_) {
                                wanted(ci_, dc);
                                mid_cells[(cj * bc + ci) as usize] = true;
                                any_mid = true;
                                continue;
                            }
                            if !visible(planes, cell.min, cell.max) {
                                continue;
                            }
                            counts.places[0] += 1;
                            all(t, cell.near.first, cell.near.count, ci_, out);
                            if dc < reach.detail && cell.detail.count > 0 {
                                counts.places[3] += 1;
                                all(t, cell.detail.first, cell.detail.count, ci_, out);
                            }
                        }
                    }
                    if any_mid && block.mid.count > 0 {
                        counts.places[1] += 1;
                        runs(t, block.mid.first, block.mid.count, &mid_cells[..(bc * bc) as usize], out);
                    }
                }
            }
            if any_far && r.far.count > 0 {
                counts.places[2] += 1;
                runs(t, r.far.first, r.far.count, &far_blocks[..(rb * rb) as usize], out);
            }
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_root_is_close() {
        for x in [0.25f32, 1.0, 2.0, 1e4, 3.7e6] {
            assert!((sqrt(x) - x.sqrt()).abs() / x.sqrt() < 1e-4, "{x}");
        }
    }

    #[test]
    fn runs_follow_the_spans() {
        let city = City { cell: 128.0, cells_x: 4, cells_z: 4, block_cells: 2, region_blocks: 2, ..Default::default() };
        let batches = [Batch { kind: kind::SOLID, idx_count: 30, spans: 0, ..Default::default() }];
        // Four cells of a block: 3, 0, 6 and 1 triangles.
        let spans = [0, 9, 9, 27, 30];
        let t = Tables { city: &city, cells: &[], blocks: &[], regions: &[], batches: &batches, spans: &spans };
        let mut out = Vec::new();
        runs(&t, 0, 1, &[true, true, false, true], &mut out);
        assert_eq!(out, [Item { batch: 0, from: 0, to: 9, cell: RESIDENT }, Item { batch: 0, from: 27, to: 30, cell: RESIDENT }]);
    }
}
