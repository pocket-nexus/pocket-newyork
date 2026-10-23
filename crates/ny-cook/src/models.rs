//! What moves, as the export drew it (`web/pocket/models.js`): each model's parts become shapes in the pack's own
//! streams, and `MODL` says which batches make a model.

use crate::bins::{Bins, Vx, Written};
use crate::geom;
use crate::ir::Array;
use ny_pack::{self as pack, Model};
use serde::Deserialize;
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
}

#[derive(Deserialize)]
struct ModelJson {
    name: String,
    parts: Vec<Part>,
}

#[derive(Deserialize)]
struct Models {
    models: Vec<ModelJson>,
}

/// The models' parts: name, then per part its shape and whether it is seen from both sides.
pub type Parts = Vec<(String, Vec<(u32, bool)>)>;

pub fn shapes(ir: &Path, bins: &mut Bins) -> Result<Parts, String> {
    let text = std::fs::read_to_string(ir.join("models.json")).map_err(|e| format!("models.json: {e}"))?;
    let models: Models = serde_json::from_str(&text).map_err(|e| format!("models.json: {e}"))?;
    let bytes = std::fs::read(ir.join("models.bin")).map_err(|e| format!("models.bin: {e}"))?;
    let f32s = |a: &Array| -> Vec<f32> { (0..a.len).map(|i| f32::from_le_bytes(bytes[a.off + i * 4..a.off + i * 4 + 4].try_into().unwrap())).collect() };
    let u8s = |a: &Array| -> &[u8] { &bytes[a.off..a.off + a.len] };
    let u32s = |a: &Array| -> Vec<u32> { (0..a.len).map(|i| u32::from_le_bytes(bytes[a.off + i * 4..a.off + i * 4 + 4].try_into().unwrap())).collect() };
    let mut out = Vec::new();
    for m in &models.models {
        let mut parts = Vec::new();
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
            let verts: Vec<Vx> = (0..p.verts)
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
                    Vx { p: [pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]], n: n.map(geom::snorm8), ao: 255, a, b, uv: [0.0; 2] }
                })
                .collect();
            parts.push((bins.shape(&verts, &idx), p.double));
        }
        out.push((m.name.clone(), parts));
    }
    Ok(out)
}

/// Appends the models' batches to the table, past every place's, and returns `MODL`.
pub fn table(parts: &Parts, w: &mut Written) -> Vec<Model> {
    let mut out = Vec::new();
    for (name, list) in parts {
        let first = w.batches.len() as u32;
        for &(shape, double) in list {
            let mut b = w.shape_batches[shape as usize];
            b.kind = if double { pack::kind::OPEN_INST } else { pack::kind::SOLID_INST };
            w.batches.push(b);
        }
        let mut n = [0u8; 24];
        for (i, c) in name.bytes().take(23).enumerate() {
            n[i] = c;
        }
        out.push(Model { name: n, first, count: list.len() as u32 });
    }
    out
}
