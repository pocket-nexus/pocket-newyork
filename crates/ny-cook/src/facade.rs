//! The walls of facade boxes, under the pages the export baked (`web/pocket/bake.js`).
//!
//! A box-local facade material lays each wall of a box out as the reference's `facadeCell` does: bays of the wall's
//! width over whole storeys of the world's height, solid corner piers on masonry above the ground storey, a solid
//! band where a storey is cut short at the box's top or bottom. A wall becomes rows of quads (a partial storey at
//! the bottom, the ground storey, the whole storeys above, a partial storey at the top) and, on masonry, columns
//! (pier, bays, pier), each under its page; the page repeats along the wall and up it.

use crate::bins::Vx;
use crate::geom::{self, M4};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Deserialize)]
struct BakePass {
    file: String,
    scale: f32,
}

#[derive(Deserialize)]
struct BakePage {
    factory: String,
    args: serde_json::Value,
    page: String,
    base: u32,
    rows: u32,
    #[serde(rename = "windowWidth")]
    window_width: f32,
    #[serde(rename = "floorHeight")]
    floor_height: f32,
    pier: f32,
    day: BakePass,
    night: Option<BakePass>,
}

#[derive(Deserialize)]
struct BakeJson {
    facades: Vec<BakePage>,
}

/// A page: its picture and what part of a wall it shows.
#[derive(Clone, Copy, Debug)]
pub struct Page {
    pub tex: u32,
    /// The first storey it shows, and how many.
    pub base: u32,
    pub rows: u32,
}

/// A facade material's layout and its pages.
pub struct Style {
    pub window: f32,
    pub storey: f32,
    pub pier: f32,
    pub pages: HashMap<String, Page>,
}

pub struct Facades {
    /// By `key`: material uuids change with every export, what made a material does not.
    pub styles: HashMap<String, Style>,
}

/// What identifies a facade material across exports: its factory and the arguments it was called with.
pub fn key(factory: &str, args: &serde_json::Value) -> String {
    format!("{factory}:{args}")
}

/// The reference's wall kinds (`WALL_KINDS`).
mod kind {
    pub const SHOP: u32 = 0;
    pub const ENTRY: u32 = 1;
    pub const REAR: u32 = 2;
    pub const LOT_LINE: u32 = 3;
    pub const BLANK_STONE: u32 = 4;
}

pub fn load(ir: &Path, textures: &mut crate::tex::Textures) -> Result<Facades, String> {
    let text = std::fs::read_to_string(ir.join("bake.json")).map_err(|e| format!("bake.json: {e}"))?;
    let bake: BakeJson = serde_json::from_str(&text).map_err(|e| format!("bake.json: {e}"))?;
    let mut styles: HashMap<String, Style> = HashMap::new();
    for p in bake.facades {
        let day = textures.add_png(&ir.join(&p.day.file), 0.0)?;
        if let Some(n) = &p.night {
            let night = textures.add_png(&ir.join(&n.file), 0.0)?;
            textures.entries[day as usize].night = night;
            // A night page's texels are its emission over `scale`: the scale rides in its `metres`.
            textures.entries[night as usize].metres = n.scale;
        }
        let _ = p.day.scale;
        // What the page reflects: a curtain wall's glass and metal, a masonry front's stone among its windows; the
        // windowless pages, their stone or precast panels.
        let blank = matches!(p.page.as_str(), "lot" | "stone" | "pier_front" | "pier_rear");
        let (rough, metal) = match (p.factory.as_str(), blank) {
            ("createCurtainWallMaterial", false) => (0.12, 0.72),
            ("createCurtainWallMaterial", true) => (0.88, 0.0),
            (_, false) => (0.55, 0.06),
            (_, true) => (0.82, 0.06),
        };
        let g = crate::bins::gloss(rough, metal);
        textures.entries[day as usize].gloss = [g[0], g[1], 0, 0];
        let s = styles.entry(key(&p.factory, &p.args)).or_insert_with(|| Style { window: p.window_width, storey: p.floor_height, pier: p.pier, pages: HashMap::new() });
        s.pages.insert(p.page.clone(), Page { tex: day, base: p.base, rows: p.rows });
    }
    Ok(Facades { styles })
}

