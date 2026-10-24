//! BC1 and BC3 blocks taken apart into rows of RGBA, for a GPU that reads no blocks (a phone's).

/// The four colours of a block's endpoints (`r5 g6 b5` each), as BC1 decodes them; `alpha`: whether the third
/// stands between them with the fourth transparent (BC1 with `c0 <= c1`), or two thirds (BC3 always).
fn palette(c0: u16, c1: u16, alpha: bool) -> [[u8; 4]; 4] {
    let rgb = |c: u16| {
        let (r, g, b) = ((c >> 11) & 31, (c >> 5) & 63, c & 31);
        [((r << 3) | (r >> 2)) as u32, ((g << 2) | (g >> 4)) as u32, ((b << 3) | (b >> 2)) as u32]
    };
    let (a, b) = (rgb(c0), rgb(c1));
    let mix = |wa: u32, wb: u32, d: u32| [0, 1, 2].map(|k| ((a[k] * wa + b[k] * wb) / d) as u8);
    let solid = |c: [u32; 3]| [c[0] as u8, c[1] as u8, c[2] as u8, 255];
    if c0 > c1 || !alpha {
        let (x, y) = (mix(2, 1, 3), mix(1, 2, 3));
        [solid(a), solid(b), [x[0], x[1], x[2], 255], [y[0], y[1], y[2], 255]]
    } else {
        let m = mix(1, 1, 2);
        [solid(a), solid(b), [m[0], m[1], m[2], 255], [0, 0, 0, 0]]
    }
}

/// A level of `w` × `h` texels of blocks (`bc3`: 16 bytes a block, alpha first; else BC1, 8) as rows of RGBA.
pub fn decode(bc3: bool, data: &[u8], w: u32, h: u32, out: &mut Vec<u8>) {
    let (w, h) = (w as usize, h as usize);
    out.clear();
    out.resize(w * h * 4, 0);
    let per = if bc3 { 16 } else { 8 };
    let bw = w.div_ceil(4);
    for (n, block) in data.chunks_exact(per).enumerate() {
        let (bx, by) = ((n % bw) * 4, (n / bw) * 4);
        if by >= h {
            break;
        }
        let colour = &block[per - 8..];
        let c0 = u16::from_le_bytes([colour[0], colour[1]]);
        let c1 = u16::from_le_bytes([colour[2], colour[3]]);
        let p = palette(c0, c1, !bc3);
        let bits = u32::from_le_bytes([colour[4], colour[5], colour[6], colour[7]]);
        // BC3's alpha: two endpoints and sixteen 3-bit indices.
        let alphas = bc3.then(|| {
            let (a0, a1) = (block[0] as u32, block[1] as u32);
            let mut table = [a0, a1, 0, 0, 0, 0, 0, 0];
            if a0 > a1 {
                for k in 1..7 {
                    table[k + 1] = ((7 - k as u32) * a0 + k as u32 * a1) / 7;
                }
            } else {
                for k in 1..5 {
                    table[k + 1] = ((5 - k as u32) * a0 + k as u32 * a1) / 5;
                }
                table[6] = 0;
                table[7] = 255;
            }
            let mut indices = 0u64;
            for k in 0..6 {
                indices |= (block[2 + k] as u64) << (8 * k);
            }
            (table, indices)
        });
        for y in 0..4 {
            for x in 0..4 {
                let (px, py) = (bx + x, by + y);
                if px >= w || py >= h {
                    continue;
                }
                let k = y * 4 + x;
                let mut texel = p[((bits >> (2 * k)) & 3) as usize];
                if let Some((table, indices)) = alphas {
                    texel[3] = table[((indices >> (3 * k)) & 7) as usize] as u8;
                }
                out[(py * w + px) * 4..(py * w + px) * 4 + 4].copy_from_slice(&texel);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_of_one_colour_decodes_to_it() {
        // BC1, both endpoints pure red, every index 0.
        let red = 0xf800u16.to_le_bytes();
        let block = [red[0], red[1], red[0], red[1], 0, 0, 0, 0];
        let mut out = Vec::new();
        decode(false, &block, 4, 4, &mut out);
        assert!(out.chunks(4).all(|t| t == [255, 0, 0, 255]));
        // BC3, alpha 128 everywhere over the same colour.
        let mut block3 = vec![128, 128, 0, 0, 0, 0, 0, 0];
        block3.extend_from_slice(&block);
        decode(true, &block3, 4, 4, &mut out);
        assert!(out.chunks(4).all(|t| t == [255, 0, 0, 128]));
    }
}
