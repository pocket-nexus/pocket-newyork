//! Where a point is, as the reference names it (`getLocationLabel` and `nearestRoads` in city-config.js): a
//! landmark within 80 m with its corner, Central Park with the street it is near, a waterfront, an avenue and a
//! street, a road, or the harbour. `Location` holds `NAME` lines; `label` writes the reference's text.

use crate::game::Location;
use crate::world::World;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use libm::{fabs, fabsf, hypotf};

/// A road near a point: its name, how far, and whether its nearest segment runs east-west.
#[derive(Clone, Copy, Debug)]
pub struct Near {
    pub name: u32,
    pub dist: f64,
    pub horizontal: bool,
}

impl World {
    /// `nearestRoads`: one entry per road name within `radius`, nearest first.
    pub fn nearest_roads(&self, x: f32, z: f32, radius: f32) -> Vec<Near> {
        let mut best: Vec<Near> = Vec::new();
        let g = &self.road_grid;
        let (xd, zd, rd) = (x as f64, z as f64, radius as f64);
        for ix in g.cell_of_f64(xd - rd)..=g.cell_of_f64(xd + rd) {
            for iz in g.cell_of_f64(zd - rd)..=g.cell_of_f64(zd + rd) {
                for si in g.at(ix, iz) {
                    // In doubles, as the reference: two segments meeting at a vertex are as near as each other, and
                    // which one counts (and so whether the road runs east-west there) is settled by their rounding.
                    let s = &self.segments[si as usize];
                    let (ax, az, px, pz) = (s.a[0], s.a[1], xd, zd);
                    let (dx, dz) = (s.b[0] - ax, s.b[1] - az);
                    let l2 = dx * dx + dz * dz;
                    let l2 = if l2 == 0.0 { 1e-9 } else { l2 };
                    let t = (((px - ax) * dx + (pz - az) * dz) / l2).min(1.0).max(0.0);
                    let d = libm::hypot(ax + t * dx - px, az + t * dz - pz);
                    if d > rd {
                        continue;
                    }
                    let name = self.roads[s.road as usize];
                    match best.iter_mut().find(|n| n.name == name) {
                        Some(n) => {
                            if d < n.dist {
                                n.dist = d;
                                n.horizontal = fabs(dx) > fabs(dz);
                            }
                        }
                        None => best.push(Near { name, dist: d, horizontal: fabs(dx) > fabs(dz) }),
                    }
                }
            }
        }
        // (a stable sort, as Array.prototype.sort is)
        best.sort_by(|a, b| a.dist.partial_cmp(&b.dist).unwrap_or(core::cmp::Ordering::Equal));
        best
    }

    /// `getLocationLabel`, as name lines.
    pub fn location(&self, x: f32, z: f32) -> Location {
        let id = |n: u32| Some(n as u16);
        for lm in &self.landmarks {
            if hypotf(x - lm.x, z - lm.z) < 80.0 {
                let near = self.nearest_roads(x, z, 120.0);
                let ave = near.iter().find(|r| !r.horizontal);
                let st = near.iter().find(|r| r.horizontal);
                return match (ave, st) {
                    (Some(a), Some(s)) => Location { place: id(lm.name), avenue: id(a.name), street: id(s.name), near: false },
                    _ => Location { place: id(lm.name), ..Location::default() },
                };
            }
        }
        let cp = self.info.central_park;
        if x > cp[0] && x < cp[1] && z > cp[2] && z < cp[3] && !self.streets.is_empty() {
            let mut st = &self.streets[0];
            for s in &self.streets {
                if s.number != 0 && fabsf(s.z - z) < fabsf(st.z - z) {
                    st = s;
                }
            }
            return Location { place: id(self.info.specials[0]), avenue: None, street: id(st.name), near: true };
        }
        let (west, east) = self.shore_at(z);
        let near = self.nearest_roads(x, z, 160.0);
        let ave = near.iter().find(|r| !r.horizontal);
        let st = near.iter().find(|r| r.horizontal);
        if z > self.info.cut_z && (x < west + 30.0 || x > east - 30.0) {
            let side = if x < (west + east) / 2.0 { self.info.specials[1] } else { self.info.specials[2] };
            return Location { place: id(side), avenue: None, street: st.and_then(|s| id(s.name)), near: st.is_some() };
        }
        if let (Some(a), Some(s)) = (ave, st) {
            return Location { place: None, avenue: id(a.name), street: id(s.name), near: false };
        }
        if let Some(r) = near.first() {
            return if r.horizontal { Location { street: id(r.name), ..Location::default() } } else { Location { avenue: id(r.name), ..Location::default() } };
        }
        Location { place: id(self.info.specials[3]), ..Location::default() }
    }

    /// The reference's text for a location ("Empire State Building · 5th Ave & W 34th St").
    pub fn label(&self, l: &Location) -> String {
        let name = |n: Option<u16>| n.map(|n| self.name(n)).unwrap_or("");
        let corner = match (l.avenue, l.street) {
            (Some(_), Some(_)) => format!("{} & {}", name(l.avenue), name(l.street)),
            (Some(_), None) => String::from(name(l.avenue)),
            (None, Some(_)) => String::from(name(l.street)),
            (None, None) => String::new(),
        };
        match l.place {
            None => corner,
            Some(_) if corner.is_empty() => String::from(name(l.place)),
            Some(_) if l.near => format!("{} · near {}", name(l.place), corner),
            Some(_) => format!("{} · {}", name(l.place), corner),
        }
    }
}
