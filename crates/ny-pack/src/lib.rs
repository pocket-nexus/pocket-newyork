//! The device pack (`NYPK`): what the scene compiler writes and a runtime reads.
//!
//! Little-endian. Header: `"NYPK"`, version, section count, zero; then one 16-byte entry per section (tag, offset,
//! size, zero). Sections start on 16-byte boundaries. A runtime reads the header and the table, then each section
//! it wants.
//!
//! The city is cut three ways. A *cell* is a square of `City::cell` metres, drawn at the near level of detail, and
//! its small things (street furniture, signs, rooftop plant) at the detail level, closer still. A *block* is
//! `City::block_cells` squared cells, drawn at the mid level. A *region* is `City::region_blocks` squared blocks,
//! drawn at the far level. Something larger than its cell (a bridge, a tall tower's wall) belongs
//! to the cell its centre lies in, and that cell's bounds grow to hold it.
//!
//! | Tag    | Contents |
//! | ------ | -------- |
//! | `META` | JSON: identity, sources, statistics, the profile; for tools |
//! | `CITY` | `City` |
//! | `CELL` | `Cell` table, row by row from the north-west |
//! | `BLCK` | `Block` table, row by row |
//! | `REGN` | `Region` table, row by row |
//! | `BTCH` | `Batch` table |
//! | `VWAL` | `WallVertex` records: the walls of the facade boxes |
//! | `VSOL` | `SolidVertex` records: everything coloured by vertex |
//! | `VGND` | `GroundVertex` records: surfaces under a picture that repeats over the ground |
//! | `IDX0` | `u16` indices, relative to each batch's first vertex |
//! | `INST` | `Instance` records of instanced batches |
//! | `NCEL` | `NearCell` table, one per cell: where its record of `NEAR` is |
//! | `NEAR` | per cell, the vertices, indices and instances its near and detail batches draw, read when the eye comes near |
//! | `SPAN` | `u32` offsets: per mid batch, where each of its block's cells starts in its indices (or instances), and the end (`City::block_cells² + 1`); per far batch the same for its region's blocks |
//! | `TEXS` | `TexEntry` table |
//! | `TDAT` | the pictures' levels, largest first, as `TexEntry` says |
//! | `PRST` | `Preset` table: the hours the reference offers |
//! | `SKYP` | `SkyHead`, then per hour (`PRST` order) its sky's panorama, `w × h` RGBA8 texels row by row: longitude from +x towards +z along a row, latitude from straight up to straight down down the rows, as the display shows it |
//! | `HMAP` | `Heights`, then `u16` per cell of its grid, row by row from the north-west: the top of whatever stands there, in `Heights::unit` metres |
//! | `LAMP` | `Heights` (its `unit`: the light a full channel stands for), then `u16` per cell, `r << 11 \| g << 5 \| b`, each channel the square root of its share of `unit`: the light the street lamps leave on the ground by night at the reference's full lamp scale |
//! | `MODL` | `Model` table: what moves (traffic, the player's vehicles), each a range of instanced batches of `BTCH` |

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod sim;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const MAGIC: u32 = u32::from_le_bytes(*b"NYPK");
pub const VERSION: u32 = 14;

pub const fn tag(t: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*t)
}
pub const META: u32 = tag(b"META");
pub const CITY: u32 = tag(b"CITY");
pub const CELL: u32 = tag(b"CELL");
pub const BLCK: u32 = tag(b"BLCK");
pub const REGN: u32 = tag(b"REGN");
pub const BTCH: u32 = tag(b"BTCH");
pub const VWAL: u32 = tag(b"VWAL");
pub const VSOL: u32 = tag(b"VSOL");
pub const VGND: u32 = tag(b"VGND");
pub const IDX0: u32 = tag(b"IDX0");
pub const INST: u32 = tag(b"INST");
pub const SPAN: u32 = tag(b"SPAN");
pub const NCEL: u32 = tag(b"NCEL");
pub const NEAR: u32 = tag(b"NEAR");
pub const TEXS: u32 = tag(b"TEXS");
pub const TDAT: u32 = tag(b"TDAT");
pub const PRST: u32 = tag(b"PRST");
pub const SKYP: u32 = tag(b"SKYP");
pub const MODL: u32 = tag(b"MODL");
pub const HMAP: u32 = tag(b"HMAP");
pub const LAMP: u32 = tag(b"LAMP");

