//! The trees nearest the eye, at the reference's finer levels. The cells hold every tree as its species' crown of
//! whole-crown cards: the reference's level 2, which it draws past 420 m. Nearer, the reference draws a tree at
//! level 1 (cards along its branches), and within 105 m at level 0 (a card on every twig), each in one of two forms.
//! A frame finds the trees nearest the eye in the cells around it, places each at a finer level as an instance of
//! that level's model, and returns the distance within which the cells' crowns are not drawn, in the view or in the
//! sun's map: a device's leaf programs take it with the eye (`uCut`) and work each tree's distance out as
//! [`distance`] does. A tree out of view is placed too: its shadow falls into the view.

use alloc::format;
use alloc::vec::Vec;

use ny_pack::{kind, Batch, Cell, City, Instance, Model};
use ny_sim::math::V3;
use ny_view::Planes;

/// How far and how many trees a device draws finer: level 0 within `fine` (at most `fine_count` trees, those in
/// view first), level 1 within `reach` (at most `count` trees in all).
#[derive(Clone, Copy, Debug)]
pub struct Reach {
    pub fine: f32,
    pub fine_count: usize,
    pub reach: f32,
    pub count: usize,
}

/// The middle of a crown above a tree's foot, in the tree's frame (`py` in the reference's `createTreeField`).
pub const MID_CROWN: f32 = 6.0;
/// Trees nearer each other than this (metres) in distance from the eye are not parted by the cut: a GPU works the
/// distance out again, and may round it the other way.
const GAP: f32 = 0.05;

/// A tree's distance from `eye`: to the middle of its crown.
pub fn distance(eye: V3, t: &Instance) -> f32 {
    let (dx, dy, dz) = (t.rows[0][3] - eye.x, t.rows[1][3] + MID_CROWN * t.rows[1][1] - eye.y, t.rows[2][3] - eye.z);
    libm::sqrtf(dx * dx + dy * dy + dz * dz)
}

struct Species {
    /// The first vertex of its crown's shape, which the cells' batches of it draw.
    far: u32,
    /// Its crown's bounds in its own frame: the middle and the radius.
    middle: [f32; 3],
    radius: f32,
    /// The models of its levels 0 and 1, by form.
    levels: [[usize; 2]; 2],
}

struct Found {
    distance: f32,
    species: u16,
    seen: bool,
    fine: bool,
    tree: Instance,
}

pub struct Trees {
    species: Vec<Species>,
    found: Vec<Found>,
}

impl Trees {
    /// The pack's models of the trees' levels (`tree:<species>` and `tree:<species>:<form>:<level>`); a species
    /// without all four of its finer levels stays a crown.
    pub fn new(models: &[Model], batches: &[Batch]) -> Trees {
        let index = |name: &str| models.iter().position(|m| m.name() == name);
        let mut species = Vec::new();
        for m in models {
            let Some(name) = m.name().strip_prefix("tree:").filter(|n| !n.contains(':')) else { continue };
            let level = |form: u8, level: u8| index(&format!("tree:{name}:{form}:{level}"));
            let (Some(a), Some(b), Some(c), Some(d)) = (level(0, 0), level(0, 1), level(1, 0), level(1, 1)) else { continue };
            let Some(far) = batches.get(m.first as usize) else { continue };
            let half = far.span.map(|s| s * 0.5);
            species.push(Species {
                far: far.vtx_first,
                middle: [0, 1, 2].map(|k| far.origin[k] + half[k]),
                radius: libm::sqrtf(half[0] * half[0] + half[1] * half[1] + half[2] * half[2]),
                levels: [[a, b], [c, d]],
            });
        }
        Trees { species, found: Vec::new() }
    }

