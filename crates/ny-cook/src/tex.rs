//! Pictures for the pack: PNGs of the bake, compressed to BC1 with their mipmaps, into `TEXS` and `TDAT`.

use ny_pack::{tex_format, TexEntry, NO_TEX};
use std::path::Path;

#[derive(Default)]
pub struct Textures {
    pub entries: Vec<TexEntry>,
    pub data: Vec<u8>,
    means: Vec<[f32; 3]>,
}

fn mean(px: &[u8]) -> [f32; 3] {
    let mut s = [0.0f64; 3];
    for t in px.chunks(4) {
        for c in 0..3 {
            s[c] += crate::geom::linear(t[c]) as f64;
        }
    }
    let n = (px.len() / 4).max(1) as f64;
    [(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32]
}

/// Halves an RGBA picture by averaging 2 × 2 texels (in linear light).
fn halve(px: &[u8], w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let lin: Vec<f32> = (0..256).map(|i| crate::geom::linear(i as u8)).collect();
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..4 {
                let mut s = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = ((x * 2 + dx).min(w - 1), (y * 2 + dy).min(h - 1));
                    let v = px[(sy * w + sx) * 4 + c];
                    s += if c < 3 { lin[v as usize] } else { v as f32 / 255.0 };
                }
                s *= 0.25;
                out[(y * nw + x) * 4 + c] = if c < 3 { crate::geom::srgb(s) } else { (s * 255.0).round() as u8 };
            }
        }
    }
    (out, nw, nh)
}

impl Textures {
    /// Adds an RGBA picture with every level down to 4 × 4, as BC1; returns its index.
    pub fn add_rgba(&mut self, px: Vec<u8>, w: usize, h: usize, metres: f32) -> u32 {
        self.add_as(px, w, h, metres, tex_format::BC1)
    }

    /// As `add_rgba`, keeping its alpha: BC3.
    pub fn add_rgba_alpha(&mut self, px: Vec<u8>, w: usize, h: usize, metres: f32) -> u32 {
        self.add_as(px, w, h, metres, tex_format::BC3)
    }

    fn add_as(&mut self, px: Vec<u8>, w: usize, h: usize, metres: f32, format: u32) -> u32 {
        if self.means.len() < self.entries.len() + 1 {
            self.means.push(mean(&px));
        }
        let offset = self.data.len();
        let (mut cur, mut cw, mut ch) = (px, w, h);
        let mut mips = 0;
        loop {
            let size = tex_format::level_bytes(format, cw as u32, ch as u32);
            let mut out = vec![0u8; size];
            let f = if format == tex_format::BC3 { texpresso::Format::Bc3 } else { texpresso::Format::Bc1 };
            f.compress(&cur, cw, ch, texpresso::Params::default(), &mut out);
            self.data.extend_from_slice(&out);
            mips += 1;
            if cw <= 4 || ch <= 4 {
                break;
            }
            let (n, nw, nh) = halve(&cur, cw, ch);
            cur = n;
            cw = nw;
            ch = nh;
        }
        // 128-byte boundaries, as GXM likes a texture's start.
        while self.data.len() % 128 != 0 {
            self.data.push(0);
        }
        self.entries.push(TexEntry { format, width: w as u32, height: h as u32, mips, offset: offset as u32, size: (self.data.len() - offset) as u32, night: NO_TEX, metres, gloss: [204, 0, 0, 0] });
        self.entries.len() as u32 - 1
    }

    pub fn add_png(&mut self, path: &Path, metres: f32) -> Result<u32, String> {
        let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?.to_rgba8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let raw = img.into_raw();
        self.means.push(mean(&raw));
        Ok(self.add_rgba(raw, w, h, metres))
    }

    /// The mean colour of a picture added from a PNG (linear), by index.
    pub fn mean(&self, tex: u32) -> [f32; 3] {
        self.means.get(tex as usize).copied().unwrap_or([0.5; 3])
    }
}