/// Levels of detail: near by cell, mid by block, far by region, and the cell's small things (detail).
pub const LODS: usize = 4;

/// The city's frame: metres, x east, y up, z south (towards the Battery), as the reference has it.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct City {
    /// The north-west corner of cell (0, 0), and a cell's side.
    pub x0: f32,
    pub z0: f32,
    pub cell: f32,
    pub cells_x: u32,
    pub cells_z: u32,
    /// Cells along a block's side, blocks along a region's side.
    pub block_cells: u32,
    pub region_blocks: u32,
    /// Lowest and highest point of anything drawn.
    pub y_min: f32,
    pub y_max: f32,
    /// Where the eye starts: position, then the point it looks at.
    pub view: [f32; 6],
    pub flags: u32,
}

impl City {
    pub fn blocks_x(&self) -> u32 {
        self.cells_x.div_ceil(self.block_cells)
    }
    pub fn blocks_z(&self) -> u32 {
        self.cells_z.div_ceil(self.block_cells)
    }
    pub fn regions_x(&self) -> u32 {
        self.blocks_x().div_ceil(self.region_blocks)
    }
    pub fn regions_z(&self) -> u32 {
        self.blocks_z().div_ceil(self.region_blocks)
    }
}

/// Which program draws a batch.
pub mod kind {
    /// The walls of facade boxes: `WallVertex`, under one facade page.
    pub const WALL: u32 = 0;
    /// Geometry coloured by vertex: `SolidVertex`.
    pub const SOLID: u32 = 1;
    /// Geometry coloured by vertex and seen from both sides (thin parts, cards): `SolidVertex`, drawn without
    /// culling.
    pub const OPEN: u32 = 2;
    /// Surfaces under a picture that repeats over the ground: `GroundVertex`.
    pub const GROUND: u32 = 3;
    /// One shape drawn at every `Instance` of the batch: `SolidVertex` in the shape's own frame.
    pub const SOLID_INST: u32 = 4;
    /// As `SOLID_INST`, seen from both sides.
    pub const OPEN_INST: u32 = 5;
    /// Trees: a crown of cards drawn at every `Instance`, `WallVertex` in the tree's own frame: `uv` in the
    /// leaves' picture (`tex`), `tint` the bark's colour with alpha 255 on a leaf card. Seen from both sides; a
    /// card is cut where the picture's alpha is under a half.
    pub const LEAF_INST: u32 = 6;
    /// Surfaces under a picture of their own (a sign, a billboard): `WallVertex` with `uv` over the picture (`tex`,
    /// whose `metres` is the scale its texels stand over), `tint` alpha 255 where the reference does not light it.
    pub const PICTURE: u32 = 7;
    /// As `PICTURE`, one shape at every `Instance` (a shop's sign along every block).
    pub const PICTURE_INST: u32 = 8;
    /// A model's part under pictures of the model seen along each of its axes (`tex`: sRGB with white paint, alpha
    /// how much is paint; its `night`: what it emits by night over `metres`), looked up by the box its `Model`
    /// holds: `SolidVertex` shapes, one at every `Instance`.
    pub const BOXED_INST: u32 = 9;
    pub const COUNT: usize = 10;

    pub fn instanced(kind: u32) -> bool {
        kind == SOLID_INST || kind == OPEN_INST || kind == LEAF_INST || kind == PICTURE_INST || kind == BOXED_INST
    }
}

/// A range of the `BTCH` table.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Draws {
    pub first: u32,
    pub count: u32,
}

/// A square of the city at the near level of detail. `min`, `max`: bounds of everything its batches draw.
/// `detail`: its small things, drawn only closer than the near level reaches.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Cell {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub near: Draws,
    pub detail: Draws,
}

/// A cell's record of `NEAR`: `offset` and `size` in the section, and the bytes of its parts in the order they are
/// stored, each from a 16-byte boundary: solid vertices, wall vertices, ground vertices, indices, instances. A near
/// or detail batch counts its vertices, indices and instances from the start of its cell's parts; an instanced one
/// draws a shape whose vertices and indices are in the pack's own streams (`VSOL`, `IDX0`), and its instances are
/// the cell's.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct NearCell {
    pub offset: u32,
    pub size: u32,
    pub parts: [u32; 5],
    pub pad: u32,
}