    /// Places the trees within `r.reach` of `eye` at their finer levels onto `out` (level 0 to those in view of
    /// `planes` first), and returns the distance within which the cells' crowns are not drawn (0: all are).
    /// `record(cell)`: the bytes of the instances of a cell's record of `NEAR`, when it is in.
    #[allow(clippy::too_many_arguments)]
    pub fn place<'a>(&mut self, eye: V3, planes: &Planes, r: &Reach, city: &City, cells: &[Cell], batches: &[Batch], record: impl Fn(u32) -> Option<&'a [u8]>, out: &mut Vec<(usize, Instance)>) -> f32 {
        self.found.clear();
        if self.species.is_empty() || r.count == 0 {
            return 0.0;
        }
        // (a little past the reach: the cut falls between a tree found and the next)
        let look = r.reach + 1.0;
        let cell_of = |x: f32, z: f32| (libm::floorf((x - city.x0) / city.cell) as i32, libm::floorf((z - city.z0) / city.cell) as i32);
        let (lo, hi) = (cell_of(eye.x - look, eye.z - look), cell_of(eye.x + look, eye.z + look));
        for cz in lo.1.max(0)..=hi.1.min(city.cells_z as i32 - 1) {
            for cx in lo.0.max(0)..=hi.0.min(city.cells_x as i32 - 1) {
                let index = cz as u32 * city.cells_x + cx as u32;
                let Some(cell) = cells.get(index as usize) else { continue };
                if cell.near.count == 0 {
                    continue;
                }
                let Some(bytes) = record(index) else { continue };
                for b in &batches[cell.near.first as usize..(cell.near.first + cell.near.count) as usize] {
                    if b.kind != kind::LEAF_INST {
                        continue;
                    }
                    let Some(s) = self.species.iter().position(|s| s.far == b.vtx_first) else { continue };
                    for k in 0..b.inst_count as usize {
                        let Some(tree) = ny_pack::read::<Instance>(bytes, (b.inst_first as usize + k) * core::mem::size_of::<Instance>()) else { break };
                        let d = distance(eye, &tree);
                        if d < look {
                            self.found.push(Found { distance: d, species: s as u16, seen: false, fine: false, tree });
                        }
                    }
                }
            }
        }
        self.found.sort_unstable_by(|a, b| a.distance.total_cmp(&b.distance));
        // The nearest within the reach, as many as the count allows.
        let mut n = self.found.partition_point(|f| f.distance < r.reach).min(r.count);
        while n > 0 && n < self.found.len() && self.found[n].distance - self.found[n - 1].distance < GAP {
            n -= 1;
        }
        let cut = match n {
            0 => 0.0,
            _ if n == self.found.len() => look,
            _ => (self.found[n - 1].distance + self.found[n].distance) * 0.5,
        };
        // Level 0 to the nearest within `fine` in view, then to the nearest out of it.
        for f in &mut self.found[..n] {
            let s = &self.species[f.species as usize];
            let t = &f.tree.rows;
            let c = [0, 1, 2].map(|k| t[k][0] * s.middle[0] + t[k][1] * s.middle[1] + t[k][2] * s.middle[2] + t[k][3]);
            let scale = [0, 1, 2].map(|k| libm::sqrtf(t[0][k] * t[0][k] + t[1][k] * t[1][k] + t[2][k] * t[2][k])).into_iter().fold(0.0, f32::max);
            // (the finer levels' cards reach a little past the crown's)
            let rr = s.radius * scale * 1.25;
            f.seen = ny_view::visible(planes, [c[0] - rr, c[1] - rr, c[2] - rr], [c[0] + rr, c[1] + rr, c[2] + rr]);
        }
        let mut fine = 0;
        for pass in [true, false] {
            for f in &mut self.found[..n] {
                if f.seen == pass && fine < r.fine_count && f.distance < r.fine {
                    f.fine = true;
                    fine += 1;
                }
            }
        }
        for f in &self.found[..n] {
            let level = if f.fine { 0 } else { 1 };
            let form = (255 - f.tree.tint[3]).min(1) as usize;
            let t = f.tree.tint;
            out.push((self.species[f.species as usize].levels[form][level], Instance { rows: f.tree.rows, tint: [t[0], t[1], t[2], 255] }));
        }
        cut
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ny_pack::Draws;

    fn model(name: &str, first: u32) -> Model {
        let mut n = [0u8; 24];
        n[..name.len()].copy_from_slice(name.as_bytes());
        Model { name: n, first, count: 1, ..Default::default() }
    }

    /// A tree standing at `x` along a row, in form `form`.
    fn tree(x: f32, form: u8) -> Instance {
        Instance { rows: [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 10.0]], tint: [128, 128, 128, 255 - form] }
    }

    #[test]
    fn the_nearest_finer_and_the_cut_between() {
        // One cell holding five trees of one species as its crown (shape at vertex 100); the species' models.
        let crown = Batch { kind: kind::LEAF_INST, vtx_first: 100, origin: [-3.0, 0.0, -3.0], span: [6.0, 12.0, 6.0], inst_count: 5, ..Default::default() };
        let level = |v: u32| Batch { kind: kind::LEAF_INST, vtx_first: v, ..Default::default() };
        let batches = [crown, crown, level(200), level(300), level(400), level(500)];
        let models = [model("tree:elm", 1), model("tree:elm:0:0", 2), model("tree:elm:0:1", 3), model("tree:elm:1:0", 4), model("tree:elm:1:1", 5)];
        let city = City { cell: 128.0, cells_x: 1, cells_z: 1, ..Default::default() };
        let cells = [Cell { min: [0.0; 3], max: [128.0, 20.0, 128.0], near: Draws { first: 0, count: 1 }, detail: Draws::default() }];
        let mut record = Vec::new();
        for (k, x) in [50.0, 10.0, 40.0, 20.0, 30.0].into_iter().enumerate() {
            record.extend_from_slice(ny_pack::bytes_of(&tree(x, (k % 2) as u8)));
        }
        let mut trees = Trees::new(&models, &batches);
        let mut out = Vec::new();
        // (the eye level with the crowns' middles, 10, 20 … 50 m from them; every plane passes everything)
        let eye = V3 { x: 0.0, y: MID_CROWN, z: 10.0 };
        let all: Planes = [[0.0, 0.0, 0.0, 1.0]; 6];
        let r = Reach { fine: 15.0, fine_count: 1, reach: 100.0, count: 3 };
        let cut = trees.place(eye, &all, &r, &city, &cells, &batches, |_| Some(record.as_slice()), &mut out);
        assert_eq!(cut, 35.0);
        // 10 m: form 1 at level 0; 20 m: form 1 at level 1; 30 m: form 0 at level 1; each opaque.
        let placed: Vec<(usize, f32)> = out.iter().map(|(m, t)| (*m, t.rows[0][3])).collect();
        assert_eq!(placed, [(3, 10.0), (4, 20.0), (2, 30.0)]);
        assert!(out.iter().all(|(_, t)| t.tint[3] == 255));
        // Nothing found: nothing cut.
        out.clear();
        assert_eq!(trees.place(V3 { x: 0.0, y: 500.0, z: 10.0 }, &all, &r, &city, &cells, &batches, |_| Some(record.as_slice()), &mut out), 0.0);
        assert!(out.is_empty());
    }
}
