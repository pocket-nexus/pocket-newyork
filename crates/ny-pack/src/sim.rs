//! What moves reads from the pack (`ny-sim`): the reference's colliders, the ground and shore its controllers
//! stand on, the names its location label reads, its road network and signal timing, its cameras and the four
//! entities as they start. The compiler writes these from `sim.json` (`crates/ny-cook/src/sim.rs`).
//!
//! | Tag    | Contents |
//! | ------ | -------- |
//! | `SIMW` | `SimWorld` |
//! | `COLL` | `Collider` table, in the reference's order (collisions are resolved in that order) |
//! | `SHOR` | `[f32; 2]` per shore step: the island's west and east shore x (`SimWorld::shore_*`) |
//! | `SBLK` | `SimBlock` table: the curb islands, for what stands on a sidewalk |
//! | `SPTS` | `[f64; 2]` points of the blocks' outlines and the roads' centrelines, as the reference has them (which side of 45° a segment runs, and so a label, can rest on the last bit) |
//! | `ROAD` | `Road` table: named centrelines |
//! | `STRT` | `Street` table: the numbered streets |
//! | `LMRK` | `Mark` table: the landmarks the location label names |
//! | `TELE` | `Mark` table: the teleports (`name` is the reference's key) |
//! | `NAME` | every name, UTF-8, one per line; a record's `name` is its line |
//! | `NODE` | `TrafficNode` table |
//! | `EDGE` | `TrafficEdge` table |
//! | `LANE` | `f32` lane offsets to the right of an edge's centreline, left to right |
//! | `MOVE` | `TrafficMove` table |
//! | `MOUT` | `i8` per lane of a move's edge: the lane of the next edge it leads to, or -1 |
//! | `CAMS` | `CameraPreset` table, in the reference's order |
//! | `ENTS` | `EntityStart` × 4: person, car, cyclist, helicopter |
//! | `WALK` | `WalkEdge` per edge of `EDGE`: where pedestrians may walk beside it, the crosswalks its vehicles cross |
//! | `WNOD` | `WalkNode` per node of `NODE`: its crosswalks and where pedestrians may cross them |
//! | `HWYL` | `HighwayLane` table: the highway's lanes |
//! | `HWYP` | `[f32; 3]` points of the highway lanes' centre lines |
//!
//! `WALK` to `HWYP` are optional: a pack without them has no pedestrians and no highway traffic.

use crate::tag;

pub const SIMW: u32 = tag(b"SIMW");
pub const COLL: u32 = tag(b"COLL");
pub const SHOR: u32 = tag(b"SHOR");
pub const SBLK: u32 = tag(b"SBLK");
pub const SPTS: u32 = tag(b"SPTS");
pub const ROAD: u32 = tag(b"ROAD");
pub const STRT: u32 = tag(b"STRT");
pub const LMRK: u32 = tag(b"LMRK");
pub const TELE: u32 = tag(b"TELE");
pub const NAME: u32 = tag(b"NAME");
pub const NODE: u32 = tag(b"NODE");
pub const EDGE: u32 = tag(b"EDGE");
pub const LANE: u32 = tag(b"LANE");
pub const MOVE: u32 = tag(b"MOVE");
pub const MOUT: u32 = tag(b"MOUT");
pub const CAMS: u32 = tag(b"CAMS");
pub const ENTS: u32 = tag(b"ENTS");
pub const WALK: u32 = tag(b"WALK");
pub const WNOD: u32 = tag(b"WNOD");
pub const HWYL: u32 = tag(b"HWYL");
pub const HWYP: u32 = tag(b"HWYP");

/// Every section the simulation reads.
pub const SECTIONS: [u32; 17] = [SIMW, COLL, SHOR, SBLK, SPTS, ROAD, STRT, LMRK, TELE, NAME, NODE, EDGE, LANE, MOVE, MOUT, CAMS, ENTS];

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct SimWorld {
    /// The detailed island's bounds (`ISLAND_BOUNDS`): min x, max x, min z, max z.
    pub island: [f32; 4],
    pub sidewalk_height: f32,
    pub water_level: f32,
    /// Central Park's bounds (`CENTRAL_PARK_BOUNDS`), as `island`.
    pub central_park: [f32; 4],
    /// Bryant Park's Great Lawn, as `island`, and its top; `lawn_top` is NaN where there is none.
    pub lawn: [f32; 4],
    pub lawn_top: f32,
    /// The detailed island ends here to the north (`CUT_Z`).
    pub cut_z: f32,
    /// `SHOR`: the first step's z, metres between steps, steps.
    pub shore_z0: f32,
    pub shore_step: f32,
    pub shore_n: u32,
    /// `SIGNAL_TIMING`: cycle, avenue amber, avenue red, street amber, walk; seconds.
    pub signal: [f32; 5],
    /// Names the location label uses besides the roads': Central Park, the Hudson River and East River
    /// waterfronts, New York Harbor (`NAME` lines).
    pub specials: [u32; 4],
}

/// A box the player cannot enter below its top: axis-aligned bounds, and an oriented box inside them when `obb`
/// is 1 (centre, half sizes along its own axes, the cosine and sine of its turn).
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Collider {
    pub min_x: f32,
    pub max_x: f32,
    pub min_z: f32,
    pub max_z: f32,
    pub max_y: f32,
    pub obb: u32,
    pub cx: f32,
    pub cz: f32,
    pub hx: f32,
    pub hz: f32,
    pub cs: f32,
    pub sn: f32,
}

pub mod block {
    /// The outline is its bounds.
    pub const RECT: u32 = 1;
    /// A median mall or a sliver: a sidewalk whatever its parkland.
    pub const MEDIAN: u32 = 2;
}