impl NearCell {
    /// Byte offset of each part within the record.
    pub fn starts(&self) -> [usize; 5] {
        let mut at = [0usize; 5];
        let mut o = 0usize;
        for k in 0..5 {
            at[k] = o;
            o = (o + self.parts[k] as usize + 15) & !15;
        }
        at
    }
}

/// `City::block_cells` squared cells at the mid level.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Block {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub mid: Draws,
}

/// `City::region_blocks` squared blocks at the far level.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Region {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub far: Draws,
}

/// One draw. Its vertices are `u16` positions normalized over the box `origin` .. `origin + span`, which is also
/// its bounds; `tex` is the picture it reads (`NO_TEX` for none). An instanced batch (`kind::instanced`) draws
/// its vertices, in the shape's own frame, once per record `inst_first .. inst_first + inst_count` of `INST`.
/// Instanced batches of one shape share its vertices and indices.
///
/// A mid batch has its triangles (an instanced one, its instances) ordered by the cell of its block they belong
/// to, and a far batch by the block of its region: `spans` is where its record in `SPAN` starts (`NO_SPANS` for a
/// near or detail batch). A frame draws, of a block whose cells are partly near, the runs of the other cells.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Batch {
    pub kind: u32,
    pub tex: u32,
    pub vtx_first: u32,
    pub vtx_count: u32,
    pub idx_first: u32,
    pub idx_count: u32,
    pub origin: [f32; 3],
    pub span: [f32; 3],
    pub inst_first: u32,
    pub inst_count: u32,
    pub spans: u32,
    pub pad: u32,
}

pub const NO_SPANS: u32 = u32::MAX;

/// Where one instance stands: the rows of a 3 × 4 matrix from the shape's frame to the world, and what its colour
/// is to the shape's (linear, 128 = as the shape has it), 52 bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Instance {
    pub rows: [[f32; 4]; 3],
    pub tint: [u8; 4],
}

pub const NO_TEX: u32 = u32::MAX;

/// A wall of a facade box, 20 bytes. `uv`: the facade page's coordinates over `UV_RANGE` (the page repeats);
/// `tint`: what the box's own colour is to the page's (sRGB, 128 = as drawn); `ao`: how much of the sky the
/// point sees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct WallVertex {
    pub pos: [u16; 3],
    pub normal: [i8; 3],
    pub ao: u8,
    pub uv: [i16; 2],
    pub tint: [u8; 4],
    pub pad: [u8; 2],
}

/// `WallVertex::uv` dequantizes as `uv / 32767 × UV_RANGE`.
pub const UV_RANGE: f32 = 64.0;

/// Geometry coloured by vertex, 20 bytes. `color`: albedo, sRGB; `glow`: what it emits by night, rgb over
/// `glow[3] / 255 × GLOW_RANGE`; `gloss`: what it reflects, roughness and metalness (0..255 for 0..1). A surface
/// the reference does not light (`MeshBasicNodeMaterial`) has black albedo and its colour as glow, with
/// `GLOW_ALWAYS` in `color[3]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct SolidVertex {
    pub pos: [u16; 3],
    pub normal: [i8; 3],
    pub ao: u8,
    pub color: [u8; 4],
    pub glow: [u8; 4],
    pub gloss: [u8; 2],
}

pub const GLOW_RANGE: f32 = 16.0;
/// `SolidVertex::color[3]`: the glow shows by day as well.
pub const GLOW_ALWAYS: u8 = 255;
/// `SolidVertex::color[3]` of an instanced shape's vertex that takes the instance's tint whole.
pub const TINTED: u8 = 254;

/// A surface under a picture that repeats over the ground, 16 bytes. Its texture coordinates are its own x and z
/// over the picture's size in metres (`TexEntry::metres`). `tint`: as `WallVertex::tint`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct GroundVertex {
    pub pos: [u16; 3],
    pub normal: [i8; 3],
    pub ao: u8,
    pub tint: [u8; 4],
    pub pad: [u8; 2],
}

