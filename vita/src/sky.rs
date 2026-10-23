//! The sky behind the city: a dome around the eye, coloured from the hour's sky (its zenith and its horizon, the
//! sun's glow), at the far plane.

use pocket_vita_gxm::mem::{Block, Kind};
use pocket_vita_gxm::program::F32;
use vita2d_sys as g;

use crate::gpu::{self, Blend, Gpu, Param, Program, Stream, Uniforms};

const SKY_V: &str = include_str!("../shaders/sky_v.cg");
const SKY_F: &str = include_str!("../shaders/sky_f.cg");

pub struct Sky {
    _block: Block,
    prog: Program,
    mvp: Param,
    look: Param,
    dome: (*const u8, *const u16, u32),
}

impl Sky {
    /// # Safety
    /// GXM is initialized.
    pub unsafe fn new(gpu: &mut Gpu, msaa: u32) -> Result<Sky, String> {
        let mut block = Block::with_access(Kind::Main, 16 * 1024, false)?;
        // A unit sphere, its rings closer together towards the horizon, where the colour changes fastest.
        const RINGS: usize = 14;
        const AROUND: usize = 24;
        let dv = block.alloc((RINGS + 1) * AROUND * 12, 16).ok_or("sky dome")?.cast::<f32>();
        let di = block.alloc(RINGS * AROUND * 12, 16).ok_or("sky dome")?.cast::<u16>();
        for r in 0..=RINGS {
            let t = r as f32 / RINGS as f32 * 2.0 - 1.0;
            let elevation = t * t * t.signum() * core::f32::consts::FRAC_PI_2;
            for a in 0..AROUND {
                let turn = a as f32 / AROUND as f32 * core::f32::consts::TAU;
                let at = dv.add((r * AROUND + a) * 3);
                *at = libm::cosf(elevation) * libm::cosf(turn);
                *at.add(1) = libm::sinf(elevation);
                *at.add(2) = libm::cosf(elevation) * libm::sinf(turn);
            }
        }
        for r in 0..RINGS {
            for a in 0..AROUND {
                let (p, q) = ((r * AROUND + a) as u16, (r * AROUND + (a + 1) % AROUND) as u16);
                let at = di.add((r * AROUND + a) * 6);
                for (k, v) in [p, q, p + AROUND as u16, q, q + AROUND as u16, p + AROUND as u16].into_iter().enumerate() {
                    *at.add(k) = v;
                }
            }
        }
        let prog = gpu.program("sky", "", SKY_V, SKY_F, &[Stream { stride: 12, instanced: false, attrs: &[("aPosition", 0, F32, 3)] }], msaa, [Blend::Opaque, Blend::Alpha])?;
        Ok(Sky { _block: block, mvp: prog.vs.param("uMvp"), look: prog.vs.param("uSky"), prog, dome: (dv.cast(), di, (RINGS * AROUND * 6) as u32) })
    }

    /// `mvp`: the view and projection with the eye at the origin. `sky`: [0] the haze at the horizon, [1] the
    /// zenith, [2] towards the sun, [3] the glow around the sun.
    ///
    /// # Safety
    /// Inside a scene on `ctx`.
    pub unsafe fn draw(&self, ctx: *mut g::SceGxmContext, mvp: &[f32; 16], sky: &[f32; 16]) {
        self.prog.bind(ctx, false);
        gpu::state_overlay(ctx, true);
        let v = Uniforms::vertex(ctx);
        v.set(self.mvp, mvp);
        v.set(self.look, sky);
        gpu::draw(ctx, self.dome.0, self.dome.1, self.dome.2);
    }
}
