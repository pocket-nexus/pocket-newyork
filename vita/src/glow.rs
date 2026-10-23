//! The glow of what is bright: the reference's bloom.
//!
//! The scene is drawn straight into the display's multisampled surface. A quarter-size chain takes the
//! frame the display is showing (the one before this), keeps what is bright in it (lit windows, lamps, signs) and
//! blurs it, and the result is added over this frame: lights glow one frame late, and no pass copies the scene.
//! Every tap's texture coordinate comes from the vertex stage, so no fragment program computes one (from Pocket
//! Tokyo's `post.rs`).

use pocket_vita_gxm::mem::{Arena, Block, Kind};
use pocket_vita_gxm::program::F32;
use pocket_vita_gxm::target::{ColorFormat, Depth, Msaa, Target};
use vita2d_sys as g;

use crate::gpu::{self, Blend, Gpu, Param, Program, Stream, Uniforms};

const POST_V: &str = include_str!("../shaders/post_v.cg");
const BRIGHT_F: &str = include_str!("../shaders/bright_f.cg");
const BLUR_F: &str = include_str!("../shaders/blur_f.cg");
const GLOW_F: &str = include_str!("../shaders/glow_f.cg");

pub const W: u32 = 960;
pub const H: u32 = 544;
/// The bright chain runs at a quarter of the frame.
const QW: u32 = 240;
const QH: u32 = 136;

struct Pass {
    prog: Program,
    tap: Param,
    params: [Param; 2],
    samplers: [Option<u32>; 2],
}

impl Pass {
    #[allow(clippy::too_many_arguments)]
    unsafe fn new(gpu: &mut Gpu, name: &str, fs: &str, uniforms: [&str; 2], samplers: [&str; 2], msaa: u32, blend: Blend) -> Result<Pass, String> {
        let prog = gpu.program(name, "", POST_V, fs, &[Stream { stride: 8, instanced: false, attrs: &[("aPosition", 0, F32, 2)] }], msaa, [blend, Blend::Alpha])?;
        let tap = prog.vs.param("uTap");
        let params = uniforms.map(|u| if u.is_empty() { core::ptr::null() } else { prog.fs.param(u) });
        let samplers = samplers.map(|s| if s.is_empty() { None } else { prog.fs.sampler_index(s) });
        Ok(Pass { prog, tap, params, samplers })
    }

    /// Draws the full-screen triangle with this pass's taps, fragment uniforms and textures.
    unsafe fn draw(&self, ctx: *mut g::SceGxmContext, tri: (*const u8, *const u16), taps: &[f32; 32], uniforms: [&[f32]; 2], textures: [Option<&g::SceGxmTexture>; 2]) {
        self.prog.bind(ctx, false);
        gpu::state_overlay(ctx, false);
        Uniforms::vertex(ctx).set(self.tap, taps);
        if self.params.iter().any(|p| !p.is_null()) {
            let f = Uniforms::fragment(ctx);
            for (p, v) in self.params.iter().zip(uniforms) {
                if !v.is_empty() {
                    f.set(*p, v);
                }
            }
        }
        for (unit, tex) in self.samplers.iter().zip(textures) {
            if let (Some(unit), Some(tex)) = (unit, tex) {
                g::sceGxmSetFragmentTexture(ctx, *unit, tex);
            }
        }
        gpu::draw(ctx, tri.0, tri.1, 3);
    }
}

/// Eight identity taps.
fn taps() -> [f32; 32] {
    let mut t = [0.0; 32];
    for k in 0..8 {
        t[k * 4] = 1.0;
        t[k * 4 + 1] = 1.0;
    }
    t
}

unsafe fn viewport(ctx: *mut g::SceGxmContext, w: u32, h: u32) {
    let (hw, hh) = (w as f32 * 0.5, h as f32 * 0.5);
    g::sceGxmSetViewport(ctx, hw, hw, hh, -hh, 0.0, 1.0);
    g::sceGxmSetRegionClip(ctx, g::SceGxmRegionClipMode_SCE_GXM_REGION_CLIP_OUTSIDE, 0, 0, w - 1, h - 1);
}

pub struct Glow {
    _block: Block,
    tri: (*const u8, *const u16),
    a: Target,
    b: Target,
    bright: Pass,
    blur: Pass,
    glow: Pass,
    /// Whether `a` holds a glow to add to this frame.
    lit: bool,
}