/// A picture of `TDAT`.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct TexEntry {
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub mips: u32,
    pub offset: u32,
    pub size: u32,
    /// The picture's own emission (a facade page by night) is the next entry.
    pub night: u32,
    /// Metres the picture covers along x and z, for a picture that repeats over the ground; 0 otherwise.
    pub metres: f32,
    /// What its surface reflects: roughness and metalness (0..255 for 0..1), as the material that drew it has them.
    pub gloss: [u8; 4],
}

pub mod tex_format {
    /// BC1 blocks in row order.
    pub const BC1: u32 = 1;
    /// BC3 blocks in row order.
    pub const BC3: u32 = 3;

    /// Bytes of one level.
    pub fn level_bytes(format: u32, w: u32, h: u32) -> usize {
        let blocks = (w.max(4) as usize / 4) * (h.max(4) as usize / 4);
        blocks * if format == BC3 { 16 } else { 8 }
    }
}

/// One of the reference's hours (`TIME_PRESETS` in its lighting.js). Colours are linear.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Preset {
    /// Towards the sun (or the moon), and its colour × intensity.
    pub sun_dir: [f32; 3],
    pub sun: [f32; 3],
    /// Hemisphere light: from the sky, from the ground, × intensity.
    pub hemi_sky: [f32; 3],
    pub hemi_ground: [f32; 3],
    pub sky_top: [f32; 3],
    pub sky_horizon: [f32; 3],
    pub fog: [f32; 3],
    pub fog_density: f32,
    /// How bright the rooms are (`uInteriorGlow`), and how far night has come.
    pub interior_glow: f32,
    pub night: f32,
    pub exposure: f32,
    pub pad: f32,
    /// The light on a white diffuse face, linear, as the export read it back: from the sky and the ground (a
    /// hemisphere, environment included) and from the sun, with the camera's white balance.
    pub light_sky: [f32; 3],
    pub light_ground: [f32; 3],
    pub light_sun: [f32; 3],
    /// The haze's colour just over the horizon towards the sun, across from it and away from it, as the display
    /// shows it.
    pub haze: [[f32; 3]; 3],
    /// The height over which the haze thins by e.
    pub fog_height: f32,
    /// The grade and ACES of the hour, to what the display shows: y = x(ax + b) / (x(cx + d) + 1).
    pub tone: [f32; 4],
    /// The sky a surface reflects (the reference's environment), linear, with its intensity and the white balance:
    /// just over the horizon, and a third of the way up.
    pub env_horizon: [f32; 3],
    pub env_up: [f32; 3],
    /// The reference's bloom for the hour: its strength, and its threshold as the display shows it.
    pub bloom: [f32; 2],
    /// How wet the roadway is (`uRoadWetness`).
    pub road_wetness: f32,
    pub pad2: f32,
}

/// The head of `SKYP`.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct SkyHead {
    pub w: u32,
    pub h: u32,
    pub count: u32,
    pub pad: u32,
}

/// The grid of heights the shadows are swept from: its north-west corner, cell size and extent, and metres per unit
/// of its values.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Heights {
    pub x0: f32,
    pub z0: f32,
    pub step: f32,
    pub unit: f32,
    pub w: u32,
    pub h: u32,
    pub pad: [u32; 2],
}

/// A model the game places each frame: its parts are the instanced batches `first .. first + count` of `BTCH`, whose
/// vertices are in the model's own frame (x across, y up, z ahead) and in no place's lists. A vertex's `color[3]`
/// below 255 is how much of it is paint (254 is all of it): an instance's tint is its paint.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Model {
    pub name: [u8; 24],
    pub first: u32,
    pub count: u32,
    /// The box its pictures were drawn over (`kind::BOXED_INST`), in its own frame; all zero without them.
    pub box_min: [f32; 3],
    pub box_size: [f32; 3],
}

impl Model {
    pub fn name(&self) -> &str {
        let n = self.name.iter().position(|&b| b == 0).unwrap_or(self.name.len());
        core::str::from_utf8(&self.name[..n]).unwrap_or("")
    }
}

pub fn bytes_of<T: Copy>(v: &T) -> &[u8] {
    unsafe { core::slice::from_raw_parts(v as *const T as *const u8, core::mem::size_of::<T>()) }
}

pub fn slice_bytes<T: Copy>(v: &[T]) -> &[u8] {
    unsafe { core::slice::from_raw_parts(v.as_ptr() as *const u8, core::mem::size_of_val(v)) }
}

