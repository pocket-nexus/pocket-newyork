//! The sun's depth map around the eye: what stands nearest the sun over each point of a square the sun looks down
//! on, drawn before the frame from the cells near the eye and what moves, so that trees, lamp posts, awnings and
//! cars cast shadows and buildings' shadows have edges. Past the square, the height grid's sweep (`shadow.rs`) shades
//! alone; inside it every program takes the darker of the two.

use pocket_vita_gxm::mem::Arena;
use pocket_vita_gxm::target::{ColorFormat, Depth, Msaa, Target};
use vita2d_sys as g;

use ny_frame::mat::Mat4;
use ny_frame::sun::SunView;
pub use ny_frame::sun::HALF;
use ny_sim::math::{v3, V3};

/// Texels along a side: 31 cm texels.
pub const SIDE: u32 = 1024;

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

    /// Places the square around `eye` for the sun towards `l` (a unit vector), in whole texels
    /// (`ny_frame::sun::SunView::place`).
    pub fn place(&mut self, eye: V3, l: V3) {
        let at = SunView::place(eye, l, SIDE);
        (self.draw, self.read, self.middle, self.sun) = (at.draw, at.read, at.middle, at.sun);
    }

    /// Leaves the map out this frame: every point reads as outside it.
    pub fn off(&mut self) {
        self.read = SunView::off().read;
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
