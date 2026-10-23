//! Matrices as three.js stores them (column-major `[f32; 16]`), and what the compiler does with them.

pub type M4 = [f32; 16];

pub const IDENTITY: M4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

pub fn mul(a: &M4, b: &M4) -> M4 {
    let mut o = [0.0f32; 16];
    for c in 0..4 {
        for r in 0..4 {
            o[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    o
}

#[inline]
pub fn point(m: &M4, p: [f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

/// What carries normals through `m`: its cofactors (the inverse transpose times the determinant), with the sign of
/// the determinant, so they keep their side. The second value is whether `m` mirrors (triangles turn over).
pub fn normal_matrix(m: &M4) -> ([f32; 9], bool) {
    let a = [m[0], m[1], m[2], m[4], m[5], m[6], m[8], m[9], m[10]];
    // columns c0 = a[0..3], c1 = a[3..6], c2 = a[6..9]
    let c0 = [a[0], a[1], a[2]];
    let c1 = [a[3], a[4], a[5]];
    let c2 = [a[6], a[7], a[8]];
    let x0 = cross(c1, c2);
    let x1 = cross(c2, c0);
    let x2 = cross(c0, c1);
    let det = dot(c0, x0);
    let s = if det < 0.0 { -1.0 } else { 1.0 };
    // n' = cof(M) n, with cof(M) whose rows are x0, x1, x2
    ([x0[0] * s, x1[0] * s, x2[0] * s, x0[1] * s, x1[1] * s, x2[1] * s, x0[2] * s, x1[2] * s, x2[2] * s], det < 0.0)
}

#[inline]
pub fn normal(n: &[f32; 9], v: [f32; 3]) -> [f32; 3] {
    normalize([n[0] * v[0] + n[3] * v[1] + n[6] * v[2], n[1] * v[0] + n[4] * v[1] + n[7] * v[2], n[2] * v[0] + n[5] * v[1] + n[8] * v[2]])
}

#[inline]
pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

#[inline]
pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
pub fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
pub fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
pub fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
pub fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

#[inline]
pub fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = length(a);
    if l > 1e-12 {
        scale(a, 1.0 / l)
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// An axis-aligned box.
#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb { min: [f32::MAX; 3], max: [f32::MIN; 3] };

    pub fn add(&mut self, p: [f32; 3]) {
        for k in 0..3 {
            self.min[k] = self.min[k].min(p[k]);
            self.max[k] = self.max[k].max(p[k]);
        }
    }
    pub fn union(&mut self, o: &Aabb) {
        if o.is_empty() {
            return;
        }
        self.add(o.min);
        self.add(o.max);
    }
    pub fn is_empty(&self) -> bool {
        self.min[0] > self.max[0]
    }
    pub fn center(&self) -> [f32; 3] {
        scale(add(self.min, self.max), 0.5)
    }
    pub fn size(&self) -> [f32; 3] {
        sub(self.max, self.min)
    }
    /// Half the diagonal.
    pub fn radius(&self) -> f32 {
        length(self.size()) * 0.5
    }
}

/// sRGB byte to linear.
pub fn linear(c: u8) -> f32 {
    let s = c as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear to an sRGB byte.
pub fn srgb(x: f32) -> u8 {
    let x = x.clamp(0.0, 1.0);
    let s = if x <= 0.0031308 { x * 12.92 } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round() as u8
}

pub fn snorm8(v: f32) -> i8 {
    (v.clamp(-1.0, 1.0) * 127.0).round() as i8
}
