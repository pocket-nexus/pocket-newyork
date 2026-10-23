//! 4 × 4 matrices in row-major order, the layout `mul(M, v)` reads in Cg.

use ny_sim::math::{v3, V3};

pub type Mat4 = [f32; 16];

pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut o = [0.0f32; 16];
    for r in 0..4 {
        for c in 0..4 {
            o[r * 4 + c] = a[r * 4] * b[c] + a[r * 4 + 1] * b[4 + c] + a[r * 4 + 2] * b[8 + c] + a[r * 4 + 3] * b[12 + c];
        }
    }
    o
}

/// Perspective with depth 0 at `near` and 1 at `far`, looking down -Z.
pub fn perspective(fov_y_deg: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / libm::tanf(fov_y_deg.to_radians() * 0.5);
    let a = far / (near - far);
    [f / aspect, 0.0, 0.0, 0.0, 0.0, f, 0.0, 0.0, 0.0, 0.0, a, a * near, 0.0, 0.0, -1.0, 0.0]
}

/// View matrix for an eye looking along the unit vector `look`.
pub fn view(eye: V3, look: V3) -> Mat4 {
    let mut right = look.cross(v3(0.0, 1.0, 0.0));
    if right.length() < 1e-5 {
        right = v3(1.0, 0.0, 0.0);
    }
    let r = right.normalize();
    let u = r.cross(look);
    [r.x, r.y, r.z, -r.dot(eye), u.x, u.y, u.z, -u.dot(eye), -look.x, -look.y, -look.z, look.dot(eye), 0.0, 0.0, 0.0, 1.0]
}

/// `m × T(t) × S(s)`: folds a batch's dequantization into a view-projection.
pub fn with_bounds(m: &Mat4, t: [f32; 3], s: [f32; 3]) -> Mat4 {
    let mut o = [0.0f32; 16];
    for r in 0..4 {
        let (a, b, c, d) = (m[r * 4], m[r * 4 + 1], m[r * 4 + 2], m[r * 4 + 3]);
        o[r * 4] = a * s[0];
        o[r * 4 + 1] = b * s[1];
        o[r * 4 + 2] = c * s[2];
        o[r * 4 + 3] = a * t[0] + b * t[1] + c * t[2] + d;
    }
    o
}

/// Six planes `(n, d)` with `n·p + d >= 0` inside, from a view-projection with depth in [0, 1].
pub fn planes(m: &Mat4) -> [[f32; 4]; 6] {
    let row = |i: usize| [m[i * 4], m[i * 4 + 1], m[i * 4 + 2], m[i * 4 + 3]];
    let (x, y, z, w) = (row(0), row(1), row(2), row(3));
    let add = |a: [f32; 4], b: [f32; 4]| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
    let sub = |a: [f32; 4], b: [f32; 4]| [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]];
    [add(w, x), sub(w, x), add(w, y), sub(w, y), z, sub(w, z)]
}
