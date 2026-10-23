//! The sky behind the city: a dome around the eye, at the far plane, under the hour's panorama of the reference's
//! sky (`SKYP`: by longitude and latitude, graded as the display shows it).

use pocket_vita_gxm::mem::{Arena, Block, Kind};
use pocket_vita_gxm::program::F32;
use vita2d_sys as g;

use crate::gpu::{self, Blend, Gpu, Param, Program, Stream, Uniforms};

const SKY_V: &str = include_str!("../shaders/sky_v.cg");
const SKY_F: &str = include_str!("../shaders/sky_f.cg");

pub struct Sky {
    _block: Block,
    prog: Program,
    mvp: Param,
    sun: Param,
    unit: u32,
    textures: Vec<g::SceGxmTexture>,
    dome: (*const u8, *const u16, u32),
}

impl Sky {
    /// `skyp`: the pack's `SKYP` section.
    ///
    /// # Safety
    /// GXM is initialized; `vram` outlives the sky.
    pub unsafe fn new(gpu: &mut Gpu, vram: &mut Arena, msaa: u32, skyp: &[u8]) -> Result<Sky, String> {
        let head: ny_pack::SkyHead = ny_pack::read(skyp, 0).ok_or("sky record")?;
        let bytes = (head.w * head.h * 4) as usize;
        if skyp.len() < 16 + bytes * head.count as usize || head.count == 0 {
            return Err("the skies are short".into());
        }
        let mut textures = Vec::with_capacity(head.count as usize);
        for k in 0..head.count as usize {
            let at = vram.alloc(bytes, 512)?;
            core::ptr::copy_nonoverlapping(skyp[16 + k * bytes..].as_ptr(), at, bytes);
            let mut t: g::SceGxmTexture = core::mem::zeroed();
            if g::sceGxmTextureInitLinear(&mut t, at.cast(), g::SceGxmTextureFormat_SCE_GXM_TEXTURE_FORMAT_U8U8U8U8_ABGR, head.w, head.h, 1) < 0 {
                return Err("sky texture".into());
            }
            g::sceGxmTextureSetMinFilter(&mut t, g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_LINEAR);
            g::sceGxmTextureSetMagFilter(&mut t, g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_LINEAR);
            g::sceGxmTextureSetUAddrMode(&mut t, g::SceGxmTextureAddrMode_SCE_GXM_TEXTURE_ADDR_REPEAT);
            g::sceGxmTextureSetVAddrMode(&mut t, g::SceGxmTextureAddrMode_SCE_GXM_TEXTURE_ADDR_CLAMP);
            textures.push(t);
        }
        // A unit sphere by longitude and latitude, its rings closer together towards the horizon, where the colour
        // changes fastest; the first column again at the end, so that no triangle wraps round the panorama.
        const RINGS: usize = 24;
        const AROUND: usize = 32;
        const COLS: usize = AROUND + 1;
        let mut block = Block::with_access(Kind::Main, 64 * 1024, false)?;
        let dv = block.alloc((RINGS + 1) * COLS * 20, 16).ok_or("sky dome")?.cast::<f32>();
        let di = block.alloc(RINGS * AROUND * 12, 16).ok_or("sky dome")?.cast::<u16>();
        for r in 0..=RINGS {
            let t = 1.0 - r as f32 / RINGS as f32 * 2.0;
            let elevation = t * t * t.signum() * core::f32::consts::FRAC_PI_2;
            for a in 0..COLS {
                let u = a as f32 / AROUND as f32;
                let turn = u * core::f32::consts::TAU;
                let at = dv.add((r * COLS + a) * 5);
                *at = libm::cosf(elevation) * libm::cosf(turn);
                *at.add(1) = libm::sinf(elevation);
                *at.add(2) = libm::cosf(elevation) * libm::sinf(turn);
                *at.add(3) = u;
                *at.add(4) = 0.5 - elevation / core::f32::consts::PI;
            }
        }
        for r in 0..RINGS {
            for a in 0..AROUND {
                let (p, q) = ((r * COLS + a) as u16, (r * COLS + a + 1) as u16);
                let at = di.add((r * AROUND + a) * 6);
                for (k, v) in [p, q, p + COLS as u16, q, q + COLS as u16, p + COLS as u16].into_iter().enumerate() {
                    *at.add(k) = v;
                }
            }
        }
        let prog = gpu.program("sky", "", SKY_V, SKY_F, &[Stream { stride: 20, instanced: false, attrs: &[("aPosition", 0, F32, 3), ("aUv", 12, F32, 2)] }], msaa, [Blend::Opaque, Blend::Alpha])?;
        Ok(Sky { _block: block, mvp: prog.vs.param("uMvp"), sun: prog.vs.param("uSun"), unit: prog.fs.sampler_index("uSky").unwrap_or(0), textures, prog, dome: (dv.cast(), di, (RINGS * AROUND * 6) as u32) })
    }

    /// `mvp`: the view and projection with the eye at the origin; `hour`: which of the pack's hours; `sun`: towards
    /// the sun (or the moon), and its colour as the display shows it.
    ///
    /// # Safety
    /// Inside a scene on `ctx`.
    pub unsafe fn draw(&self, ctx: *mut g::SceGxmContext, mvp: &[f32; 16], hour: usize, sun: &[f32; 8]) {
        self.prog.bind(ctx, false);
        gpu::state_overlay(ctx, true);
        g::sceGxmSetFragmentTexture(ctx, self.unit, &self.textures[hour.min(self.textures.len() - 1)]);
        let v = Uniforms::vertex(ctx);
        v.set(self.mvp, mvp);
        v.set(self.sun, sun);
        gpu::draw(ctx, self.dome.0, self.dome.1, self.dome.2);
    }
}
