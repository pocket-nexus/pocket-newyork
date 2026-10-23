//! Programs. The Cg sources are part of the binary; each compiles once on
//! the device through SceShaccCg and is cached as a GXP keyed by its source.
//! A packaged build ships the GXPs and never loads the compiler.

use pocket_vita_gxm::patcher::Patcher;
pub use pocket_vita_gxm::program::Blend;
use pocket_vita_gxm::program::{self, Gxp, Output, Registered};
use pocket_vita_gxm::shacccg::{Compiler, Stage};
use vita2d_sys as g;

use crate::paths;

pub type Param = *const g::SceGxmProgramParameter;

pub struct Program {
    pub vs: Registered,
    pub fs: Registered,
    pub vp: *mut g::SceGxmVertexProgram,
    first: *mut g::SceGxmFragmentProgram,
    second: *mut g::SceGxmFragmentProgram,
}

impl Program {
    /// Binds the program; `second` picks the second of its two fragment programs (the blended one, by default).
    pub unsafe fn bind(&self, ctx: *mut g::SceGxmContext, second: bool) {
        g::sceGxmSetVertexProgram(ctx, self.vp);
        g::sceGxmSetFragmentProgram(ctx, if second { self.second } else { self.first });
    }
}

/// A draw's uniforms: reserved from the context, then written by parameter.
pub struct Uniforms(*mut core::ffi::c_void);

impl Uniforms {
    pub unsafe fn vertex(ctx: *mut g::SceGxmContext) -> Uniforms {
        let mut buf = core::ptr::null_mut();
        g::sceGxmReserveVertexDefaultUniformBuffer(ctx, &mut buf);
        Uniforms(buf)
    }

    pub unsafe fn fragment(ctx: *mut g::SceGxmContext) -> Uniforms {
        let mut buf = core::ptr::null_mut();
        g::sceGxmReserveFragmentDefaultUniformBuffer(ctx, &mut buf);
        Uniforms(buf)
    }

    /// Writes `values` at the length the caller declares; a program that does not have the parameter skips it.
    #[inline]
    pub unsafe fn set(&self, param: Param, values: &[f32]) {
        if !self.0.is_null() && !param.is_null() {
            g::sceGxmSetUniformDataF(self.0, param, 0, values.len() as u32, values.as_ptr());
        }
    }
}

pub struct Gpu {
    pub patcher: Patcher,
    compiler: Option<Compiler>,
    /// Programs compiled this run (the rest came from the cache).
    pub compiled: u32,
    pub cached: u32,
    /// Hashes of every program in use, for packaging.
    pub manifest: Vec<String>,
    live: bool,
}

fn fnv64(parts: &[&[u8]]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for p in parts {
        for &b in *p {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// One vertex stream: its stride, whether it advances once per instance instead of once per vertex, and its
/// attributes as (name, offset, format, components).
pub struct Stream<'a> {
    pub stride: u16,
    pub instanced: bool,
    pub attrs: &'a [(&'a str, u16, u32, u8)],
}

impl Gpu {
    pub unsafe fn new(live: bool) -> Result<Gpu, String> {
        let _ = std::fs::create_dir_all(paths::GXP_CACHE);
        Ok(Gpu { patcher: Patcher::new(512 * 1024, 256 * 1024, 512 * 1024)?, compiler: None, compiled: 0, cached: 0, manifest: Vec::new(), live })
    }

    /// The GXP of a source: cached on the memory card, shipped in the package, or compiled now.
    unsafe fn gxp(&mut self, name: &str, source: &str, stage: Stage) -> Result<Vec<u8>, String> {
        let tag: &[u8] = match stage {
            Stage::Vertex => b"v",
            Stage::Fragment => b"f",
        };
        let hash = format!("{:016x}", fnv64(&[source.as_bytes(), tag]));
        self.manifest.push(hash.clone());
        for dir in [paths::GXP_CACHE, paths::GXP_PACKAGED] {
            if let Ok(bytes) = std::fs::read(format!("{dir}/{hash}.gxp")) {
                if bytes.len() > 16 {
                    self.cached += 1;
                    return Ok(bytes);
                }
            }
        }
        if self.compiler.is_none() {
            self.compiler = Some(Compiler::load().map_err(|e| format!("{name}: no cached program and no shader compiler ({e})"))?);
        }
        let out = self.compiler.as_ref().unwrap().compile(name, source, stage, &[]).map_err(|d| {
            let lines: Vec<String> = d.iter().map(|x| format!("{}:{}: {} {}", x.line, x.column, x.level, x.message)).collect();
            format!("{name}: {}", lines.join("; "))
        })?;
        self.compiled += 1;
        let _ = std::fs::write(format!("{}/{hash}.gxp", paths::GXP_CACHE), &out.program);
        if self.live {
            // A copy on the computer, for packaging.
            let _ = crate::hostfs::write(&format!("{}/{hash}.gxp", paths::GXP_HOST), &out.program);
        }
        Ok(out.program)
    }

