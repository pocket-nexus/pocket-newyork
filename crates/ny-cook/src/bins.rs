//! What the compiler collects before it writes: triangles in world space, binned by level of detail, place, program
//! and picture; then cut into batches a device draws with `u16` indices and positions normalized over each batch.
//!
//! A mid bin keeps, with each triangle, the cell of its block it belongs to, and a far bin the block of its region
//! (its *sub*): their batches are ordered by it, with a `SPAN` record of where each starts.

use crate::geom::{self, Aabb};
use ny_pack::{self as pack, Batch, City, Draws, GroundVertex, Instance, SolidVertex, WallVertex, NO_SPANS};
use std::collections::{BTreeMap, HashMap};

/// A vertex before it is quantized. `a`, `b`: colour and glow of a solid, tint of a wall or the ground.
#[derive(Clone, Copy, Debug, Default)]
pub struct Vx {
    pub p: [f32; 3],
    pub n: [i8; 3],
    pub ao: u8,
    pub a: [u8; 4],
    pub b: [u8; 4],
    pub uv: [f32; 2],
}

#[derive(Default)]
pub struct Bin {
    pub verts: Vec<Vx>,
    pub idx: Vec<u32>,
    /// Per triangle: the cell of the block (mid) or the block of the region (far) it belongs to.
    pub sub: Vec<u8>,
    pub bounds: Option<Aabb>,
}

impl Bin {
    /// Adds vertices and the triangles over them (indices into `verts`).
    pub fn add(&mut self, verts: &[Vx], tris: &[u32], sub: u8) {
        let base = self.verts.len() as u32;
        let b = self.bounds.get_or_insert(Aabb::EMPTY);
        for v in verts {
            b.add(v.p);
        }
        self.verts.extend_from_slice(verts);
        self.idx.extend(tris.iter().map(|i| base + i));
        self.sub.extend(std::iter::repeat_n(sub, tris.len() / 3));
    }
}

/// Level of detail, place (cell, block or region index), program, picture (or shape, for instances).
pub type Key = (u8, u32, u32, u32);

/// A shape drawn at many places: its vertices in its own frame, written once.
pub struct Shape {
    pub bin: Bin,
}

/// The instances of one shape in one place, each with its sub.
#[derive(Default)]
pub struct InstBin {
    pub list: Vec<(u8, Instance)>,
    pub bounds: Option<Aabb>,
}

pub struct Bins {
    pub city: City,
    pub map: BTreeMap<Key, Bin>,
    pub shapes: Vec<Shape>,
    pub inst: BTreeMap<Key, InstBin>,
}

pub const NEAR: u8 = 0;
pub const MID: u8 = 1;
pub const FAR: u8 = 2;
/// A cell's small things.
pub const DETAIL: u8 = 3;

impl Bins {
    pub fn new(city: City) -> Bins {
        Bins { city, map: BTreeMap::new(), shapes: Vec::new(), inst: BTreeMap::new() }
    }

    fn cell_of(&self, p: [f32; 3]) -> (u32, u32) {
        let c = &self.city;
        let cx = (((p[0] - c.x0) / c.cell).floor() as i64).clamp(0, c.cells_x as i64 - 1) as u32;
        let cz = (((p[2] - c.z0) / c.cell).floor() as i64).clamp(0, c.cells_z as i64 - 1) as u32;
        (cx, cz)
    }

    /// The place at `lod` that a point belongs to, and its sub there.
    pub fn place(&self, lod: u8, p: [f32; 3]) -> (u32, u8) {
        let c = &self.city;
        let (cx, cz) = self.cell_of(p);
        let (bc, rb) = (c.block_cells, c.region_blocks);
        match lod {
            NEAR | DETAIL => (cz * c.cells_x + cx, 0),
            MID => ((cz / bc) * c.blocks_x() + cx / bc, ((cz % bc) * bc + cx % bc) as u8),
            _ => {
                let (bx, bz) = (cx / bc, cz / bc);
                ((bz / rb) * c.regions_x() + bx / rb, ((bz % rb) * rb + bx % rb) as u8)
            }
        }
    }

    pub fn bin(&mut self, lod: u8, place: u32, kind: u32, tex: u32) -> &mut Bin {
        self.map.entry((lod, place, kind, tex)).or_default()
    }

    pub fn shape(&mut self, verts: &[Vx], idx: &[u32]) -> u32 {
        let mut bin = Bin::default();
        bin.add(verts, idx, 0);
        self.shapes.push(Shape { bin });
        self.shapes.len() as u32 - 1
    }