/// A curb island: bounds, its outline (`count` points of `SPTS` from `first`), `block` flags, its share of parkland.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct SimBlock {
    pub min_x: f32,
    pub max_x: f32,
    pub min_z: f32,
    pub max_z: f32,
    pub first: u32,
    pub count: u32,
    pub flags: u32,
    pub park: f32,
}

/// A named centreline: `count` points of `SPTS` from `first`.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Road {
    pub first: u32,
    pub count: u32,
    pub name: u32,
    pub pad: u32,
}

/// A numbered street: its z and its short name ("72nd St").
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Street {
    pub z: f32,
    pub number: u32,
    pub name: u32,
    pub pad: u32,
}

/// A named spot: a landmark, or a teleport and the heading it arrives at.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Mark {
    pub x: f32,
    pub z: f32,
    pub heading: f32,
    pub name: u32,
}

/// An intersection of the road network: centre, half sizes of its box, whether it has signals, right-of-way groups.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct TrafficNode {
    pub x: f32,
    pub z: f32,
    pub hx: f32,
    pub hz: f32,
    pub signal: u32,
    pub groups: u32,
}

pub mod edge {
    /// An avenue (`axis === 'ave'`): buses keep to these.
    pub const AVE: u32 = 1;
}

/// A directed road between two intersections: where its lanes start (after the crosswalk), its direction and right
/// normal, its length; its lanes (`LANE`), its moves at its end (`MOVE`), its right-of-way group there (0: the
/// avenue phase) and the oncoming edge (or -1).
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct TrafficEdge {
    pub from: u32,
    pub to: u32,
    pub sx: f32,
    pub sz: f32,
    pub dx: f32,
    pub dz: f32,
    pub rx: f32,
    pub rz: f32,
    pub len: f32,
    pub lane_first: u32,
    pub lane_count: u32,
    pub move_first: u32,
    pub move_count: u32,
    pub flags: u32,
    pub group: u32,
    pub opposite: i32,
}

pub mod turn {
    pub const LEFT: u32 = 0;
    pub const STRAIGHT: u32 = 1;
    pub const RIGHT: u32 = 2;
}

/// A way on from the end of an edge: the edge it leads to, `turn`, and where its lane table starts in `MOUT`.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct TrafficMove {
    pub to: u32,
    pub turn: u32,
    pub out_first: u32,
    pub pad: u32,
}

/// Steps of the offsets a sidewalk track may take beside an edge (`WalkEdge::walk`): `half + 1.4 + k × OFF_STEP`.
pub const OFF_STEPS: u32 = 53;
pub const OFF_STEP: f32 = 0.05;
/// Steps of the lines a crosswalk track may take off its centre line (`WalkNode::lines`): `0.5 + k × LINE_STEP`.
pub const LINE_STEPS: u32 = 12;
pub const LINE_STEP: f32 = 0.05;

/// Where the reference's pedestrians may walk beside an edge, as its own tests found it (traffic.js
/// `sidewalkTrack`): bit k of `walk[0]` (to its right) or `walk[1]` (to its left) for a track `half + 1.4 + k ×
/// OFF_STEP` off its lanes' centre line; and what its vehicles need of the crosswalks: the road's half width, the
/// sides of `to` and of `from` it arrives and leaves over, the lateral axis a lane meets them along, whether it is a
/// centreline street.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct WalkEdge {
    pub half: f32,
    pub clx: f32,
    pub clz: f32,
    pub in_side: u8,
    pub out_side: u8,
    pub general: u8,
    pub pad: u8,
    pub walk: [u64; 2],
}

/// A signalized intersection's crosswalks (traffic.js `crosswalkTrack`), sides 0 east and 1 west of the avenue
/// (across the street), 2 south and 3 north of the street (across the avenue): the edge whose road each spans (-1:
/// none), that road's centre (x for an avenue, z for a street) and half width, and which lines make a track, bit k
/// for `0.5 + k × LINE_STEP` on the side's own sign and bit `LINE_STEPS + k` on the other.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct WalkNode {
    pub road: [i32; 4],
    pub across: [f32; 4],
    pub half: [f32; 4],
    pub lines: [u32; 4],
}

/// A lane of the highway (city-grid.js `highwayLanes`): its centre line, `count` points of `HWYP` from `first`, and
/// the speed its traffic keeps.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct HighwayLane {
    pub first: u32,
    pub count: u32,
    pub speed: f32,
    pub pad: u32,
}

/// One of the reference's cameras: vertical field of view (degrees), whether it is scenic (fixed or scripted) or
/// follows the player, its id (`NAME` line).
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct CameraPreset {
    pub fov: f32,
    pub scenic: u32,
    pub id: u32,
    pub pad: u32,
}

/// An entity as it starts: position, heading, its chase camera's distance and height, the height it is looked at.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct EntityStart {
    pub pos: [f32; 3],
    pub heading: f32,
    pub chase_distance: f32,
    pub chase_height: f32,
    pub target_y: f32,
    pub pad: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_have_their_sizes() {
        assert_eq!(core::mem::size_of::<SimWorld>(), 112);
        assert_eq!(core::mem::size_of::<Collider>(), 48);
        assert_eq!(core::mem::size_of::<SimBlock>(), 32);
        assert_eq!(core::mem::size_of::<TrafficEdge>(), 64);
        assert_eq!(core::mem::size_of::<EntityStart>(), 32);
        assert_eq!(core::mem::size_of::<WalkEdge>(), 32);
        assert_eq!(core::mem::size_of::<WalkNode>(), 64);
        assert_eq!(core::mem::size_of::<HighwayLane>(), 16);
    }
}
