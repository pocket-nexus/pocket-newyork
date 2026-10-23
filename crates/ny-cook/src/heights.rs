//! The city as heights (`HMAP`): the top of whatever stands in each cell of a grid, the field the shadows are swept
//! from on a device (`ny_sim::shadow`).

use ny_pack::Heights;

pub struct Grid {
    pub head: Heights,
    pub data: Vec<u16>,
}

impl Grid {
    /// A grid of `step`-metre cells over `min .. max` (x, z).
    pub fn new(min: [f32; 2], max: [f32; 2], step: f32, unit: f32) -> Grid {
        let x0 = (min[0] / step).floor() * step;
        let z0 = (min[1] / step).floor() * step;
        // A width the device's texture rows like (a multiple of 8).
        let w = (((max[0] - x0) / step).ceil() as u32).div_ceil(8) * 8;
        let h = ((max[1] - z0) / step).ceil() as u32;
        Grid { head: Heights { x0, z0, step, unit, w, h, pad: [0; 2] }, data: vec![0; (w * h) as usize] }
    }

    fn put(&mut self, x: f32, z: f32, y: f32) {
        let h = &self.head;
        let (i, j) = (((x - h.x0) / h.step).floor(), ((z - h.z0) / h.step).floor());
        if i < 0.0 || j < 0.0 || i >= h.w as f32 || j >= h.h as f32 {
            return;
        }
        let at = j as usize * h.w as usize + i as usize;
        let q = (y.max(0.0) / h.unit).round().min(65535.0) as u16;
        if q > self.data[at] {
            self.data[at] = q;
        }
    }

    /// A box's top over its footprint: a quadrilateral (its four lower corners, in order) at height `top`.
    pub fn footprint(&mut self, corners: [[f32; 2]; 4], top: f32) {
        let step = self.head.step;
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for c in corners {
            for k in 0..2 {
                lo[k] = lo[k].min(c[k]);
                hi[k] = hi[k].max(c[k]);
            }
        }
        let inside = |p: [f32; 2]| {
            let mut sign = 0.0f32;
            for k in 0..4 {
                let (a, b) = (corners[k], corners[(k + 1) % 4]);
                let c = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
                if c.abs() < 1e-6 {
                    continue;
                }
                if sign == 0.0 {
                    sign = c.signum();
                } else if c.signum() != sign {
                    return false;
                }
            }
            true
        };
        let mut z = (lo[1] / step).floor() * step + step * 0.5;
        while z < hi[1] {
            let mut x = (lo[0] / step).floor() * step + step * 0.5;
            while x < hi[0] {
                if inside([x, z]) {
                    self.put(x, z, top);
                }
                x += step;
            }
            z += step;
        }
        // A box narrower than a cell still stands in the cell of each of its corners.
        for c in corners {
            self.put(c[0], c[1], top);
        }
    }

    /// A triangle, by points over it no farther apart than half a cell.
    pub fn triangle(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3]) {
        let span = |p: [f32; 3], q: [f32; 3]| ((p[0] - q[0]).powi(2) + (p[2] - q[2]).powi(2)).sqrt();
        let n = ((span(a, b).max(span(b, c)).max(span(c, a)) / (self.head.step * 0.5)).ceil() as usize).clamp(1, 512);
        for i in 0..=n {
            for j in 0..=(n - i) {
                let (u, v) = (i as f32 / n as f32, j as f32 / n as f32);
                let w = 1.0 - u - v;
                let p = [0, 1, 2].map(|k| a[k] * w + b[k] * u + c[k] * v);
                self.put(p[0], p[2], p[1]);
            }
        }
    }

    /// The height at a point, metres (0 outside the grid).
    pub fn at(&self, x: f32, z: f32) -> f32 {
        let h = &self.head;
        let (i, j) = (((x - h.x0) / h.step).floor(), ((z - h.z0) / h.step).floor());
        if i < 0.0 || j < 0.0 || i >= h.w as f32 || j >= h.h as f32 {
            return 0.0;
        }
        self.data[j as usize * h.w as usize + i as usize] as f32 * h.unit
    }

    pub fn bytes(&self) -> Vec<u8> {
        let mut out = ny_pack::bytes_of(&self.head).to_vec();
        out.extend_from_slice(ny_pack::slice_bytes(&self.data));
        out
    }
}
