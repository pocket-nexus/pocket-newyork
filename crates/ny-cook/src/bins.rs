//! What the compiler collects before it writes: triangles in world space, binned by level of detail, place, program
//! and picture; then cut into batches a device draws with `u16` indices and positions normalized over each batch.

use crate::geom::{self, Aabb};
use ny_pack::{self as pack, Batch, City, Draws, GroundVertex, SolidVertex, WallVertex};
use std::collections::BTreeMap;

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
    pub bounds: Option<Aabb>,
}

impl Bin {
    /// Adds vertices and the triangles over them (indices into `verts`).
    pub fn add(&mut self, verts: &[Vx], tris: &[u32]) {
        let base = self.verts.len() as u32;
        let b = self.bounds.get_or_insert(Aabb::EMPTY);
        for v in verts {
            b.add(v.p);
        }
        self.verts.extend_from_slice(verts);
        self.idx.extend(tris.iter().map(|i| base + i));
    }
}

/// Level of detail, place (cell, block or region index), program, picture.
pub type Key = (u8, u32, u32, u32);

pub struct Bins {
    pub city: City,
    pub map: BTreeMap<Key, Bin>,
}

pub const NEAR: u8 = 0;
pub const MID: u8 = 1;
pub const FAR: u8 = 2;

impl Bins {
    pub fn new(city: City) -> Bins {
        Bins { city, map: BTreeMap::new() }
    }

    /// The place at `lod` that a point belongs to.
    pub fn place(&self, lod: u8, p: [f32; 3]) -> u32 {
        let c = &self.city;
        let cx = (((p[0] - c.x0) / c.cell).floor() as i64).clamp(0, c.cells_x as i64 - 1) as u32;
        let cz = (((p[2] - c.z0) / c.cell).floor() as i64).clamp(0, c.cells_z as i64 - 1) as u32;
        match lod {
            NEAR => cz * c.cells_x + cx,
            MID => (cz / c.block_cells) * c.blocks_x() + cx / c.block_cells,
            _ => (cz / c.block_cells / c.region_blocks) * c.regions_x() + cx / c.block_cells / c.region_blocks,
        }
    }

    pub fn bin(&mut self, lod: u8, place: u32, kind: u32, tex: u32) -> &mut Bin {
        self.map.entry((lod, place, kind, tex)).or_default()
    }

    pub fn triangles(&self) -> [usize; 3] {
        let mut t = [0; 3];
        for ((lod, ..), b) in &self.map {
            t[*lod as usize] += b.idx.len() / 3;
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
}

fn quantize(p: [f32; 3], origin: [f32; 3], span: [f32; 3]) -> [u16; 3] {
    let q = |k: usize| (((p[k] - origin[k]) / span[k]).clamp(0.0, 1.0) * 65535.0).round() as u16;
    [q(0), q(1), q(2)]
}

impl Written {
    /// Cuts a bin into batches of at most 65 536 vertices and appends them.
    fn put(&mut self, kind: u32, tex: u32, bin: &Bin) {
        let mut at = 0;
        while at < bin.idx.len() {
            // Triangles in order until their vertices would pass the limit.
            let mut remap: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
            let mut local: Vec<u32> = Vec::new();
            let mut order: Vec<u32> = Vec::new();
            let mut end = at;
            while end < bin.idx.len() {
                let fresh = bin.idx[end..end + 3].iter().filter(|i| !remap.contains_key(i)).count();
                if order.len() + fresh > 65535 {
                    break;
                }
                for &i in &bin.idx[end..end + 3] {
                    let n = order.len() as u32;
                    let j = *remap.entry(i).or_insert_with(|| {
                        order.push(i);
                        n
                    });
                    local.push(j);
                }
                end += 3;
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
            self.batches.push(Batch { kind, tex, vtx_first, vtx_count: order.len() as u32, idx_first, idx_count: local.len() as u32, origin, span });
            at = end;
        }
    }

    /// Every place's batches, in the order of the tables.
    pub fn from_bins(bins: &Bins) -> Written {
        let c = bins.city;
        let mut w = Written::default();
        let counts = [(c.cells_x * c.cells_z) as usize, (c.blocks_x() * c.blocks_z()) as usize, (c.regions_x() * c.regions_z()) as usize];
        // The bins of each place, by level.
        let mut by_place: [BTreeMap<u32, Vec<(&Key, &Bin)>>; 3] = Default::default();
        for (k, b) in &bins.map {
            if !b.idx.is_empty() {
                by_place[k.0 as usize].entry(k.1).or_default().push((k, b));
            }
        }
        for lod in 0..3 {
            for place in 0..counts[lod] as u32 {
                let first = w.batches.len() as u32;
                let mut bounds = Aabb::EMPTY;
                if let Some(list) = by_place[lod].get(&place) {
                    for (k, b) in list {
                        w.put(k.2, k.3, b);
                        if let Some(bb) = &b.bounds {
                            bounds.union(bb);
                        }
                    }
                }
                let draws = Draws { first, count: w.batches.len() as u32 - first };
                let (min, max) = if bounds.is_empty() { ([0.0; 3], [0.0; 3]) } else { (bounds.min, bounds.max) };
                match lod {
                    0 => w.cells.push(pack::Cell { min, max, near: draws }),
                    1 => w.blocks.push(pack::Block { min, max, mid: draws }),
                    _ => w.regions.push(pack::Region { min, max, far: draws }),
                }
            }
        }
        w
    }
}
