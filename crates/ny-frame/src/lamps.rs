//! The lamps' light by night (`LAMP`): what they leave on the ground under them, as the pack holds it, and that
//! light around a point, as the street and its storefronts hand it on. Both are `r5 g6 b5` texels, each channel the
//! square root of its share of the head's `unit`.

use alloc::vec;
use alloc::vec::Vec;

use ny_pack::Heights;

/// From world x and z to the lamps' texture coordinates: x scale, x offset, z scale, z offset.
pub fn mapping(head: &Heights) -> [f32; 4] {
    let (sx, sz) = (1.0 / (head.w as f32 * head.step), 1.0 / (head.h as f32 * head.step));
    [sx, -head.x0 * sx, sz, -head.z0 * sz]
}

/// The light around a point from `texels` (the section's grid, after its head): the pools averaged over 16 m
/// cells, then twice over 5 × 5 of those (about 80 m). Its width, height and texels, on the same coordinates.
pub fn around(head: &Heights, texels: &[u8]) -> (u32, u32, Vec<u16>) {
    let decode = |v: u16| [((v >> 11) & 31) as f32 / 31.0, ((v >> 5) & 63) as f32 / 63.0, (v & 31) as f32 / 31.0].map(|s| s * s);
    const D: u32 = 4;
    let (cw, ch) = (head.w.div_ceil(D), head.h.div_ceil(D));
    let mut coarse = vec![[0.0f32; 3]; (cw * ch) as usize];
    for z in 0..head.h {
        for x in 0..head.w {
            let i = ((z * head.w + x) * 2) as usize;
            let c = decode(u16::from_le_bytes([texels[i], texels[i + 1]]));
            let o = &mut coarse[((z / D) * cw + x / D) as usize];
            for k in 0..3 {
                o[k] += c[k] / (D * D) as f32;
            }
        }
    }
    for _ in 0..2 {
        let src = coarse.clone();
        for z in 0..ch as i32 {
            for x in 0..cw as i32 {
                let mut s = [0.0f32; 3];
                for dz in -2..=2 {
                    for dx in -2..=2 {
                        let (xx, zz) = ((x + dx).clamp(0, cw as i32 - 1), (z + dz).clamp(0, ch as i32 - 1));
                        let c = src[(zz as u32 * cw + xx as u32) as usize];
                        for k in 0..3 {
                            s[k] += c[k] / 25.0;
                        }
                    }
                }
                coarse[(z as u32 * cw + x as u32) as usize] = s;
            }
        }
    }
    let q = |v: f32, max: f32| libm::roundf(libm::sqrtf(v.clamp(0.0, 1.0)) * max) as u16;
    let out = coarse.iter().map(|c| q(c[0], 31.0) << 11 | q(c[1], 63.0) << 5 | q(c[2], 31.0)).collect();
    (cw, ch, out)
}
