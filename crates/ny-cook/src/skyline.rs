//! The far level of the facade boxes as prisms, as Pocket Tokyo draws its simple levels (`tokyo-cook/src/prisms.rs`,
//! ported): every box's footprint goes into one grid of heights, the tallest box over a cell's middle holding it. In
//! a block of the far level, cells next to each other at about one height form a terrace, whoever's roof they are;
//! a terrace's outline, straightened, becomes its walls in the colours of the boxes behind them, and a flat top in
//! the colour of its roofs. A wall stands only where the outline looks out over lower ground.

use crate::bins::{Bins, Vx, FAR};
use crate::geom;
use ny_pack as pack;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize, Clone, Copy, Debug)]
pub struct Profile {
    /// Metres a cell of the grid is across.
    pub cell: f32,
    /// Metres of height one terrace may span.
    pub cap: f32,
    /// A terrace smaller than this (m²) joins a neighbour, or is left out.
    pub min_area: f32,
    /// How far (m) a straightened outline may leave the cells' own.
    pub eps: f32,
    /// Roofs lower than this are left out.
    pub min_height: f32,
    /// Passes of the smoothing of the terraces' outlines.
    pub smooth: u32,
}

/// A facade box as the far level sees it.
pub struct Box {
    /// Its footprint's corners, x and z, in turn around it.
    pub corners: [[f32; 2]; 4],
    pub bottom: f32,
    pub top: f32,
    /// The colour of its walls (the mean of its pages) and of its roof, sRGB; what its walls emit by night
    /// (`glow_bytes`).
    pub wall: [u8; 3],
    pub roof: [u8; 3],
    pub glow: [u8; 4],
    /// What its walls and its roof reflect (`bins::gloss`).
    pub wall_gloss: [u8; 2],
    pub roof_gloss: [u8; 2],
}

const NONE: u32 = u32::MAX;

/// Roof heights over the blocks the boxes stand in, and whose roof each cell is.
struct Grid {
    x0: f32,
    z0: f32,
    step: f32,
    w: usize,
    h: usize,
    height: Vec<f32>,
    /// Box index + 1; 0 where none is.
    owner: Vec<u32>,
}

impl Grid {
    fn new(boxes: &[Box], x0: f32, z0: f32, w: usize, h: usize, step: f32) -> Grid {
        let mut g = Grid { x0, z0, step, w, h, height: vec![f32::NEG_INFINITY; w * h], owner: vec![0; w * h] };
        for (k, b) in boxes.iter().enumerate() {
            let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
            for c in b.corners {
                for a in 0..2 {
                    lo[a] = lo[a].min(c[a]);
                    hi[a] = hi[a].max(c[a]);
                }
            }
            let range = |lo: f32, hi: f32, o: f32, n: usize| ((((lo - o) / step - 0.5).ceil().max(0.0)) as isize, ((((hi - o) / step - 0.5).floor()) as isize).min(n as isize - 1));
            let (i0, i1) = range(lo[0], hi[0], x0, w);
            let (j0, j1) = range(lo[1], hi[1], z0, h);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let q = [x0 + (i as f32 + 0.5) * step, z0 + (j as f32 + 0.5) * step];
                    let o = j as usize * w + i as usize;
                    if inside_quad(&b.corners, q) && b.top > g.height[o] {
                        g.height[o] = b.top;
                        g.owner[o] = k as u32 + 1;
                    }
                }
            }
        }
        g
    }
}