    /// An instance of `shape` in a place; `bounds`: where it stands in the world.
    pub fn instance(&mut self, lod: u8, place: (u32, u8), kind: u32, shape: u32, inst: Instance, bounds: &Aabb) {
        let b = self.inst.entry((lod, place.0, kind, shape)).or_default();
        b.list.push((place.1, inst));
        b.bounds.get_or_insert(Aabb::EMPTY).union(bounds);
    }

    /// Triangles each level holds, instances counted.
    pub fn triangles(&self) -> [usize; 4] {
        let mut t = [0; 4];
        for ((lod, ..), b) in &self.map {
            t[*lod as usize] += b.idx.len() / 3;
        }
        for ((lod, _, _, shape), b) in &self.inst {
            t[*lod as usize] += self.shapes[*shape as usize].bin.idx.len() / 3 * b.list.len();
        }
        t
    }
}

/// The pack's tables and streams.
#[derive(Default)]
pub struct Written {
    pub cells: Vec<pack::Cell>,
    pub blocks: Vec<pack::Block>,
    pub regions: Vec<pack::Region>,
    pub batches: Vec<Batch>,
    pub walls: Vec<WallVertex>,
    pub solids: Vec<SolidVertex>,
    pub grounds: Vec<GroundVertex>,
    pub indices: Vec<u16>,
    pub instances: Vec<Instance>,
    pub spans: Vec<u32>,
}

fn quantize(p: [f32; 3], origin: [f32; 3], span: [f32; 3]) -> [u16; 3] {
    let q = |k: usize| (((p[k] - origin[k]) / span[k]).clamp(0.0, 1.0) * 65535.0).round() as u16;
    [q(0), q(1), q(2)]
}

/// Offsets where each of `subs` sub-places starts in a sorted list of sub values, and the end, in `unit`s.
fn spans_of(sorted: &[u8], subs: usize, unit: u32) -> Vec<u32> {
    (0..=subs).map(|s| sorted.partition_point(|&x| (x as usize) < s) as u32 * unit).collect()
}

impl Written {
    /// Cuts a bin into batches of at most 65 536 vertices and appends them; with `subs`, its triangles are ordered
    /// by their sub and each batch gets a `SPAN` record.
    fn put(&mut self, kind: u32, tex: u32, bin: &Bin, subs: usize) {
        // Triangles in the order of their sub.
        let mut order_t: Vec<usize> = (0..bin.idx.len() / 3).collect();
        if subs > 0 {
            order_t.sort_by_key(|&t| bin.sub[t]);
        }
        let mut at = 0;
        while at < order_t.len() {
            let mut remap: HashMap<u32, u32> = HashMap::new();
            let mut local: Vec<u32> = Vec::new();
            let mut order: Vec<u32> = Vec::new();
            let mut subs_here: Vec<u8> = Vec::new();
            let mut end = at;
            while end < order_t.len() {
                let t = order_t[end];
                let tri = &bin.idx[t * 3..t * 3 + 3];
                let fresh = tri.iter().filter(|i| !remap.contains_key(i)).count();
                if order.len() + fresh > 65535 {
                    break;
                }
                for &i in tri {
                    let n = order.len() as u32;
                    let j = *remap.entry(i).or_insert_with(|| {
                        order.push(i);
                        n
                    });
                    local.push(j);
                }
                subs_here.push(bin.sub[t]);
                end += 1;
            }
            let mut b = Aabb::EMPTY;
            for &i in &order {
                b.add(bin.verts[i as usize].p);
            }
            let origin = b.min;
            let span = geom::sub(b.max, b.min).map(|s| s.max(1e-3));
            let vtx_first = match kind {
                pack::kind::WALL => self.walls.len(),
                pack::kind::GROUND => self.grounds.len(),
                _ => self.solids.len(),
            } as u32;
            for &i in &order {
                let v = &bin.verts[i as usize];
                let pos = quantize(v.p, origin, span);
                match kind {
                    pack::kind::WALL => {
                        let q = |x: f32| (x / pack::UV_RANGE * 32767.0).round().clamp(-32767.0, 32767.0) as i16;
                        self.walls.push(WallVertex { pos, normal: v.n, ao: v.ao, uv: [q(v.uv[0]), q(v.uv[1])], tint: v.a, pad: [0; 2] });
                    }
                    pack::kind::GROUND => self.grounds.push(GroundVertex { pos, normal: v.n, ao: v.ao, tint: v.a, pad: [0; 2] }),
                    _ => self.solids.push(SolidVertex { pos, normal: v.n, ao: v.ao, color: v.a, glow: v.b, pad: [0; 2] }),
                }
            }
            let idx_first = self.indices.len() as u32;
            self.indices.extend(local.iter().map(|&i| i as u16));
            let spans = if subs > 0 {
                let at = self.spans.len() as u32;
                self.spans.extend(spans_of(&subs_here, subs, 3));
                at
            } else {
                NO_SPANS
            };
            self.batches.push(Batch { kind, tex, vtx_first, vtx_count: order.len() as u32, idx_first, idx_count: local.len() as u32, origin, span, inst_first: 0, inst_count: 0, spans, pad: 0 });
            at = end;
        }
    }

