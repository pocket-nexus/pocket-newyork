//! What the simulation reads of the city, from the pack's sections (`ny_pack::sim`): the colliders, the ground,
//! the shore, the names, the road network, the cameras and the four entities as they start. Lookups the reference
//! makes through hash maps of cells (`nearColliders`, `blockAt`, `nearestRoads`) go through `Grid` here: the same
//! cells, kept as one sorted list.

use alloc::string::String;
use alloc::vec::Vec;
use ny_pack::sim::*;
use ny_pack::{read, table};

/// Items bucketed into square cells: every (cell, item) pair, sorted, so a cell's items are one run of the list,
/// in the order the items were added.
#[derive(Clone, Debug, Default)]
pub struct Grid {
    pub cell: f32,
    pairs: Vec<(i64, u32)>,
}

impl Grid {
    pub fn new(cell: f32) -> Grid {
        Grid { cell, pairs: Vec::new() }
    }

    pub fn key(ix: i64, iz: i64) -> i64 {
        ix * 1_000_000 + iz
    }

    pub fn cell_of(&self, v: f32) -> i64 {
        libm::floorf(v / self.cell) as i64
    }

    /// Puts item `i` in every cell its bounds touch.
    pub fn insert(&mut self, i: u32, min_x: f32, max_x: f32, min_z: f32, max_z: f32) {
        for ix in self.cell_of(min_x)..=self.cell_of(max_x) {
            for iz in self.cell_of(min_z)..=self.cell_of(max_z) {
                self.pairs.push((Grid::key(ix, iz), i));
            }
        }
    }

    /// As `insert`, for bounds in doubles (a cell's edge stays where the reference has it).
    pub fn insert_f64(&mut self, i: u32, min_x: f64, max_x: f64, min_z: f64, max_z: f64) {
        let c = self.cell as f64;
        let cell = |v: f64| libm::floor(v / c) as i64;
        for ix in cell(min_x)..=cell(max_x) {
            for iz in cell(min_z)..=cell(max_z) {
                self.pairs.push((Grid::key(ix, iz), i));
            }
        }
    }

    pub fn cell_of_f64(&self, v: f64) -> i64 {
        libm::floor(v / self.cell as f64) as i64
    }

    pub fn finish(&mut self) {
        self.pairs.sort_unstable();
    }

    /// The items of one cell, in the order they were added.
    pub fn at(&self, ix: i64, iz: i64) -> impl Iterator<Item = u32> + '_ {
        let k = Grid::key(ix, iz);
        let from = self.pairs.partition_point(|p| p.0 < k);
        self.pairs[from..].iter().take_while(move |p| p.0 == k).map(|p| p.1)
    }
}

/// A curb island.
#[derive(Clone, Debug, Default)]
pub struct Block {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub poly: Vec<[f64; 2]>,
    pub rect: bool,
    pub median: bool,
    pub park: f32,
}

/// A named centreline's segment.
#[derive(Clone, Copy, Debug, Default)]
pub struct Segment {
    pub road: u32,
    pub a: [f64; 2],
    pub b: [f64; 2],
}

#[derive(Clone, Debug, Default)]
pub struct World {
    pub info: SimWorld,
    pub colliders: Vec<Collider>,
    pub collider_grid: Grid,
    pub shore: Vec<[f32; 2]>,
    pub blocks: Vec<Block>,
    pub block_grid: Grid,
    /// Name of each road (a `names` line).
    pub roads: Vec<u32>,
    pub segments: Vec<Segment>,
    pub road_grid: Grid,
    pub streets: Vec<Street>,
    pub landmarks: Vec<Mark>,
    pub teleports: Vec<Mark>,
    pub names: Vec<String>,
    pub cameras: Vec<CameraPreset>,
    /// Person, car, cyclist, helicopter.
    pub entities: [EntityStart; 4],
    pub nodes: Vec<TrafficNode>,
    pub edges: Vec<TrafficEdge>,
    pub lanes: Vec<f32>,
    pub moves: Vec<TrafficMove>,
    pub outs: Vec<i8>,
}