/// One quad of a wall: its picture and its four corners, counter-clockwise seen from outside.
pub struct Quad {
    pub tex: u32,
    pub v: [Vx; 4],
}

/// The quads of the four walls of one box. `m`: the instance's world matrix (a unit box turned about y, scaled
/// and moved); `bx`: the `box` attribute (width, height, depth, seed with the tone in its fraction); `faces`: the
/// wall kinds of the −x, +x, −z and +z walls.
pub fn box_walls(style: &Style, m: &M4, bx: [f32; 4], faces: [f32; 4], out: &mut Vec<Quad>) {
    walls(style, m, bx, faces, false, out)
}

/// The walls of one box as one quad each, under the page of its upper storeys: the mid and far level.
pub fn box_walls_simple(style: &Style, m: &M4, bx: [f32; 4], faces: [f32; 4], out: &mut Vec<Quad>) {
    walls(style, m, bx, faces, true, out)
}

fn walls(style: &Style, m: &M4, bx: [f32; 4], faces: [f32; 4], simple: bool, out: &mut Vec<Quad>) {
    let (w, h, d) = (bx[0], bx[1], bx[2]);
    // World height of the box's bottom and top.
    let yb = geom::point(m, [0.0, -0.5, 0.0])[1];
    let yt = geom::point(m, [0.0, 0.5, 0.0])[1];
    let f = style.storey;
    // (local normal, the wall's kind, its width along the axis it runs on)
    let walls: [([f32; 3], f32, f32); 4] = [([-1.0, 0.0, 0.0], faces[0], d), ([1.0, 0.0, 0.0], faces[1], d), ([0.0, 0.0, -1.0], faces[2], w), ([0.0, 0.0, 1.0], faces[3], w)];
    for (n, kind_f, width) in walls {
        let kind = kind_f.round() as u32;
        let along_z = n[0] != 0.0;
        // Whole-bay offset per wall: floor(box.w + dot(n, (17, 0, 37))).
        let wall_seed = (bx[3] + n[0] * 17.0 + n[2] * 37.0).floor();
        let windowed = matches!(kind, kind::SHOP | kind::ENTRY | kind::REAR);
        let pier = if style.pier > 0.0 && windowed { style.pier.min(width * 0.2) } else { 0.0 };
        let inner = width - 2.0 * pier;
        let bays = if style.pier > 0.0 { (inner / style.window + 0.5).floor().max(1.0) } else { (width / style.window + 0.5).floor().max(1.0) };
        let bay_w = if style.pier > 0.0 { inner / bays } else { width / bays };
        // Bays from the wall's corner (along local +x or +z), as gridU; the page holds 8, whose phase repeats.
        let grid_u = |s: f32| -> f32 { (s - if style.pier > 0.0 { style.pier.min(width * 0.2) } else { 0.0 }) / bay_w + wall_seed };
        let phase = (grid_u(0.0) / 8.0).floor() * 8.0;
        // A point of the wall: s metres along it from its corner, at world height y.
        let corner = |s: f32, y: f32| -> [f32; 3] {
            let a = s / width - 0.5;
            let ly = (y - yb) / (yt - yb) - 0.5;
            let local = if along_z { [n[0] * 0.5, ly, a] } else { [a, ly, n[2] * 0.5] };
            geom::point(m, local)
        };
        let world_n = geom::normalize(geom::sub(geom::point(m, n), geom::point(m, [0.0; 3])));
        let nn = world_n.map(geom::snorm8);
        let page = |name: &str| style.pages.get(name).copied();
        let (upper, ground, solid, pier_page) = match kind {
            kind::SHOP => (page("up_front"), page("ground_shop"), page("stone"), page("pier_front")),
            kind::ENTRY => (page("up_front"), page("ground_entry"), page("stone"), page("pier_front")),
            kind::REAR => (page("up_rear"), page("ground_rear"), page("lot"), page("pier_rear")),
            kind::LOT_LINE => (page("lot"), page("lot"), page("lot"), None),
            _ => (page("stone"), page("stone"), page("stone"), None),
        };
        let Some(upper) = upper else { continue };
        let solid = solid.unwrap_or(upper);
        let mut quad = |pg: Page, s0: f32, s1: f32, y0: f32, y1: f32, uv: &dyn Fn(f32, f32) -> [f32; 2]| {
            if s1 - s0 < 0.01 || y1 - y0 < 0.01 {
                return;
            }
            let mk = |s: f32, y: f32| Vx { p: corner(s, y), n: nn, ao: 255, a: [128, 128, 128, 255], b: [0; 4], uv: uv(s, y), gloss: crate::bins::MATTE };
            let mut v = [mk(s0, y0), mk(s1, y0), mk(s1, y1), mk(s0, y1)];
            // Counter-clockwise seen from outside.
            let c = geom::cross(geom::sub(v[1].p, v[0].p), geom::sub(v[2].p, v[0].p));
            if geom::dot(c, world_n) < 0.0 {
                v.swap(1, 3);
            }
            out.push(Quad { tex: pg.tex, v });
        };
        // Texture coordinates: u in pages of 8 bays, v down the page from its top storey (row 0 at the top).
        let rows_v = |pg: Page| move |y: f32| (pg.base + pg.rows) as f32 - y / f;
        let bays_uv = |pg: Page| move |s: f32, y: f32| [(grid_u(s) - phase) / 8.0, rows_v(pg)(y) / pg.rows as f32];
        // A plain page (lot line, stone) runs in metres over its 8 bays.
        let plain_uv = |pg: Page| move |s: f32, y: f32| [s / (8.0 * style.window), rows_v(pg)(y) / pg.rows as f32];

        if simple {
            let uv = |s: f32, y: f32| [(grid_u(s) - phase) / 8.0, rows_v(upper)(y) / upper.rows as f32];
            if kind == kind::LOT_LINE || kind == kind::BLANK_STONE {
                quad(upper, 0.0, width, yb, yt, &plain_uv(upper));
            } else {
                quad(upper, 0.0, width, yb, yt, &uv);
            }
            continue;
        }
        // Storeys: whole from k0 to k1 (inclusive), as the reference counts them.
        let k0 = ((yb - 0.05) / f).ceil().max(0.0);
        let k1 = ((yt + 0.05) / f).floor() - 1.0;
        if k1 < k0 {
            // Not one whole storey: a solid wall.
            quad(solid, 0.0, width, yb, yt, &plain_uv(solid));
            continue;
        }
        let (yw0, yw1) = (k0 * f, (k1 + 1.0) * f);
        quad(solid, 0.0, width, yb, yw0, &plain_uv(solid));
        quad(solid, 0.0, width, yw1, yt, &plain_uv(solid));
        let mut up_from = yw0;
        if k0 == 0.0 && kind != kind::LOT_LINE && kind != kind::BLANK_STONE {
            if let Some(g) = ground {
                // The ground storey runs across the whole wall: on a shop front the shop windows reach the corners.
                quad(g, 0.0, width, 0.0, f, &bays_uv(g));
                up_from = f;
            }
        }
        if up_from < yw1 {
            if kind == kind::LOT_LINE || kind == kind::BLANK_STONE {
                quad(upper, 0.0, width, up_from, yw1, &plain_uv(upper));
            } else if pier > 0.0 {
                let pp = pier_page.unwrap_or(solid);
                quad(pp, 0.0, pier, up_from, yw1, &|s, y| [s / pier, rows_v(pp)(y) / pp.rows as f32]);
                quad(upper, pier, width - pier, up_from, yw1, &bays_uv(upper));
                quad(pp, width - pier, width, up_from, yw1, &|s, y| [(width - s) / pier, rows_v(pp)(y) / pp.rows as f32]);
            } else {
                quad(upper, 0.0, width, up_from, yw1, &bays_uv(upper));
            }
        }
    }
}