    /// Every place's batches, in the order of the tables: a cell's near batches, then its detail batches.
    pub fn from_bins(bins: &Bins) -> Written {
        let c = bins.city;
        let mut w = Written::default();
        let counts = [(c.cells_x * c.cells_z) as usize, (c.blocks_x() * c.blocks_z()) as usize, (c.regions_x() * c.regions_z()) as usize];
        let subs_of = |lod: usize| match lod as u8 {
            MID => (c.block_cells * c.block_cells) as usize,
            FAR => (c.region_blocks * c.region_blocks) as usize,
            _ => 0,
        };
        // Every shape once: its batch, which instanced batches copy.
        let mut shape_batch: Vec<Batch> = Vec::with_capacity(bins.shapes.len());
        for s in &bins.shapes {
            let before = w.batches.len();
            w.put(pack::kind::SOLID, pack::NO_TEX, &s.bin, 0);
            assert_eq!(w.batches.len(), before + 1, "a shape must fit one batch");
            shape_batch.push(w.batches.pop().unwrap());
        }
        let mut by_place: [BTreeMap<u32, Vec<(&Key, &Bin)>>; 4] = Default::default();
        for (k, b) in &bins.map {
            if !b.idx.is_empty() {
                by_place[k.0 as usize].entry(k.1).or_default().push((k, b));
            }
        }
        let mut inst_by_place: [BTreeMap<u32, Vec<(&Key, &InstBin)>>; 4] = Default::default();
        for (k, b) in &bins.inst {
            inst_by_place[k.0 as usize].entry(k.1).or_default().push((k, b));
        }
        let put_all = |w: &mut Written, lod: usize, place: u32, bounds: &mut Aabb| -> Draws {
            let first = w.batches.len() as u32;
            let subs = subs_of(lod);
            if let Some(list) = by_place[lod].get(&place) {
                for (k, b) in list {
                    w.put(k.2, k.3, b, subs);
                    if let Some(bb) = &b.bounds {
                        bounds.union(bb);
                    }
                }
            }
            if let Some(list) = inst_by_place[lod].get(&place) {
                for (k, b) in list {
                    let mut sorted: Vec<&(u8, Instance)> = b.list.iter().collect();
                    sorted.sort_by_key(|e| e.0);
                    let mut batch = shape_batch[k.3 as usize];
                    batch.kind = k.2;
                    batch.inst_first = w.instances.len() as u32;
                    batch.inst_count = sorted.len() as u32;
                    batch.spans = if subs > 0 {
                        let at = w.spans.len() as u32;
                        let keys: Vec<u8> = sorted.iter().map(|e| e.0).collect();
                        w.spans.extend(spans_of(&keys, subs, 1));
                        at
                    } else {
                        NO_SPANS
                    };
                    w.instances.extend(sorted.iter().map(|e| e.1));
                    w.batches.push(batch);
                    if let Some(bb) = &b.bounds {
                        bounds.union(bb);
                    }
                }
            }
            Draws { first, count: w.batches.len() as u32 - first }
        };
        for lod in 0..3 {
            for place in 0..counts[lod] as u32 {
                let mut bounds = Aabb::EMPTY;
                let draws = put_all(&mut w, lod, place, &mut bounds);
                let detail = if lod == 0 { put_all(&mut w, DETAIL as usize, place, &mut bounds) } else { Draws::default() };
                let (min, max) = if bounds.is_empty() { ([0.0; 3], [0.0; 3]) } else { (bounds.min, bounds.max) };
                match lod {
                    0 => w.cells.push(pack::Cell { min, max, near: draws, detail }),
                    1 => w.blocks.push(pack::Block { min, max, mid: draws }),
                    _ => w.regions.push(pack::Region { min, max, far: draws }),
                }
            }
        }
        w
    }
}