/// Whether `q` is inside the convex footprint `c` (either way round).
fn inside_quad(c: &[[f32; 2]; 4], q: [f32; 2]) -> bool {
    let mut sign = 0.0f32;
    for k in 0..4 {
        let (a, b) = (c[k], c[(k + 1) % 4]);
        let cross = (b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0]);
        if cross.abs() < 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

#[derive(Clone, Copy)]
struct Edge {
    /// Start and end vertex, as `j * (n + 1) + i` on the tile's lattice of cell corners.
    from: u32,
    to: u32,
    /// What lies across: `NONE`, a terrace, or (`open`) the same terrace in the next tile.
    across: u32,
    open: bool,
}

/// Ramer-Douglas-Peucker over `pts[a..=b]`: marks the points to keep.
fn rdp(pts: &[[f32; 2]], a: usize, b: usize, eps: f32, keep: &mut [bool]) {
    if b <= a + 1 {
        return;
    }
    let (p, q) = (pts[a], pts[b]);
    let (dx, dz) = (q[0] - p[0], q[1] - p[1]);
    let len = (dx * dx + dz * dz).sqrt().max(1e-6);
    let (mut worst, mut at) = (0.0f32, a);
    for (k, c) in pts.iter().enumerate().take(b).skip(a + 1) {
        let d = ((c[0] - p[0]) * dz - (c[1] - p[1]) * dx).abs() / len;
        if d > worst {
            worst = d;
            at = k;
        }
    }
    if worst > eps {
        keep[at] = true;
        rdp(pts, a, at, eps, keep);
        rdp(pts, at, b, eps, keep);
    }
}

fn inside(poly: &[[f32; 2]], x: f32, z: f32) -> bool {
    let mut c = false;
    let mut k = poly.len() - 1;
    for m in 0..poly.len() {
        let (a, b) = (poly[m], poly[k]);
        if (a[1] > z) != (b[1] > z) && x < (b[0] - a[0]) * (z - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
        k = m;
    }
    c
}

/// Puts the far level of `boxes` in `bins`; the triangles of tops and of walls it made.
pub fn build(boxes: &[Box], rules: Profile, bins: &mut Bins) -> [usize; 2] {
    if boxes.is_empty() {
        return [0; 2];
    }
    let city = bins.city;
    let tile = city.cell * city.block_cells as f32;
    let n = (tile / rules.cell).round() as usize;
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for b in boxes {
        for c in b.corners {
            for k in 0..2 {
                lo[k] = lo[k].min(c[k]);
                hi[k] = hi[k].max(c[k]);
            }
        }
    }
    // The grid starts on a block's edge and holds whole blocks.
    let gx0 = city.x0 + ((lo[0] - city.x0) / tile).floor() * tile;
    let gz0 = city.z0 + ((lo[1] - city.z0) / tile).floor() * tile;
    let tiles = [((hi[0] - gx0) / tile).ceil() as usize, ((hi[1] - gz0) / tile).ceil() as usize];
    let grid = Grid::new(boxes, gx0, gz0, tiles[0] * n, tiles[1] * n, rules.cell);
    let mut tris = [0; 2];
    for tz in 0..tiles[1] {
        for tx in 0..tiles[0] {
            let (x0, z0) = (gx0 + tx as f32 * tile, gz0 + tz as f32 * tile);
            let (verts, idx, t) = prisms(&grid, x0, z0, n, &rules, boxes);
            if !idx.is_empty() {
                let (place, sub) = bins.place(FAR, [x0 + tile * 0.5, 0.0, z0 + tile * 0.5]);
                bins.bin(FAR, place, pack::kind::SOLID, pack::NO_TEX).add(&verts, &idx, sub);
            }
            tris[0] += t[0];
            tris[1] += t[1];
        }
    }
    tris
}

/// The prisms of the tile whose corner is `(x0, z0)`, `n` cells a side: vertices, indices, and the triangles of
/// tops and of walls.
fn prisms(grid: &Grid, x0: f32, z0: f32, n: usize, rules: &Profile, boxes: &[Box]) -> (Vec<Vx>, Vec<u32>, [usize; 2]) {
    let (gi, gj) = (((x0 - grid.x0) / grid.step).round() as isize, ((z0 - grid.z0) / grid.step).round() as isize);
    // The tile's cells with one more all round: what the neighbours hold along the border.
    let w = n + 2;
    let at = |i: usize, j: usize| j * w + i;
    let cell = |i: usize, j: usize| -> Option<usize> {
        let (ci, cj) = (gi + i as isize - 1, gj + j as isize - 1);
        (ci >= 0 && cj >= 0 && (ci as usize) < grid.w && (cj as usize) < grid.h).then(|| cj as usize * grid.w + ci as usize)
    };
    let mut height = vec![f32::NEG_INFINITY; w * w];
    let mut any = false;
    for j in 0..w {
        for i in 0..w {
            if let Some(c) = cell(i, j) {
                let h = grid.height[c];
                if h > f32::NEG_INFINITY && grid.owner[c] != 0 && h - boxes[grid.owner[c] as usize - 1].bottom >= rules.min_height {
                    height[at(i, j)] = h;
                    any |= (1..=n).contains(&i) && (1..=n).contains(&j);
                }
            }
        }
    }
    let (mut verts, mut idx) = (Vec::new(), Vec::new());
    if !any {
        return (verts, idx, [0; 2]);
    }

    // ---- terraces: from the highest cell not yet taken, everything joined to it within the cap
    let mut label = vec![NONE; w * w];
    let mut order: Vec<usize> = (0..w * w).filter(|&c| height[c] > f32::NEG_INFINITY).collect();
    order.sort_by(|a, b| height[*b].partial_cmp(&height[*a]).unwrap().then(a.cmp(b)));
    let mut regions: Vec<(f32, usize)> = Vec::new(); // (height, cells)
    let mut stack = Vec::new();
    let mut members: Vec<f32> = Vec::new();
    for &seed in &order {
        if label[seed] != NONE {
            continue;
        }
        let r = regions.len() as u32;
        let floor = height[seed] - rules.cap;
        label[seed] = r;
        stack.push(seed);
        members.clear();
        while let Some(c) = stack.pop() {
            members.push(height[c]);
            let (i, j) = (c % w, c / w);
            for (di, dj) in [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)] {
                let (ni, nj) = (i as isize + di, j as isize + dj);
                if ni < 0 || nj < 0 || ni >= w as isize || nj >= w as isize {
                    continue;
                }
                let m = at(ni as usize, nj as usize);
                if label[m] == NONE && height[m] >= floor {
                    label[m] = r;
                    stack.push(m);
                }
            }
        }
        // The terrace stands at the height most of it has: a water tank on a roof does not lift the roof.
        members.sort_by(|a, b| a.partial_cmp(b).unwrap());
        regions.push((members[members.len() * 6 / 10], members.len()));
    }
    // ---- a cell goes over to the terrace most of the eight around it belong to: the outlines lose their frays
    for _ in 0..rules.smooth {
        let before = label.clone();
        let mut votes: Vec<(u32, u8)> = Vec::with_capacity(8);
        for j in 1..w - 1 {
            for i in 1..w - 1 {
                let own = before[at(i, j)];
                votes.clear();
                for (di, dj) in [(-1isize, -1isize), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let l = before[at((i as isize + di) as usize, (j as isize + dj) as usize)];
                    match votes.iter_mut().find(|v| v.0 == l) {
                        Some(v) => v.1 += 1,
                        None => votes.push((l, 1)),
                    }
                }
                let (most, k) = votes.iter().copied().max_by_key(|v| (v.1, std::cmp::Reverse(v.0))).unwrap();
                if most != own && k >= 5 {
                    label[at(i, j)] = most;
                    if own != NONE {
                        regions[own as usize].1 -= 1;
                    }
                    if most != NONE {
                        regions[most as usize].1 += 1;
                    }
                }
            }
        }
    }
    // ---- a terrace too small to draw joins the neighbour it shares most of its edge with, or goes
    let cell_area = grid.step * grid.step;
    let mut small: Vec<u32> = (0..regions.len() as u32).filter(|&r| (regions[r as usize].1 as f32) * cell_area < rules.min_area).collect();
    small.sort_by_key(|&r| regions[r as usize].1);
    let mut cells_of: Vec<Vec<usize>> = vec![Vec::new(); regions.len()];
    for (c, &l) in label.iter().enumerate() {
        if l != NONE {
            cells_of[l as usize].push(c);
        }
    }
    for r in small {
        let mut shared: HashMap<u32, usize> = HashMap::new();
        for &c in &cells_of[r as usize] {
            if label[c] != r {
                continue;
            }
            let (i, j) = (c % w, c / w);
            for (di, dj) in [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)] {
                let (ni, nj) = (i as isize + di, j as isize + dj);
                if ni < 0 || nj < 0 || ni >= w as isize || nj >= w as isize {
                    continue;
                }
                let l = label[at(ni as usize, nj as usize)];
                if l != NONE && l != r {
                    *shared.entry(l).or_default() += 1;
                }
            }
        }
        let to = shared.into_iter().max_by_key(|&(l, k)| (k, std::cmp::Reverse(l))).map(|(l, _)| l).unwrap_or(NONE);
        let moved = std::mem::take(&mut cells_of[r as usize]);
        for &c in &moved {
            if label[c] == r {
                label[c] = to;
            }
        }
        if to != NONE {
            regions[to as usize].1 += regions[r as usize].1;
            cells_of[to as usize].extend(moved);
        }
        regions[r as usize].1 = 0;
    }

    // ---- each terrace's colour: the mean of its roofs (in linear light), and what its roofs reflect (the first's)
    let mut roof_sum = vec![[0.0f32; 4]; regions.len()];
    let mut roof_gloss = vec![crate::bins::MATTE; regions.len()];
    for j in 1..=n {
        for i in 1..=n {
            let r = label[at(i, j)];
            if r == NONE {
                continue;
            }
            if let Some(c) = cell(i, j) {
                if grid.owner[c] != 0 {
                    let b = &boxes[grid.owner[c] as usize - 1];
                    if roof_sum[r as usize][3] == 0.0 {
                        roof_gloss[r as usize] = b.roof_gloss;
                    }
                    for k in 0..3 {
                        roof_sum[r as usize][k] += geom::linear(b.roof[k]);
                    }
                    roof_sum[r as usize][3] += 1.0;
                }
            }
        }
    }

    // ---- outlines: the edges between a terrace's cells in the tile and anything else, walked with the terrace
    // on the right hand
    let lat = n + 1;
    let mut edges: Vec<Vec<Edge>> = vec![Vec::new(); regions.len()];
    for j in 1..=n {
        for i in 1..=n {
            let r = label[at(i, j)];
            if r == NONE {
                continue;
            }
            let (vi, vj) = (i - 1, j - 1);
            let v = |a: usize, b: usize| (b * lat + a) as u32;
            // (neighbour cell, from, to)
            let sides = [((i, j - 1), v(vi, vj), v(vi + 1, vj)), ((i + 1, j), v(vi + 1, vj), v(vi + 1, vj + 1)), ((i, j + 1), v(vi + 1, vj + 1), v(vi, vj + 1)), ((i - 1, j), v(vi, vj + 1), v(vi, vj))];
            for ((ni, nj), from, to) in sides {
                let across = label[at(ni, nj)];
                let out_of_tile = ni == 0 || nj == 0 || ni == n + 1 || nj == n + 1;
                if across != r || out_of_tile {
                    edges[r as usize].push(Edge { from, to, across, open: across == r });
                }
            }
        }
    }

    let mut counts = [0usize; 2];
    let point = |v: u32| -> [f32; 2] { [x0 + (v as usize % lat) as f32 * grid.step, z0 + (v as usize / lat) as f32 * grid.step] };
    let emit = |p: [[f32; 3]; 3], nrm: [f32; 3], color: [u8; 3], glow: [u8; 4], gloss: [u8; 2], verts: &mut Vec<Vx>, idx: &mut Vec<u32>| {
        let base = verts.len() as u32;
        let face = geom::cross(geom::sub(p[1], p[0]), geom::sub(p[2], p[0]));
        let order = if face[0] * nrm[0] + face[1] * nrm[1] + face[2] * nrm[2] < 0.0 { [0, 2, 1] } else { [0, 1, 2] };
        for k in order {
            verts.push(Vx { p: p[k], n: nrm.map(geom::snorm8), ao: 255, a: [color[0], color[1], color[2], 0], b: glow, uv: [0.0; 2], gloss });
        }
        idx.extend([base, base + 1, base + 2]);
    };
    for (r, list) in edges.iter().enumerate() {
        if list.is_empty() {
            continue;
        }
        let top = regions[r].0;
        let s = roof_sum[r];
        let roof = if s[3] > 0.0 { [0, 1, 2].map(|k| geom::srgb(s[k] / s[3])) } else { [128; 3] };
        let roof_gloss = roof_gloss[r];
        // Chain the edges into loops. Where two of the terrace's cells touch by a corner, the walk takes the
        // sharper turn, so each loop stays a simple outline.
        let mut next: HashMap<u32, Vec<usize>> = HashMap::new();
        for (k, e) in list.iter().enumerate() {
            next.entry(e.from).or_default().push(k);
        }
        let mut used = vec![false; list.len()];
        let mut loops: Vec<Vec<usize>> = Vec::new();
        for start in 0..list.len() {
            if used[start] {
                continue;
            }
            let mut ring = Vec::new();
            let mut k = start;
            loop {
                used[k] = true;
                ring.push(k);
                let e = list[k];
                let (a, b) = (point(e.from), point(e.to));
                let d = [b[0] - a[0], b[1] - a[1]];
                let pick = next.get(&e.to).and_then(|c| {
                    c.iter().copied().filter(|&m| !used[m]).max_by(|&m1, &m2| {
                        let turn = |m: usize| {
                            let (p, q) = (point(list[m].from), point(list[m].to));
                            d[0] * (q[1] - p[1]) - d[1] * (q[0] - p[0])
                        };
                        turn(m1).partial_cmp(&turn(m2)).unwrap()
                    })
                });
                match pick {
                    Some(m) => k = m,
                    None => break,
                }
            }
            if list[*ring.last().unwrap()].to == list[ring[0]].from && ring.len() >= 4 {
                loops.push(ring);
            }
        }

        // Straighten each loop; a straightened edge remembers which cell edges it stands for.
        struct Outline {
            pts: Vec<[f32; 2]>,
            /// For each point: the first cell edge of the outline edge that starts there, and one past its last.
            spans: Vec<(usize, usize)>,
            ring: Vec<usize>,
            area: f32,
        }
        let mut outlines: Vec<Outline> = Vec::new();
        for ring in loops {
            let pts: Vec<[f32; 2]> = ring.iter().map(|&k| point(list[k].from)).collect();
            let m = pts.len();
            // Two anchors far apart, then each half between them.
            let a = (0..m).min_by(|&p, &q| (pts[p][0] + pts[p][1]).partial_cmp(&(pts[q][0] + pts[q][1])).unwrap()).unwrap();
            let rot: Vec<[f32; 2]> = (0..=m).map(|k| pts[(a + k) % m]).collect();
            let b = (0..m)
                .max_by(|&p, &q| {
                    let d = |k: usize| (rot[k][0] - rot[0][0]).powi(2) + (rot[k][1] - rot[0][1]).powi(2);
                    d(p).partial_cmp(&d(q)).unwrap()
                })
                .unwrap();
            let mut keep = vec![false; m + 1];
            keep[0] = true;
            keep[b] = true;
            keep[m] = true;
            rdp(&rot, 0, b, rules.eps, &mut keep);
            rdp(&rot, b, m, rules.eps, &mut keep);
            let kept: Vec<usize> = (0..m).filter(|&k| keep[k]).collect();
            if kept.len() < 3 {
                continue;
            }
            let simple: Vec<[f32; 2]> = kept.iter().map(|&k| rot[k]).collect();
            let spans: Vec<(usize, usize)> = kept.iter().enumerate().map(|(q, &k)| (k, if q + 1 < kept.len() { kept[q + 1] } else { m })).collect();
            let area: f32 = (0..simple.len())
                .map(|k| {
                    let (p, q) = (simple[k], simple[(k + 1) % simple.len()]);
                    p[0] * q[1] - q[0] * p[1]
                })
                .sum::<f32>()
                * 0.5;
            if area.abs() < 0.5 * cell_area {
                continue;
            }
            let ring: Vec<usize> = (0..m).map(|k| ring[(a + k) % m]).collect();
            outlines.push(Outline { pts: simple, spans, ring, area });
        }

        // ---- the top: each outer outline with the holes that lie in it
        let outers: Vec<usize> = (0..outlines.len()).filter(|&k| outlines[k].area > 0.0).collect();
        for &o in &outers {
            let mut flat: Vec<f64> = outlines[o].pts.iter().flat_map(|p| [p[0] as f64, p[1] as f64]).collect();
            let mut holes = Vec::new();
            for hole in outlines.iter() {
                if hole.area < 0.0 && inside(&outlines[o].pts, hole.pts[0][0], hole.pts[0][1]) && (outers.len() == 1 || !outers.iter().any(|&p| p != o && outlines[p].area < outlines[o].area && inside(&outlines[p].pts, hole.pts[0][0], hole.pts[0][1]))) {
                    holes.push(flat.len() / 2);
                    flat.extend(hole.pts.iter().flat_map(|p| [p[0] as f64, p[1] as f64]));
                }
            }
            if let Ok(t) = earcutr::earcut(&flat, &holes, 2) {
                for t in t.chunks_exact(3) {
                    let p = [0, 1, 2].map(|k| [flat[t[k] * 2] as f32, top, flat[t[k] * 2 + 1] as f32]);
                    if geom::length(geom::cross(geom::sub(p[1], p[0]), geom::sub(p[2], p[0]))) > 2e-4 {
                        emit(p, [0.0, 1.0, 0.0], roof, [0; 4], roof_gloss, &mut verts, &mut idx);
                        counts[0] += 1;
                    }
                }
            }
        }

        // ---- the walls: an outline edge needs one where anything it stands for looks out over lower ground
        for outline in &outlines {
            let m = outline.pts.len();
            for k in 0..m {
                let (a, b) = (outline.pts[k], outline.pts[(k + 1) % m]);
                let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
                let len = (dx * dx + dz * dz).sqrt();
                if len < 0.05 {
                    continue;
                }
                // (with the terrace on the right of a → b, the wall looks to the left)
                let nrm = [dz / len, 0.0, -dx / len];
                // Whose wall it is: the box under the terrace's cells just inside this edge.
                let mut owner = 0;
                for t in [0.5f32, 0.25, 0.75, 0.05, 0.95] {
                    let p = [a[0] + dx * t - nrm[0] * grid.step * 0.5, a[1] + dz * t - nrm[2] * grid.step * 0.5];
                    let (ci, cj) = (((p[0] - grid.x0) / grid.step) as isize, ((p[1] - grid.z0) / grid.step) as isize);
                    if ci >= 0 && cj >= 0 && (ci as usize) < grid.w && (cj as usize) < grid.h {
                        owner = grid.owner[cj as usize * grid.w + ci as usize];
                        if owner != 0 {
                            break;
                        }
                    }
                }
                let ground = if owner != 0 { boxes[owner as usize - 1].bottom } else { 0.0 };
                let mut bottom = f32::INFINITY;
                let (s0, s1) = outline.spans[k];
                for q in s0..s1 {
                    let e = list[outline.ring[q]];
                    if e.open {
                        continue;
                    }
                    if e.across == NONE {
                        bottom = bottom.min(ground - 1.0);
                    } else if regions[e.across as usize].0 < top - 0.3 {
                        bottom = bottom.min(regions[e.across as usize].0 - 0.3);
                    }
                }
                if bottom == f32::INFINITY {
                    continue;
                }
                let (color, glow, gloss) = if owner != 0 { (boxes[owner as usize - 1].wall, boxes[owner as usize - 1].glow, boxes[owner as usize - 1].wall_gloss) } else { ([128; 3], [0; 4], crate::bins::MATTE) };
                let (pa0, pb0, pb1, pa1) = ([a[0], bottom, a[1]], [b[0], bottom, b[1]], [b[0], top, b[1]], [a[0], top, a[1]]);
                emit([pa0, pb0, pb1], nrm, color, glow, gloss, &mut verts, &mut idx);
                emit([pa0, pb1, pa1], nrm, color, glow, gloss, &mut verts, &mut idx);
                counts[1] += 2;
            }
        }
    }
    (verts, idx, counts)
}