pub fn read<T: Copy>(b: &[u8], at: usize) -> Option<T> {
    let n = core::mem::size_of::<T>();
    let s = b.get(at..at + n)?;
    Some(unsafe { core::ptr::read_unaligned(s.as_ptr() as *const T) })
}

/// Every record of a table section.
pub fn table<T: Copy>(b: &[u8]) -> Vec<T> {
    let n = core::mem::size_of::<T>();
    (0..b.len() / n).filter_map(|i| read::<T>(b, i * n)).collect()
}

/// The section table of a pack, from its first bytes.
pub struct Index {
    pub sections: Vec<(u32, usize, usize)>,
}

impl Index {
    /// Bytes of the header and the table of a pack with `count` sections.
    pub fn head_bytes(count: usize) -> usize {
        16 + count * 16
    }

    /// The section count, from the first 16 bytes.
    pub fn count(head: &[u8]) -> Result<usize, String> {
        let word = |i: usize| read::<u32>(head, i).ok_or_else(|| "pack is truncated".to_string());
        if word(0)? != MAGIC {
            return Err("not a pack".into());
        }
        if word(4)? != VERSION {
            return Err(format!("pack version {} (this build reads {VERSION})", word(4)?));
        }
        Ok(word(8)? as usize)
    }

    pub fn parse(bytes: &[u8]) -> Result<Index, String> {
        let n = Self::count(bytes)?;
        let mut sections = Vec::with_capacity(n);
        for i in 0..n {
            let at = 16 + i * 16;
            let word = |i: usize| read::<u32>(bytes, i).ok_or_else(|| "pack is truncated".to_string());
            sections.push((word(at)?, word(at + 4)? as usize, word(at + 8)? as usize));
        }
        Ok(Index { sections })
    }

    /// Offset and size of a section.
    pub fn range(&self, t: u32) -> Result<(usize, usize), String> {
        self.sections.iter().find(|s| s.0 == t).map(|s| (s.1, s.2)).ok_or_else(|| format!("pack has no {} section", String::from_utf8_lossy(&t.to_le_bytes())))
    }
}

/// Builds a pack in memory.
#[derive(Default)]
pub struct Writer {
    sections: Vec<(u32, Vec<u8>)>,
}

impl Writer {
    pub fn add(&mut self, t: u32, bytes: Vec<u8>) {
        self.sections.push((t, bytes));
    }

    pub fn finish(self) -> Vec<u8> {
        let pad = |n: usize| (n + 15) & !15;
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC.to_le_bytes());
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&(self.sections.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        let mut at = pad(16 + self.sections.len() * 16);
        for (t, bytes) in &self.sections {
            out.extend_from_slice(&t.to_le_bytes());
            out.extend_from_slice(&(at as u32).to_le_bytes());
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            at = pad(at + bytes.len());
        }
        for (_, bytes) in &self.sections {
            out.resize(pad(out.len()), 0);
            out.extend_from_slice(bytes);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_have_their_sizes() {
        assert_eq!(core::mem::size_of::<WallVertex>(), 20);
        assert_eq!(core::mem::size_of::<SolidVertex>(), 20);
        assert_eq!(core::mem::size_of::<GroundVertex>(), 16);
        assert_eq!(core::mem::size_of::<Batch>(), 64);
        assert_eq!(core::mem::size_of::<Instance>(), 52);
        assert_eq!(core::mem::size_of::<TexEntry>(), 32);
        assert_eq!(core::mem::size_of::<Cell>(), 40);
        assert_eq!(core::mem::size_of::<NearCell>(), 32);
        assert_eq!(core::mem::size_of::<Model>(), 32);
        assert_eq!(core::mem::size_of::<Heights>(), 32);
    }

    #[test]
    fn a_pack_reads_back() {
        let mut w = Writer::default();
        w.add(META, b"{}".to_vec());
        w.add(CITY, bytes_of(&City { cell: 128.0, ..Default::default() }).to_vec());
        let bytes = w.finish();
        let index = Index::parse(&bytes).unwrap();
        let (at, n) = index.range(CITY).unwrap();
        let c: City = read(&bytes[at..at + n], 0).unwrap();
        assert_eq!(c.cell, 128.0);
    }
}