    /// Compiles (or loads) a vertex and a fragment source and patches them for `streams`. `blends`: what the two
    /// fragment programs blend with; `bind(ctx, false)` takes the first.
    pub unsafe fn program(&mut self, name: &str, defines: &str, vs: &str, fs: &str, streams: &[Stream], msaa: u32, blends: [Blend; 2]) -> Result<Program, String> {
        let vsrc = format!("{defines}{vs}");
        let fsrc = format!("{defines}{fs}");
        let vbytes = self.gxp(&format!("{name}_v"), &vsrc, Stage::Vertex)?;
        let fbytes = self.gxp(&format!("{name}_f"), &fsrc, Stage::Fragment)?;
        let vs = Registered::new(self.patcher.raw, Gxp::new(&vbytes)?)?;
        let fs = Registered::new(self.patcher.raw, Gxp::new(&fbytes)?)?;
        let mut attributes = Vec::new();
        let mut layout = Vec::new();
        for (index, s) in streams.iter().enumerate() {
            for (attr, offset, format, count) in s.attrs {
                // A program that does not read an attribute of its stream has none to bind.
                let Some(reg) = vs.attribute_index(attr) else { continue };
                attributes.push(g::SceGxmVertexAttribute { streamIndex: index as u16, offset: *offset, format: *format as u8, componentCount: *count, regIndex: reg });
            }
            let source = if s.instanced { g::SceGxmIndexSource_SCE_GXM_INDEX_SOURCE_INSTANCE_16BIT } else { g::SceGxmIndexSource_SCE_GXM_INDEX_SOURCE_INDEX_16BIT };
            layout.push(g::SceGxmVertexStream { stride: s.stride, indexSource: source as u16 });
        }
        let mut vp: *mut g::SceGxmVertexProgram = core::ptr::null_mut();
        let r = g::sceGxmShaderPatcherCreateVertexProgram(self.patcher.raw, vs.id, attributes.as_ptr(), attributes.len() as u32, layout.as_ptr(), layout.len() as u32, &mut vp);
        if r < 0 {
            return Err(format!("{name}: sceGxmShaderPatcherCreateVertexProgram 0x{:08x}", r as u32));
        }
        let first = program::fragment_program(self.patcher.raw, &fs, Output::Uchar4, msaa, blends[0], vs.program())?;
        let second = program::fragment_program(self.patcher.raw, &fs, Output::Uchar4, msaa, blends[1], vs.program())?;
        Ok(Program { vs, fs, vp, first, second })
    }

    /// The compiler is only needed while programs build.
    pub fn finish(&mut self) {
        if let Some(c) = self.compiler.take() {
            c.unload();
        }
        if self.live {
            let _ = crate::hostfs::write(&format!("{}/manifest.txt", paths::GXP_HOST), self.manifest.join("\n").as_bytes());
        }
    }
}

/// Depth and culling state for opaque geometry.
pub unsafe fn state_opaque(ctx: *mut g::SceGxmContext, cull: Cull) {
    g::sceGxmSetFrontDepthFunc(ctx, g::SceGxmDepthFunc_SCE_GXM_DEPTH_FUNC_LESS_EQUAL);
    g::sceGxmSetFrontDepthWriteEnable(ctx, g::SceGxmDepthWriteMode_SCE_GXM_DEPTH_WRITE_ENABLED);
    g::sceGxmSetBackDepthFunc(ctx, g::SceGxmDepthFunc_SCE_GXM_DEPTH_FUNC_LESS_EQUAL);
    g::sceGxmSetBackDepthWriteEnable(ctx, g::SceGxmDepthWriteMode_SCE_GXM_DEPTH_WRITE_ENABLED);
    g::sceGxmSetCullMode(
        ctx,
        match cull {
            Cull::Cw => g::SceGxmCullMode_SCE_GXM_CULL_CW,
            Cull::Ccw => g::SceGxmCullMode_SCE_GXM_CULL_CCW,
            Cull::None => g::SceGxmCullMode_SCE_GXM_CULL_NONE,
        },
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cull {
    Cw,
    Ccw,
    None,
}

/// Depth-tested or not, never written, two-sided: the sky, lights, the interface.
pub unsafe fn state_overlay(ctx: *mut g::SceGxmContext, depth_test: bool) {
    let func = if depth_test { g::SceGxmDepthFunc_SCE_GXM_DEPTH_FUNC_LESS_EQUAL } else { g::SceGxmDepthFunc_SCE_GXM_DEPTH_FUNC_ALWAYS };
    g::sceGxmSetFrontDepthFunc(ctx, func);
    g::sceGxmSetFrontDepthWriteEnable(ctx, g::SceGxmDepthWriteMode_SCE_GXM_DEPTH_WRITE_DISABLED);
    g::sceGxmSetBackDepthFunc(ctx, func);
    g::sceGxmSetBackDepthWriteEnable(ctx, g::SceGxmDepthWriteMode_SCE_GXM_DEPTH_WRITE_DISABLED);
    g::sceGxmSetCullMode(ctx, g::SceGxmCullMode_SCE_GXM_CULL_NONE);
}

#[inline]
pub unsafe fn draw(ctx: *mut g::SceGxmContext, vb: *const u8, ib: *const u16, count: u32) {
    g::sceGxmSetVertexStream(ctx, 0, vb.cast());
    g::sceGxmDraw(ctx, g::SceGxmPrimitiveType_SCE_GXM_PRIMITIVE_TRIANGLES, g::SceGxmIndexFormat_SCE_GXM_INDEX_FORMAT_U16, ib.cast(), count);
}
