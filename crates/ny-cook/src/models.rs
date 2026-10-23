//! What moves, as the export drew it (`web/pocket/models.js`): each model's parts become shapes in the pack's own
//! streams, and `MODL` says which batches make a model. A car's parts drawn under its pictures (`boxes`: the car
//! seen along each axis) are `kind::BOXED_INST`, the pictures and their box the car's.

use crate::bins::{Bins, Vx, Written};
use crate::geom;
use crate::ir::Array;
use ny_pack::{self as pack, Model};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Deserialize)]
struct Part {
    verts: usize,
    position: Array,
    normal: Array,
    color: Array,
    glow: Array,
    index: Array,
    unlit: bool,
    double: bool,
    #[serde(default)]
    posed: bool,
    #[serde(default = "matte")]
    roughness: f32,
    #[serde(default)]
    metalness: f32,
    /// The pictures it is drawn under (a key of `boxes`).
    #[serde(default)]
    boxed: Option<String>,
}

fn matte() -> f32 {
    0.8
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BoxJson {
    file: String,
    paint: String,
    glow: String,
    gloss: String,
    glow_scale: f32,
    min: [f32; 3],
    size: [f32; 3],
}

#[derive(Deserialize)]
struct ModelJson {
    name: String,
    parts: Vec<Part>,
}

#[derive(Deserialize)]
struct Models {
    models: Vec<ModelJson>,
    #[serde(default)]
    boxes: HashMap<String, BoxJson>,
}

/// A model's part: its shape, whether it is seen from both sides, and the pictures it is drawn under.
pub struct Shaped {
    pub shape: u32,
    pub double: bool,
    pub boxed: Option<u32>,
}

/// The models: name, parts, and the box their pictures were drawn over.
pub type Parts = Vec<(String, Vec<Shaped>, Option<([f32; 3], [f32; 3])>)>;

/// A car's pictures: its look (sRGB, alpha its paint), and what it emits by night over its scale (alpha how much of
/// the sky it mirrors) as the look's `night`.
fn box_pictures(ir: &Path, b: &BoxJson, textures: &mut crate::tex::Textures) -> Result<u32, String> {
    let look = image::open(ir.join(&b.file)).map_err(|e| format!("{}: {e}", b.file))?.to_rgba8();
    let paint = image::open(ir.join(&b.paint)).map_err(|e| format!("{}: {e}", b.paint))?.to_rgba8();
    let (w, h) = (look.width() as usize, look.height() as usize);
    let mut px = look.into_raw();
    for (i, p) in paint.pixels().enumerate() {
        px[i * 4 + 3] = p.0[0];
    }
    let t = textures.add_rgba_alpha(px, w, h, 0.0);
    let lit = image::open(ir.join(&b.glow)).map_err(|e| format!("{}: {e}", b.glow))?.to_rgba8();
    let gloss = image::open(ir.join(&b.gloss)).map_err(|e| format!("{}: {e}", b.gloss))?.to_rgba8();
    let mut px = lit.into_raw();
    for (i, p) in gloss.pixels().enumerate() {
        px[i * 4 + 3] = p.0[0];
    }
    let glow = textures.add_rgba_alpha(px, w, h, b.glow_scale);
    textures.entries[t as usize].night = glow;
    Ok(t)
}

pub fn shapes(ir: &Path, bins: &mut Bins, textures: &mut crate::tex::Textures) -> Result<Parts, String> {
    let text = std::fs::read_to_string(ir.join("models.json")).map_err(|e| format!("models.json: {e}"))?;
    let models: Models = serde_json::from_str(&text).map_err(|e| format!("models.json: {e}"))?;
    let bytes = std::fs::read(ir.join("models.bin")).map_err(|e| format!("models.bin: {e}"))?;
    let f32s = |a: &Array| -> Vec<f32> { (0..a.len).map(|i| f32::from_le_bytes(bytes[a.off + i * 4..a.off + i * 4 + 4].try_into().unwrap())).collect() };
    let u8s = |a: &Array| -> &[u8] { &bytes[a.off..a.off + a.len] };
    let u32s = |a: &Array| -> Vec<u32> { (0..a.len).map(|i| u32::from_le_bytes(bytes[a.off + i * 4..a.off + i * 4 + 4].try_into().unwrap())).collect() };
    let mut pictures: HashMap<String, u32> = HashMap::new();
    for (name, b) in &models.boxes {
        pictures.insert(name.clone(), box_pictures(ir, b, textures)?);
    }
    let mut out = Vec::new();
    for m in &models.models {
        let mut parts = Vec::new();
        let mut boxed = None;
        for p in &m.parts {
            let (pos, mut nrm, col, glow) = (f32s(&p.position), f32s(&p.normal), u8s(&p.color), u8s(&p.glow));
            let idx = u32s(&p.index);
            // A posed person's normals: its faces' normals, summed at each vertex.
            if p.posed {
                nrm = vec![0.0; p.verts * 3];
                for f in idx.chunks(3) {
                    let v = |k: usize| [pos[f[k] as usize * 3], pos[f[k] as usize * 3 + 1], pos[f[k] as usize * 3 + 2]];
                    let n = geom::cross(geom::sub(v(1), v(0)), geom::sub(v(2), v(0)));
                    for &i in f {
                        for k in 0..3 {
                            nrm[i as usize * 3 + k] += n[k];
                        }
                    }
                }
            }
            let mut verts: Vec<Vx> = (0..p.verts)
                .map(|i| {
                    let n = geom::normalize([nrm[i * 3], nrm[i * 3 + 1], nrm[i * 3 + 2]]);
                    let c = [col[i * 4], col[i * 4 + 1], col[i * 4 + 2], col[i * 4 + 3]];
                    let g = [glow[i * 4], glow[i * 4 + 1], glow[i * 4 + 2], glow[i * 4 + 3]];
                    // A part the reference does not light is all glow, and none of it paint.
                    let (a, b) = if p.unlit {
                        let lin = [geom::linear(c[0]), geom::linear(c[1]), geom::linear(c[2])];
                        let top = lin[0].max(lin[1]).max(lin[2]).max(1e-4);
                        let mm = (top / pack::GLOW_RANGE * 255.0).ceil().clamp(1.0, 255.0);
                        let s = mm / 255.0 * pack::GLOW_RANGE;
                        ([0, 0, 0, pack::GLOW_ALWAYS], [(lin[0] / s * 255.0) as u8, (lin[1] / s * 255.0) as u8, (lin[2] / s * 255.0) as u8, mm as u8])
                    } else {
                        ([c[0], c[1], c[2], c[3].min(pack::TINTED)], g)
                    };
                    // (a car under its pictures reflects as its glass does; each texel says how much of that it mirrors)
                    let gloss = if p.boxed.is_some() { crate::bins::gloss(0.15, 0.34) } else { crate::bins::gloss(p.roughness, p.metalness) };
                    Vx { p: [pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]], n: n.map(geom::snorm8), ao: 255, a, b, uv: [0.0; 2], gloss }
                })
                .collect();
            // A light probed at the vertices of a large part (a tail lamp in a car's body) would spread over every
            // triangle that touches it: glow stays on triangles that glow at all three corners; the others take
            // copies of their glowing corners without it.
            let mut idx = idx;
            let glows = |v: &Vx| v.b[3] > 0 && v.a[3] != pack::GLOW_ALWAYS;
            let mut dark: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
            for f in idx.chunks_mut(3) {
                let lit = f.iter().filter(|&&i| glows(&verts[i as usize])).count();
                if lit == 0 || lit == 3 {
                    continue;
                }
                for i in f.iter_mut() {
                    if glows(&verts[*i as usize]) {
                        *i = *dark.entry(*i).or_insert_with(|| {
                            verts.push(Vx { b: [0; 4], ..verts[*i as usize] });
                            verts.len() as u32 - 1
                        });
                    }
                }
            }
            let tex = p.boxed.as_ref().and_then(|b| pictures.get(b)).copied();
            if let Some(b) = p.boxed.as_ref().and_then(|b| models.boxes.get(b)) {
                boxed = Some((b.min, b.size));
            }
            parts.push(Shaped { shape: bins.shape(&verts, &idx), double: p.double, boxed: tex });
        }
        out.push((m.name.clone(), parts, boxed));
    }
    Ok(out)
}

/// Appends the models' batches to the table, past every place's, and returns `MODL`.
pub fn table(parts: &Parts, w: &mut Written) -> Vec<Model> {
    let mut out = Vec::new();
    for (name, list, boxed) in parts {
        let first = w.batches.len() as u32;
        for p in list {
            let mut b = w.shape_batches[p.shape as usize];
            b.kind = if p.boxed.is_some() { pack::kind::BOXED_INST } else if p.double { pack::kind::OPEN_INST } else { pack::kind::SOLID_INST };
            b.tex = p.boxed.unwrap_or(pack::NO_TEX);
            w.batches.push(b);
        }
        let mut n = [0u8; 24];
        for (i, c) in name.bytes().take(23).enumerate() {
            n[i] = c;
        }
        let (box_min, box_size) = boxed.unwrap_or(([0.0; 3], [0.0; 3]));
        out.push(Model { name: n, first, count: list.len() as u32, box_min, box_size });
    }
    out
}