impl Glow {
    /// # Safety
    /// GXM is initialized; the arenas outlive the targets.
    pub unsafe fn new(gpu: &mut Gpu, vram: &mut Arena, main: &mut Arena, msaa: u32) -> Result<Glow, String> {
        let mut block = Block::with_access(Kind::Main, 4096, false)?;
        let vb = block.alloc(24, 16).ok_or("glow triangle")?.cast::<f32>();
        let ib = block.alloc(8, 16).ok_or("glow triangle")?.cast::<u16>();
        for (i, v) in [-1.0f32, -1.0, 3.0, -1.0, -1.0, 3.0].into_iter().enumerate() {
            *vb.add(i) = v;
        }
        for i in 0..3 {
            *ib.add(i) = i as u16;
        }
        let a = Target::new(vram, main, QW, QH, ColorFormat::Rgba8, Msaa::None, Depth::None)?;
        let b = Target::new(vram, main, QW, QH, ColorFormat::Rgba8, Msaa::None, Depth::None)?;
        Ok(Glow {
            tri: (vb.cast(), ib),
            _block: block,
            a,
            b,
            bright: Pass::new(gpu, "bright", BRIGHT_F, ["uParams", ""], ["uSource", ""], 0, Blend::Opaque)?,
            blur: Pass::new(gpu, "blur", BLUR_F, ["", ""], ["uSource", ""], 0, Blend::Opaque)?,
            glow: Pass::new(gpu, "glow", GLOW_F, ["uGlow", ""], ["uBloom", ""], msaa, Blend::Additive)?,
            lit: false,
        })
    }

    /// Runs the quarter-size chain over the frame the display shows (`shown`: `W` × `H` texels in rows of
    /// `stride`): bright, blur across, blur down. Nothing when `gain` is 0.
    ///
    /// # Safety
    /// Outside any scene.
    pub unsafe fn run(&mut self, ctx: *mut g::SceGxmContext, gain: f32, threshold: f32, shown: *const core::ffi::c_void, stride: u32) -> Result<(), String> {
        self.lit = false;
        if gain <= 0.0 || shown.is_null() {
            return Ok(());
        }
        let mut source: g::SceGxmTexture = core::mem::zeroed();
        if g::sceGxmTextureInitLinear(&mut source, shown, g::SceGxmTextureFormat_SCE_GXM_TEXTURE_FORMAT_U8U8U8U8_ABGR, stride, H, 1) < 0 {
            return Ok(());
        }
        g::sceGxmTextureSetMinFilter(&mut source, g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_LINEAR);
        g::sceGxmTextureSetMagFilter(&mut source, g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_LINEAR);
        // Bright: four taps one source texel off the centre, each a bilinear 2 x 2. The frame is the left part of its rows.
        let wide = W as f32 / stride as f32;
        let mut t = taps();
        let (tx, ty) = (1.0 / stride as f32, 1.0 / H as f32);
        for (k, (sx, sy)) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)].into_iter().enumerate() {
            t[k * 4] = wide;
            t[k * 4 + 2] = sx * tx;
            t[k * 4 + 3] = sy * ty;
        }
        self.a.begin(ctx, 1.0)?;
        viewport(ctx, QW, QH);
        self.bright.draw(ctx, self.tri, &t, [&[threshold, 1.0 / (1.0 - threshold).max(0.05), 0.0, 0.0], &[]], [Some(&source), None]);
        self.a.end(ctx, None);
        // Blur: across into b, down back into a.
        for (from_a, (dx, dy)) in [(true, (1.0 / QW as f32, 0.0)), (false, (0.0, 1.0 / QH as f32))] {
            let mut t = taps();
            for (k, o) in [0.0f32, 1.3846, -1.3846, 3.2308, -3.2308].into_iter().enumerate() {
                t[k * 4 + 2] = o * dx;
                t[k * 4 + 3] = o * dy;
            }
            let (src, dst) = if from_a { (&self.a.texture as *const g::SceGxmTexture, &mut self.b) } else { (&self.b.texture as *const g::SceGxmTexture, &mut self.a) };
            dst.begin(ctx, 1.0)?;
            viewport(ctx, QW, QH);
            self.blur.draw(ctx, self.tri, &t, [&[], &[]], [Some(&*src), None]);
            dst.end(ctx, None);
        }
        self.lit = true;
        Ok(())
    }

    /// Adds the glow over the open display scene.
    ///
    /// # Safety
    /// Inside the display scene.
    pub unsafe fn add(&self, ctx: *mut g::SceGxmContext, gain: f32) {
        if self.lit {
            viewport(ctx, W, H);
            self.glow.draw(ctx, self.tri, &taps(), [&[gain, 0.0, 0.0, 0.0], &[]], [Some(&self.a.texture), None]);
        }
    }
}
