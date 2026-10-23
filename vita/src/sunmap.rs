//! The sun's depth map around the eye: what stands nearest the sun over each point of a square the sun looks down
//! on, drawn before the frame from the cells near the eye and what moves, so that trees, lamp posts, awnings and
//! cars cast shadows and buildings' shadows have edges. Past the square, the height grid's sweep (`shadow.rs`) shades
//! alone; inside it every program takes the darker of the two.

use pocket_vita_gxm::mem::Arena;
use pocket_vita_gxm::target::{ColorFormat, Depth, Msaa, Target};
use vita2d_sys as g;

use ny_sim::math::{v3, V3};

use crate::mat::Mat4;

/// Texels along a side.
pub const SIDE: u32 = 1024;
/// Metres from the middle of the square to its edges: 31 cm texels.
pub const HALF: f32 = 160.0;
/// Metres along the sun either side of the middle that the map's depth holds.
const DEEP: f32 = 600.0;

pub struct SunMap {
    pub target: Target,
    /// From world to the map's clip space, to draw into it.
    pub draw: Mat4,
    /// From world to the map's x, y (texture coordinates) and depth, to read it.
    pub read: Mat4,
    /// The middle of the square, and the sun it was placed for.
    pub middle: V3,
    pub sun: V3,
}

impl SunMap {
    /// # Safety
    /// GXM is initialized; the arenas outlive the map.
    pub unsafe fn new(vram: &mut Arena, main: &mut Arena) -> Result<SunMap, String> {
        let target = Target::new(vram, main, SIDE, SIDE, ColorFormat::Rgba8, Msaa::None, Depth::Stored)?;
        Ok(SunMap { target, draw: [0.0; 16], read: [0.0; 16], middle: V3::ZERO, sun: v3(0.0, 1.0, 0.0) })
    }

    /// Places the square around `eye` for the sun towards `l` (a unit vector). Its middle keeps to whole texels in
    /// the plane the sun looks down on, so that shadows do not crawl as the eye moves.
    pub fn place(&mut self, eye: V3, l: V3) {
        let reference = if l.y.abs() > 0.99 { v3(0.0, 0.0, 1.0) } else { v3(0.0, 1.0, 0.0) };
        let r = l.cross(reference).normalize();
        let u = r.cross(l).normalize();
        let texel = 2.0 * HALF / SIDE as f32;
        let (a, b) = (libm::roundf(eye.dot(r) / texel) * texel, libm::roundf(eye.dot(u) / texel) * texel);
        let along = eye.dot(l);
        self.middle = r * a + u * b + l * along;
        self.sun = l;
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
        self.read = read;
        self.draw = draw;
    }

    /// Leaves the map out this frame: every point reads as outside it.
    pub fn off(&mut self) {
        #[rustfmt::skip]
        let read = [0.0, 0.0, 0.0, 9.0, 0.0, 0.0, 0.0, 9.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        self.read = read;
    }

    /// Opens the map's scene, its viewport rows running with the map's y (texture row 0 at y = 0).
    ///
    /// # Safety
    /// No other scene open on `ctx`.
    pub unsafe fn begin(&mut self, ctx: *mut g::SceGxmContext) -> Result<(), String> {
        self.target.begin(ctx, 1.0)?;
        let h = SIDE as f32 * 0.5;
        g::sceGxmSetViewport(ctx, h, h, h, h, 0.0, 1.0);
        g::sceGxmSetRegionClip(ctx, g::SceGxmRegionClipMode_SCE_GXM_REGION_CLIP_OUTSIDE, 0, 0, SIDE - 1, SIDE - 1);
        Ok(())
    }

    /// # Safety
    /// Inside the map's scene.
    pub unsafe fn end(&self, ctx: *mut g::SceGxmContext) {
        self.target.end(ctx, None);
    }

    pub fn texture(&self) -> &g::SceGxmTexture {
        self.target.depth_texture.as_ref().expect("the sun's map keeps its depth")
    }
}
