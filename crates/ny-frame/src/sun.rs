//! The sun's depth map around the eye: what stands nearest the sun over each point of a square the sun looks down
//! on, drawn before the frame from the cells near the eye and what moves, so that trees, lamp posts, awnings and
//! cars cast shadows and buildings' shadows have edges. Past the square, the height grid's sweep (`shadow`) shades
//! alone; inside it every program takes the darker of the two.

use ny_sim::math::{v3, V3};

use crate::mat::Mat4;

/// Metres from the middle of the square to its edges.
pub const HALF: f32 = 160.0;
/// Metres along the sun either side of the middle that the map's depth holds.
pub const DEEP: f32 = 600.0;

/// Where the map is this frame.
#[derive(Clone, Copy)]
pub struct SunView {
    /// From world to the map's clip space, to draw into it (its y rising with the map's rows).
    pub draw: Mat4,
    /// From world to the map's x, y (texture coordinates) and depth, to read it.
    pub read: Mat4,
    /// The middle of the square, and the sun it was placed for.
    pub middle: V3,
    pub sun: V3,
}

impl SunView {
    /// The square around `eye` for the sun towards `l` (a unit vector), on a map of `side` texels a side. Its middle
    /// keeps to whole texels in the plane the sun looks down on, so that shadows do not crawl as the eye moves.
    pub fn place(eye: V3, l: V3, side: u32) -> SunView {
        let reference = if libm::fabsf(l.y) > 0.99 { v3(0.0, 0.0, 1.0) } else { v3(0.0, 1.0, 0.0) };
        let r = l.cross(reference).normalize();
        let u = r.cross(l).normalize();
        let texel = 2.0 * HALF / side as f32;
        let (a, b) = (libm::roundf(eye.dot(r) / texel) * texel, libm::roundf(eye.dot(u) / texel) * texel);
        let along = eye.dot(l);
        let (k, d) = (1.0 / (2.0 * HALF), 1.0 / (2.0 * DEEP));
        // Depth 0 nearest the sun.
        let depth = [-l.x * d, -l.y * d, -l.z * d, (along + DEEP) * d];
        #[rustfmt::skip]
        let read = [
            r.x * k, r.y * k, r.z * k, 0.5 - a * k,
            u.x * k, u.y * k, u.z * k, 0.5 - b * k,
            depth[0], depth[1], depth[2], depth[3],
            0.0, 0.0, 0.0, 1.0,
        ];
        #[rustfmt::skip]
        let draw = [
            r.x / HALF, r.y / HALF, r.z / HALF, -a / HALF,
            u.x / HALF, u.y / HALF, u.z / HALF, -b / HALF,
            depth[0], depth[1], depth[2], depth[3],
            0.0, 0.0, 0.0, 1.0,
        ];
        SunView { draw, read, middle: r * a + u * b + l * along, sun: l }
    }

    /// No map this frame: every point reads as outside it.
    pub fn off() -> SunView {
        #[rustfmt::skip]
        let read = [0.0, 0.0, 0.0, 9.0, 0.0, 0.0, 0.0, 9.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        SunView { draw: [0.0; 16], read, middle: V3::ZERO, sun: v3(0.0, 1.0, 0.0) }
    }
}