/// The reference's cell sizes: colliders 48 m (controllers.js), blocks and roads 64 m (city-config.js).
pub const COLLIDER_CELL: f32 = 48.0;
pub const BLOCK_CELL: f32 = 64.0;
pub const ROAD_CELL: f32 = 64.0;

impl World {
    /// The world of the pack's sections; `get(tag)` returns a section's bytes.
    pub fn from_sections(mut get: impl FnMut(u32) -> Option<Vec<u8>>) -> Result<World, String> {
        let mut need = |t: u32| get(t).ok_or_else(|| alloc::format!("pack has no {} section", String::from_utf8_lossy(&t.to_le_bytes())));
        let info: SimWorld = read(&need(SIMW)?, 0).ok_or("SIMW record")?;
        let colliders: Vec<Collider> = table(&need(COLL)?);
        let shore: Vec<[f32; 2]> = table(&need(SHOR)?);
        let sblocks: Vec<SimBlock> = table(&need(SBLK)?);
        let points: Vec<[f64; 2]> = table(&need(SPTS)?);
        let roads: Vec<Road> = table(&need(ROAD)?);
        let streets: Vec<Street> = table(&need(STRT)?);
        let landmarks: Vec<Mark> = table(&need(LMRK)?);
        let teleports: Vec<Mark> = table(&need(TELE)?);
        let names: Vec<String> = String::from_utf8_lossy(&need(NAME)?).split('\n').map(String::from).collect();
        let nodes: Vec<TrafficNode> = table(&need(NODE)?);
        let edges: Vec<TrafficEdge> = table(&need(EDGE)?);
        let lanes: Vec<f32> = table(&need(LANE)?);
        let moves: Vec<TrafficMove> = table(&need(MOVE)?);
        let outs: Vec<i8> = need(MOUT)?.into_iter().map(|b| b as i8).collect();
        let cameras: Vec<CameraPreset> = table(&need(CAMS)?);
        let ents: Vec<EntityStart> = table(&need(ENTS)?);
        if ents.len() != 4 {
            return Err("ENTS holds four entities".into());
        }
        let blocks = sblocks
            .iter()
            .map(|b| Block {
                min: [b.min_x, b.min_z],
                max: [b.max_x, b.max_z],
                poly: points[b.first as usize..(b.first + b.count) as usize].to_vec(),
                rect: b.flags & block::RECT != 0,
                median: b.flags & block::MEDIAN != 0,
                park: b.park,
            })
            .collect();
        let mut segments = Vec::new();
        for (i, r) in roads.iter().enumerate() {
            let pts = &points[r.first as usize..(r.first + r.count) as usize];
            for w in pts.windows(2) {
                segments.push(Segment { road: i as u32, a: w[0], b: w[1] });
            }
        }
        let mut w = World {
            info,
            colliders,
            shore,
            blocks,
            roads: roads.iter().map(|r| r.name).collect(),
            segments,
            streets,
            landmarks,
            teleports,
            names,
            cameras,
            entities: [ents[0], ents[1], ents[2], ents[3]],
            nodes,
            edges,
            lanes,
            moves,
            outs,
            ..Default::default()
        };
        w.index();
        Ok(w)
    }

    /// Builds the cell lists.
    pub fn index(&mut self) {
        let mut g = Grid::new(COLLIDER_CELL);
        for (i, c) in self.colliders.iter().enumerate() {
            g.insert(i as u32, c.min_x, c.max_x, c.min_z, c.max_z);
        }
        g.finish();
        self.collider_grid = g;
        let mut g = Grid::new(BLOCK_CELL);
        for (i, b) in self.blocks.iter().enumerate() {
            g.insert(i as u32, b.min[0], b.max[0], b.min[1], b.max[1]);
        }
        g.finish();
        self.block_grid = g;
        let mut g = Grid::new(ROAD_CELL);
        for (i, s) in self.segments.iter().enumerate() {
            g.insert_f64(i as u32, s.a[0].min(s.b[0]), s.a[0].max(s.b[0]), s.a[1].min(s.b[1]), s.a[1].max(s.b[1]));
        }
        g.finish();
        self.road_grid = g;
    }

    /// A line of `NAME`.
    pub fn name(&self, id: u16) -> &str {
        self.names.get(id as usize).map(|s| s.as_str()).unwrap_or("")
    }

    /// The camera preset whose id is `id` (the reference's `CAMERA_PRESETS` ids).
    pub fn camera_named(&self, id: &str) -> Option<usize> {
        self.cameras.iter().position(|c| self.names.get(c.id as usize).map(|s| s.as_str()) == Some(id))
    }

    /// The teleport whose key is `id` (`TELEPORT_LOCATIONS`).
    pub fn teleport_named(&self, id: &str) -> Option<usize> {
        self.teleports.iter().position(|t| self.names.get(t.name as usize).map(|s| s.as_str()) == Some(id))
    }

    /// West and east shore x of Manhattan at `z` (`getIslandBoundsAtZ`).
    pub fn shore_at(&self, z: f32) -> (f32, f32) {
        let n = self.shore.len();
        if n < 2 {
            return (f32::MIN, f32::MAX);
        }
        let f = (z - self.info.shore_z0) / self.info.shore_step;
        let k = (libm::floorf(f) as i64).clamp(0, n as i64 - 2) as usize;
        let t = (f - k as f32).clamp(0.0, 1.0);
        let (a, b) = (self.shore[k], self.shore[k + 1]);
        (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t)
    }

    /// The colliders whose cells come within `r` of (x, z), in their original order (`nearColliders`).
    pub fn near_colliders(&self, x: f32, z: f32, r: f32, out: &mut Vec<u32>) {
        out.clear();
        let g = &self.collider_grid;
        for ix in g.cell_of(x - r)..=g.cell_of(x + r) {
            for iz in g.cell_of(z - r)..=g.cell_of(z + r) {
                out.extend(g.at(ix, iz));
            }
        }
        out.sort_unstable();
        out.dedup();
    }

    /// The curb island under (x, z) (`blockAt`).
    pub fn block_at(&self, x: f32, z: f32) -> Option<&Block> {
        let g = &self.block_grid;
        for i in g.at(g.cell_of(x), g.cell_of(z)) {
            let b = &self.blocks[i as usize];
            if x < b.min[0] || x > b.max[0] || z < b.min[1] || z > b.max[1] {
                continue;
            }
            if b.rect || point_in_poly(&b.poly, x as f64, z as f64) {
                return Some(b);
            }
        }
        None
    }

    /// On a raised curb island, not in a park (`isPointOnSidewalk`).
    pub fn on_sidewalk(&self, x: f32, z: f32) -> bool {
        self.block_at(x, z).map(|b| !(b.park > 0.6 && !b.median)).unwrap_or(false)
    }

    /// The height a mode stands at (`getGroundHeight`): the park lawns, the sidewalks for one on foot, the street.
    pub fn ground(&self, x: f32, z: f32, on_foot: bool) -> f32 {
        let cp = self.info.central_park;
        if x > cp[0] && x < cp[1] && z > cp[2] && z < cp[3] {
            return 1.24;
        }
        let l = self.info.lawn;
        if !self.info.lawn_top.is_nan() && x > l[0] && x < l[1] && z > l[2] && z < l[3] {
            return self.info.lawn_top;
        }
        if on_foot && self.on_sidewalk(x, z) {
            return self.info.sidewalk_height;
        }
        0.0
    }
}

pub fn point_in_poly(poly: &[[f64; 2]], x: f64, z: f64) -> bool {
    let mut inside = false;
    let n = poly.len();
    if n == 0 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > z) != (b[1] > z) && x < (b[0] - a[0]) * (z - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}
