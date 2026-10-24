//! The city on the GPU: the pack's tables in memory, its own streams (shapes, the mid and far levels) in one block
//! of GPU-mapped memory, the facade pages in video memory, the cells' records read as the eye moves (`near.rs`),
//! and a frame's draws, chosen by `ny_view::select` and grouped by program and picture.

use pocket_vita_gxm::mem::{Arena, Block as Memory, Kind};
use pocket_vita_gxm::program::{F32, S16N, S8N, U16N, U8N};
use pocket_vita_gxm::texture::{Format, Texture, Uploader, Wrap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use ny_pack::{self as pack, kind, Batch, Block, Cell, City, Instance, Model, NearCell, Region, TexEntry, NO_TEX};
use pocket_vita_gxm::mem::Ring;
use ny_sim::math::V3;
use ny_view::{Item, Planes, Reach, Tables, RESIDENT};
use vita2d_sys as g;

use crate::gpu::{self, Blend, Cull, Gpu, Param, Program, Stream, Uniforms};
use crate::near::Near;
use ny_frame::light::{AROUND, SHADOW_OUT, SHADOW_SOFT, SUN_BIAS};
use ny_frame::mat::{self, Mat4};
use ny_frame::trees::{Reach as TreeReach, Trees};
use crate::sunmap::SunMap;

const FRAME: &str = include_str!("../shaders/frame.cg");
const WALL_V: &str = include_str!("../shaders/wall_v.cg");
const WALL_F: &str = include_str!("../shaders/wall_f.cg");
const SOLID_V: &str = include_str!("../shaders/solid_v.cg");
const SOLID_F: &str = include_str!("../shaders/solid_f.cg");
const INST_V: &str = include_str!("../shaders/inst_v.cg");
const GROUND_V: &str = include_str!("../shaders/ground_v.cg");
const LEAF_V: &str = include_str!("../shaders/leaf_v.cg");
const LEAF_F: &str = include_str!("../shaders/leaf_f.cg");
const GROUND_F: &str = include_str!("../shaders/ground_f.cg");
const PICTURE_V: &str = include_str!("../shaders/picture_v.cg");
const BOX_V: &str = include_str!("../shaders/box_v.cg");
const SUN_V: &str = include_str!("../shaders/sun_v.cg");
const SUN_F: &str = include_str!("../shaders/sun_f.cg");
const BOX_F: &str = include_str!("../shaders/box_f.cg");
const PICTURE_F: &str = include_str!("../shaders/picture_f.cg");

pub use ny_frame::light::{Frame, Haze, LIGHT_SCALE};
/// Slots for cells' records (each as large as the largest record).
const NEAR_SLOTS: usize = 72;
/// Instances of models a frame may draw (every car of the traffic, and the player).
pub const MODEL_INSTANCES: usize = 1024;

/// A pack on storage: the section table, and reads of what a section holds.
pub struct PackFile {
    file: File,
    index: pack::Index,
    pub path: String,
}

impl PackFile {
    pub fn open(path: &str) -> Result<PackFile, String> {
        let mut file = File::open(path).map_err(|e| format!("{path}: {e}"))?;
        let mut head = [0u8; 16];
        file.read_exact(&mut head).map_err(|e| format!("{path}: {e}"))?;
        let count = pack::Index::count(&head)?;
        let mut table = vec![0u8; pack::Index::head_bytes(count)];
        table[..16].copy_from_slice(&head);
        file.read_exact(&mut table[16..]).map_err(|e| format!("{path}: {e}"))?;
        Ok(PackFile { file, index: pack::Index::parse(&table)?, path: path.to_string() })
    }

    pub fn range(&self, tag: u32) -> Result<(usize, usize), String> {
        self.index.range(tag)
    }

    pub fn size(&self, tag: u32) -> Result<usize, String> {
        Ok(self.index.range(tag)?.1)
    }

    /// `len` bytes of a section from `at`, into `to`.
    ///
    /// # Safety
    /// `to` holds `len` bytes.
    pub unsafe fn read_into(&mut self, tag: u32, at: usize, to: *mut u8, len: usize) -> Result<(), String> {
        let (offset, size) = self.index.range(tag)?;
        if at + len > size {
            return Err("read past the end of a pack section".into());
        }
        self.file.seek(SeekFrom::Start((offset + at) as u64)).map_err(|e| e.to_string())?;
        // In pieces: one request of tens of megabytes holds the card for seconds.
        let mut done = 0;
        while done < len {
            let n = (len - done).min(1 << 20);
            self.file.read_exact(core::slice::from_raw_parts_mut(to.add(done), n)).map_err(|e| format!("{}: {e}", self.path))?;
            done += n;
        }
        Ok(())
    }

    pub fn read_range(&mut self, tag: u32, at: usize, len: usize) -> Result<Vec<u8>, String> {
        let mut bytes = vec![0u8; len];
        unsafe { self.read_into(tag, at, bytes.as_mut_ptr(), len)? };
        Ok(bytes)
    }

    pub fn read(&mut self, tag: u32) -> Result<Vec<u8>, String> {
        let n = self.size(tag)?;
        let mut bytes = vec![0u8; n];
        unsafe { self.read_into(tag, 0, bytes.as_mut_ptr(), n)? };
        Ok(bytes)
    }

    /// A section, or nothing when the pack has none.
    pub fn read_opt(&mut self, tag: u32) -> Option<Vec<u8>> {
        self.read(tag).ok()
    }
}

/// A scene program and where its uniforms go.
struct Pass {
    prog: Program,
    mvp: Param,
    tile: Param,
    lamp_map: Param,
    lamp: Option<u32>,
    around: Option<u32>,
    /// The sun's depth map: from world to it, and its sampler.
    sun_vp: Param,
    sun: Option<u32>,
    vp: Param,
    bx: Param,
    frame: Param,
    look: Param,
    gloss: Param,
    bake: Param,
    veil: Param,
    /// The eye, and the square of the distance within which a tree's crown is drawn finer (`ny_frame::trees`).
    cut: Param,
    units: [u32; 2],
    shadow: Option<u32>,
}

impl Pass {
    unsafe fn new(prog: Program, pictures: [&str; 2]) -> Pass {
        let units = pictures.map(|name| if name.is_empty() { 0 } else { prog.fs.sampler_index(name).unwrap_or(0) });
        Pass { mvp: prog.vs.param("uMvp"), tile: prog.vs.param("uTile"), lamp_map: prog.vs.param("uLampMap"), lamp: prog.fs.sampler_index("uLamp"), around: prog.fs.sampler_index("uAround"), sun_vp: prog.vs.param("uSunVp"), sun: prog.fs.sampler_index("uSun"), vp: prog.vs.param("uVp"), bx: prog.vs.param("uBox"), frame: prog.vs.param("uFrame"), look: prog.fs.param("uLook"), gloss: prog.vs.param("uGloss"), bake: prog.vs.param("uBake"), veil: prog.fs.param("uVeil"), cut: prog.vs.param("uCut"), units, shadow: prog.fs.sampler_index("uShadow"), prog }
    }

    /// Binds the program, the shadow window and the lamps' light (under them, and around).
    unsafe fn bind(&self, ctx: *mut g::SceGxmContext, shadow: &g::SceGxmTexture, lamp: &g::SceGxmTexture, around: &g::SceGxmTexture, sun: &g::SceGxmTexture) {
        if let Some(unit) = self.sun {
            g::sceGxmSetFragmentTexture(ctx, unit, sun);
        }
        self.prog.bind(ctx, false);
        if let Some(unit) = self.shadow {
            g::sceGxmSetFragmentTexture(ctx, unit, shadow);
        }
        if let Some(unit) = self.around {
            g::sceGxmSetFragmentTexture(ctx, unit, around);
        }
        if let Some(unit) = self.lamp {
            g::sceGxmSetFragmentTexture(ctx, unit, lamp);
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct Stats {
    pub draws: u32,
    /// Triangles: walls, solids, instances.
    pub tris: [u32; 3],
    /// Places drawn: near cells, mid blocks, far regions, detail cells.
    pub places: [u32; 4],
}

/// What a frame draws.
#[derive(Clone, Copy)]
pub struct Show {
    pub reach: Reach,
    pub walls: bool,
    pub solids: bool,
    pub instances: bool,
    /// Metres within which trees are drawn.
    pub trees: f32,
}

/// The pack's own streams, in GPU-mapped memory.
struct Own {
    solids: *const u8,
    walls: *const u8,
    grounds: *const u8,
    indices: *const u16,
    instances: *const u8,
}

pub struct CityGpu {
    pub city: City,
    cells: Vec<Cell>,
    blocks: Vec<Block>,
    regions: Vec<Region>,
    batches: Vec<Batch>,
    spans: Vec<u32>,
    near_cells: Vec<NearCell>,
    pub near: Near,
    _memory: Memory,
    own: Own,
    texs: Vec<TexEntry>,
    textures: Vec<Option<Texture>>,
    wall: Pass,
    wall_night: Pass,
    solid: Pass,
    open: Pass,
    inst: Pass,
    inst_open: Pass,
    ground: Pass,
    ground_night: Pass,
    leaf: Pass,
    /// By night: the same, lit by the lamps too.
    solid_night: Pass,
    open_night: Pass,
    inst_night: Pass,
    inst_open_night: Pass,
    leaf_night: Pass,
    /// Cars under their pictures (`kind::BOXED_INST`), by day and by night.
    boxed: Pass,
    boxed_night: Pass,
    /// Into the sun's depth map: a shape's points, a shape at instances, a tree's cut cards.
    sun_solid: Pass,
    sun_inst: Pass,
    sun_leaf: Pass,
    /// A model's part seen through (`Batch::veil`), blended over what is behind it.
    veil: Pass,
    veil_night: Pass,
    picture: Pass,
    picture_inst: Pass,
    /// The lamps' light on the ground by night, and from world x and z to its texture coordinates; and that light
    /// around a point (over about 80 m), on the same coordinates.
    lamp: g::SceGxmTexture,
    around: g::SceGxmTexture,
    lamp_map: [f32; 4],
    /// The light a channel's full value stands for.
    pub lamp_unit: f32,
    items: Vec<Item>,
    wanted: Vec<(u32, f32)>,
    pub models: Vec<Model>,
    /// This frame's instances of the models, in a ring of two frames.
    ring: Ring,
    /// The first of the trees' models (`tree:`, past every other), and this frame's cut (`place_trees`).
    first_tree: usize,
    cut: [f32; 4],
    pub geometry_bytes: usize,
    pub texture_bytes: usize,
}

/// What every scene program is compiled with.
/// (`shadow_unit`: metres of height in one unit of the shadow texture as a program reads it; `extra`: lines a
/// development build's boot switches add, for measurements.)
pub fn scene_defines(shadow_unit: f32, extra: &str) -> String {
    format!(
        "{extra}#define AROUND {AROUND:.1}\n#define SUN_BIAS {SUN_BIAS:.4}\n#define UV_RANGE {:.1}\n#define LIGHT_SCALE {LIGHT_SCALE:.1}\n#define GLOW_RANGE {:.1}\n#define SHADOW_UNIT {shadow_unit:.3}\n#define SHADOW_K {:.3}\n#define SHADOW_OUT {SHADOW_OUT:.1}\n{FRAME}",
        pack::UV_RANGE,
        pack::GLOW_RANGE,
        1.0 / SHADOW_SOFT
    )
}


const WALL_ATTRS: [(&str, u16, u32, u8); 5] = [("aPosition", 0, U16N, 3), ("aNormal", 6, S8N, 3), ("aAo", 9, U8N, 1), ("aUv", 10, S16N, 2), ("aTint", 14, U8N, 4)];
const SOLID_ATTRS: [(&str, u16, u32, u8); 6] = [("aPosition", 0, U16N, 3), ("aNormal", 6, S8N, 3), ("aAo", 9, U8N, 1), ("aColor", 10, U8N, 4), ("aGlow", 14, U8N, 4), ("aGloss", 18, U8N, 2)];
const LEAF_ATTRS: [(&str, u16, u32, u8); 5] = [("aPosition", 0, U16N, 3), ("aNormal", 6, S8N, 3), ("aAo", 9, U8N, 1), ("aUv", 10, S16N, 2), ("aBark", 14, U8N, 4)];
const PICTURE_ATTRS: [(&str, u16, u32, u8); 5] = [("aPosition", 0, U16N, 3), ("aNormal", 6, S8N, 3), ("aAo", 9, U8N, 1), ("aUv", 10, S16N, 2), ("aSign", 14, U8N, 4)];
const GROUND_ATTRS: [(&str, u16, u32, u8); 4] = [("aPosition", 0, U16N, 3), ("aNormal", 6, S8N, 3), ("aAo", 9, U8N, 1), ("aTint", 10, U8N, 4)];
const INST_ATTRS: [(&str, u16, u32, u8); 4] = [("aRow0", 0, F32, 4), ("aRow1", 16, F32, 4), ("aRow2", 32, F32, 4), ("aTint", 48, U8N, 4)];

impl CityGpu {
    /// # Safety
    /// GXM is initialized; `vram` outlives the city.
    pub unsafe fn load(p: &mut PackFile, gpu: &mut Gpu, vram: &mut Arena, msaa: u32, shadow_unit: f32, extra: &str, mut progress: impl FnMut(&str)) -> Result<CityGpu, String> {
        let city: City = pack::read(&p.read(pack::CITY)?, 0).ok_or("city record")?;
        let cells: Vec<Cell> = pack::table(&p.read(pack::CELL)?);
        let blocks: Vec<Block> = pack::table(&p.read(pack::BLCK)?);
        let regions: Vec<Region> = pack::table(&p.read(pack::REGN)?);
        let batches: Vec<Batch> = pack::table(&p.read(pack::BTCH)?);
        let spans: Vec<u32> = pack::table(&p.read(pack::SPAN)?);
        let near_cells: Vec<NearCell> = pack::table(&p.read(pack::NCEL)?);
        let texs: Vec<TexEntry> = pack::table(&p.read(pack::TEXS)?);
        let models: Vec<Model> = pack::table(&p.read(pack::MODL)?);
        let first_tree = models.iter().position(|m| m.name().starts_with("tree:")).unwrap_or(models.len());

        progress("Preparing programs");
        let defines = scene_defines(shadow_unit, extra);
        let opaque = [Blend::Opaque, Blend::Alpha];
        let wall_stream = [Stream { stride: 20, instanced: false, attrs: &WALL_ATTRS }];
        let solid_stream = [Stream { stride: 20, instanced: false, attrs: &SOLID_ATTRS }];
        let inst_streams = [Stream { stride: 20, instanced: false, attrs: &SOLID_ATTRS }, Stream { stride: 52, instanced: true, attrs: &INST_ATTRS }];
        let wall = Pass::new(gpu.program("wall", &defines, WALL_V, WALL_F, &wall_stream, msaa, opaque)?, ["uFacade", ""]);
        let wall_night = Pass::new(gpu.program("wall_night", &format!("{defines}#define NIGHT\n"), WALL_V, WALL_F, &wall_stream, msaa, opaque)?, ["uFacade", "uNight"]);
        let solid = Pass::new(gpu.program("solid", &defines, SOLID_V, SOLID_F, &solid_stream, msaa, opaque)?, ["", ""]);
        let open = Pass::new(gpu.program("open", &format!("{defines}#define OPEN\n"), SOLID_V, SOLID_F, &solid_stream, msaa, opaque)?, ["", ""]);
        let inst = Pass::new(gpu.program("inst", &defines, INST_V, SOLID_F, &inst_streams, msaa, opaque)?, ["", ""]);
        let inst_open = Pass::new(gpu.program("inst_open", &format!("{defines}#define OPEN\n"), INST_V, SOLID_F, &inst_streams, msaa, opaque)?, ["", ""]);
        let leaf_streams = [Stream { stride: 20, instanced: false, attrs: &LEAF_ATTRS }, Stream { stride: 52, instanced: true, attrs: &INST_ATTRS }];
        let leaf = Pass::new(gpu.program("leaf", &defines, LEAF_V, LEAF_F, &leaf_streams, msaa, opaque)?, ["uLeaves", ""]);
        let night = format!("{defines}#define NIGHT\n");
        let night_open = format!("{defines}#define NIGHT\n#define OPEN\n");
        let solid_night = Pass::new(gpu.program("solid_night", &night, SOLID_V, SOLID_F, &solid_stream, msaa, opaque)?, ["", ""]);
        let open_night = Pass::new(gpu.program("open_night", &night_open, SOLID_V, SOLID_F, &solid_stream, msaa, opaque)?, ["", ""]);
        let inst_night = Pass::new(gpu.program("inst_night", &night, INST_V, SOLID_F, &inst_streams, msaa, opaque)?, ["", ""]);
        let inst_open_night = Pass::new(gpu.program("inst_open_night", &night_open, INST_V, SOLID_F, &inst_streams, msaa, opaque)?, ["", ""]);
        let leaf_night = Pass::new(gpu.program("leaf_night", &night, LEAF_V, LEAF_F, &leaf_streams, msaa, opaque)?, ["uLeaves", ""]);
        let boxed = Pass::new(gpu.program("boxed", &defines, BOX_V, BOX_F, &inst_streams, msaa, opaque)?, ["uPicture", "uNight"]);
        let boxed_night = Pass::new(gpu.program("boxed_night", &night, BOX_V, BOX_F, &inst_streams, msaa, opaque)?, ["uPicture", "uNight"]);
        const SUN_ATTRS: [(&str, u16, u32, u8); 1] = [("aPosition", 0, U16N, 3)];
        const SUN_LEAF_ATTRS: [(&str, u16, u32, u8); 3] = [("aPosition", 0, U16N, 3), ("aUv", 10, S16N, 2), ("aBark", 14, U8N, 4)];
        let sun_one = [Stream { stride: 20, instanced: false, attrs: &SUN_ATTRS }];
        let sun_many = [Stream { stride: 20, instanced: false, attrs: &SUN_ATTRS }, Stream { stride: 52, instanced: true, attrs: &INST_ATTRS[..3] }];
        let sun_cards = [Stream { stride: 20, instanced: false, attrs: &SUN_LEAF_ATTRS }, Stream { stride: 52, instanced: true, attrs: &INST_ATTRS[..3] }];
        let blended = [Blend::Alpha, Blend::Alpha];
        let veil = Pass::new(gpu.program("veil", &format!("{defines}#define VEIL\n"), INST_V, SOLID_F, &inst_streams, msaa, blended)?, ["", ""]);
        let veil_night = Pass::new(gpu.program("veil_night", &format!("{defines}#define VEIL\n#define NIGHT\n"), INST_V, SOLID_F, &inst_streams, msaa, blended)?, ["", ""]);
        let sun_solid = Pass::new(gpu.program("sun", &defines, SUN_V, SUN_F, &sun_one, 0, opaque)?, ["", ""]);
        let sun_inst = Pass::new(gpu.program("sun_inst", &format!("{defines}#define INSTANCED\n"), SUN_V, SUN_F, &sun_many, 0, opaque)?, ["", ""]);
        let sun_leaf = Pass::new(gpu.program("sun_leaf", &format!("{defines}#define INSTANCED\n#define LEAF\n"), SUN_V, SUN_F, &sun_cards, 0, opaque)?, ["uLeaves", ""]);
        let picture_stream = [Stream { stride: 20, instanced: false, attrs: &PICTURE_ATTRS }];
        let picture_streams = [Stream { stride: 20, instanced: false, attrs: &PICTURE_ATTRS }, Stream { stride: 52, instanced: true, attrs: &INST_ATTRS[..3] }];
        let picture = Pass::new(gpu.program("picture", &defines, PICTURE_V, PICTURE_F, &picture_stream, msaa, opaque)?, ["uPicture", ""]);
        let picture_inst = Pass::new(gpu.program("picture_inst", &format!("{defines}#define INSTANCED\n"), PICTURE_V, PICTURE_F, &picture_streams, msaa, opaque)?, ["uPicture", ""]);
        let ground = Pass::new(gpu.program("ground", &defines, GROUND_V, GROUND_F, &[Stream { stride: 16, instanced: false, attrs: &GROUND_ATTRS }], msaa, opaque)?, ["uGround", ""]);
        let ground_night = Pass::new(gpu.program("ground_night", &format!("{defines}#define NIGHT\n"), GROUND_V, GROUND_F, &[Stream { stride: 16, instanced: false, attrs: &GROUND_ATTRS }], msaa, opaque)?, ["uGround", ""]);

        progress("Reading the city's geometry");
        let tags = [pack::VSOL, pack::VWAL, pack::VGND, pack::IDX0, pack::INST];
        let sizes: Vec<usize> = tags.iter().map(|&t| p.size(t).unwrap_or(0)).collect();
        let pad = |n: usize| (n + 63) & !63;
        let total: usize = sizes.iter().map(|&n| pad(n.max(64))).sum();
        let mut memory = Memory::with_access(Kind::Main, total, false)?;
        let mut at = [core::ptr::null_mut::<u8>(); 5];
        let mut read_so_far = 0usize;
        for (k, &tag) in tags.iter().enumerate() {
            at[k] = memory.alloc(pad(sizes[k].max(64)), 64).ok_or("geometry block")?;
            let mut done = 0;
            while done < sizes[k] {
                let n = (sizes[k] - done).min(4 << 20);
                p.read_into(tag, done, at[k].add(done), n)?;
                done += n;
                read_so_far += n;
                progress(&format!("Reading the city's geometry: {:.0} of {:.0} MB", read_so_far as f32 / 1e6, total as f32 / 1e6));
            }
        }
        let own = Own { solids: at[0], walls: at[1], grounds: at[2], indices: at[3].cast(), instances: at[4] };

        progress("Uploading the facades");
        let (td_off, td_size) = p.range(pack::TDAT)?;
        let _ = td_off;
        let mut up = Uploader::new(4 * 1024 * 1024)?;
        let mut texture_bytes = 0usize;
        let mut textures: Vec<Option<Texture>> = Vec::with_capacity(texs.len());
        for (i, e) in texs.iter().enumerate() {
            if e.offset as usize + e.size as usize > td_size {
                return Err("a picture past the end of TDAT".into());
            }
            let mut bytes = vec![0u8; e.size as usize];
            p.read_into(pack::TDAT, e.offset as usize, bytes.as_mut_ptr(), bytes.len())?;
            let format = if e.format == pack::tex_format::BC3 { Format::Bc3 } else { Format::Bc1 };
            let mut t = up.texture(vram, format, e.width, e.height, e.mips, &bytes)?;
            up.flush();
            t.set_wrap(Wrap::Repeat, Wrap::Repeat);
            t.set_filter(true, true);
            texture_bytes += bytes.len();
            textures.push(Some(t));
            if i % 16 == 0 {
                progress(&format!("Uploading the facades: {} of {}", i + 1, texs.len()));
            }
        }
        up.free();

        progress("Laying the lamps' light");
        let lamp_bytes = p.read(pack::LAMP)?;
        let head: pack::Heights = pack::read(&lamp_bytes, 0).ok_or("lamp record")?;
        let texels = (head.w * head.h) as usize * 2;
        if lamp_bytes.len() < 32 + texels {
            return Err("the lamps' light is short".into());
        }
        let lamp_at = vram.alloc(texels, 512)?;
        core::ptr::copy_nonoverlapping(lamp_bytes[32..].as_ptr(), lamp_at, texels);
        let mut lamp: g::SceGxmTexture = core::mem::zeroed();
        if g::sceGxmTextureInitLinear(&mut lamp, lamp_at.cast(), g::SceGxmTextureFormat_SCE_GXM_TEXTURE_FORMAT_U5U6U5_RGB, head.w, head.h, 1) < 0 {
            return Err("lamp light texture".into());
        }
        g::sceGxmTextureSetMinFilter(&mut lamp, g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_LINEAR);
        g::sceGxmTextureSetMagFilter(&mut lamp, g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_LINEAR);
        g::sceGxmTextureSetUAddrMode(&mut lamp, g::SceGxmTextureAddrMode_SCE_GXM_TEXTURE_ADDR_CLAMP);
        g::sceGxmTextureSetVAddrMode(&mut lamp, g::SceGxmTextureAddrMode_SCE_GXM_TEXTURE_ADDR_CLAMP);
        let lamp_map = ny_frame::lamps::mapping(&head);
        texture_bytes += texels;
        // The lamps' light around a point, as the street and its storefronts hand it on (about 80 m).
        let (cw, ch, coarse) = ny_frame::lamps::around(&head, &lamp_bytes[32..]);
        let around_at = vram.alloc((cw * ch * 2) as usize, 512)?.cast::<u16>();
        core::ptr::copy_nonoverlapping(coarse.as_ptr(), around_at, coarse.len());
        let mut around: g::SceGxmTexture = core::mem::zeroed();
        if g::sceGxmTextureInitLinear(&mut around, around_at.cast(), g::SceGxmTextureFormat_SCE_GXM_TEXTURE_FORMAT_U5U6U5_RGB, cw, ch, 1) < 0 {
            return Err("lamp light around texture".into());
        }
        g::sceGxmTextureSetMinFilter(&mut around, g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_LINEAR);
        g::sceGxmTextureSetMagFilter(&mut around, g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_LINEAR);
        g::sceGxmTextureSetUAddrMode(&mut around, g::SceGxmTextureAddrMode_SCE_GXM_TEXTURE_ADDR_CLAMP);
        g::sceGxmTextureSetVAddrMode(&mut around, g::SceGxmTextureAddrMode_SCE_GXM_TEXTURE_ADDR_CLAMP);
        texture_bytes += (cw * ch * 2) as usize;
        drop(lamp_bytes);

        let largest = near_cells.iter().map(|c| c.size).max().unwrap_or(0) as usize;
        let (near_off, _) = p.range(pack::NEAR)?;
        let near = Near::new(&p.path, near_off as u64, largest.max(4096), NEAR_SLOTS)?;
        Ok(CityGpu {
            city,
            cells,
            blocks,
            regions,
            batches,
            spans,
            near_cells,
            near,
            geometry_bytes: memory.size(),
            _memory: memory,
            own,
            texs,
            textures,
            wall,
            wall_night,
            solid,
            open,
            inst,
            inst_open,
            ground,
            ground_night,
            leaf,
            solid_night,
            open_night,
            inst_night,
            inst_open_night,
            leaf_night,
            boxed,
            boxed_night,
            sun_solid,
            sun_inst,
            sun_leaf,
            veil,
            veil_night,
            picture,
            picture_inst,
            lamp,
            around,
            lamp_map,
            lamp_unit: head.unit,
            items: Vec::with_capacity(4096),
            wanted: Vec::with_capacity(256),
            models,
            ring: Ring::new(MODEL_INSTANCES * 52 + 4096, 2)?,
            first_tree,
            cut: [0.0, 0.0, 0.0, -1.0],
            texture_bytes,
        })
    }

    /// Draws the city from `eye`.
    ///
    /// # Safety
    /// Inside a scene on `ctx`.
    pub unsafe fn draw(&mut self, ctx: *mut g::SceGxmContext, vp: &Mat4, eye: V3, frame: &Frame, look: &[f32; 12], haze: &Haze, night: bool, shadow: &g::SceGxmTexture, sun: &SunMap, horizon: &ny_frame::horizon::Horizon, show: &Show, frame_no: u32) -> Stats {
        let planes = mat::planes(vp);
        let mut stats = Stats::default();
        let mut items = core::mem::take(&mut self.items);
        let mut wanted = core::mem::take(&mut self.wanted);
        items.clear();
        wanted.clear();
        self.near.receive();
        let tables = Tables { city: &self.city, cells: &self.cells, blocks: &self.blocks, regions: &self.regions, batches: &self.batches, spans: &self.spans };
        let near = &self.near;
        let counts = ny_view::select(&tables, &planes, [eye.x, eye.y, eye.z], &show.reach, &|c| near.ready(c).is_some(), &mut |c, d| wanted.push((c, d)), &|lo, hi| horizon.sees(lo, hi), &mut items);
        stats.places = counts.places;
        self.near.request(&mut wanted, &self.near_cells, frame_no);
        for it in &items {
            if it.cell != RESIDENT {
                self.near.touch(it.cell, frame_no);
            }
        }
        // By program, then by picture.
        let batches = &self.batches;
        items.sort_unstable_by_key(|it| {
            let b = &batches[it.batch as usize];
            (b.kind, b.tex)
        });

        let mut current: Option<(u32, u32)> = None;
        for it in &items {
            let b = &self.batches[it.batch as usize];
            let on = match b.kind {
                kind::WALL => show.walls,
                kind::SOLID | kind::OPEN => show.solids,
                kind::SOLID_INST | kind::OPEN_INST => show.instances,
                kind::LEAF_INST => show.instances && {
                    let c = &self.cells[it.cell.min(self.cells.len() as u32 - 1) as usize];
                    let (dx, dz) = ((c.min[0] + c.max[0]) * 0.5 - eye.x, (c.min[2] + c.max[2]) * 0.5 - eye.z);
                    it.cell != RESIDENT && dx * dx + dz * dz < show.trees * show.trees
                },
                kind::GROUND | kind::PICTURE => show.solids,
                kind::PICTURE_INST => show.instances,
                _ => false,
            };
            if !on {
                continue;
            }
            // The streams this item reads: the pack's own, or its cell's record.
            let (solids, walls, grounds, indices, instances) = if it.cell == RESIDENT {
                (self.own.solids, self.own.walls, self.own.grounds, self.own.indices, self.own.instances)
            } else {
                let Some(base) = self.near.ready(it.cell) else { continue };
                let s = self.near_cells[it.cell as usize].starts();
                (base.add(s[0]), base.add(s[1]), base.add(s[2]), base.add(s[3]).cast::<u16>(), base.add(s[4]))
            };
            let pass = match b.kind {
                kind::WALL => {
                    if night {
                        &self.wall_night
                    } else {
                        &self.wall
                    }
                }
                kind::SOLID => if night { &self.solid_night } else { &self.solid },
                kind::OPEN => if night { &self.open_night } else { &self.open },
                kind::SOLID_INST => if night { &self.inst_night } else { &self.inst },
                kind::GROUND => {
                    if night {
                        &self.ground_night
                    } else {
                        &self.ground
                    }
                }
                kind::LEAF_INST => if night { &self.leaf_night } else { &self.leaf },
                kind::PICTURE => &self.picture,
                kind::PICTURE_INST => &self.picture_inst,
                _ => if night { &self.inst_open_night } else { &self.inst_open },
            };
            if current.map(|c| c.0) != Some(b.kind) {
                pass.bind(ctx, shadow, &self.lamp, &self.around, sun.texture());
                gpu::state_opaque(ctx, if matches!(b.kind, kind::OPEN | kind::OPEN_INST | kind::LEAF_INST | kind::PICTURE | kind::PICTURE_INST) { Cull::None } else { Cull::Cw });
                current = Some((b.kind, NO_TEX));
            }
            if matches!(b.kind, kind::LEAF_INST | kind::PICTURE | kind::PICTURE_INST) && current.map(|c| c.1) != Some(b.tex) {
                if let Some(Some(t)) = self.textures.get(b.tex as usize) {
                    g::sceGxmSetFragmentTexture(ctx, pass.units[0], &t.gxm);
                }
                current = Some((b.kind, b.tex));
            }
            if b.kind == kind::GROUND && current.map(|c| c.1) != Some(b.tex) {
                if let Some(Some(t)) = self.textures.get(b.tex as usize) {
                    g::sceGxmSetFragmentTexture(ctx, pass.units[0], &t.gxm);
                }
                current = Some((b.kind, b.tex));
            }
            if b.kind == kind::WALL && current.map(|c| c.1) != Some(b.tex) {
                if let Some(Some(t)) = self.textures.get(b.tex as usize) {
                    g::sceGxmSetFragmentTexture(ctx, pass.units[0], &t.gxm);
                }
                let night_tex = self.texs.get(b.tex as usize).map(|e| e.night).unwrap_or(NO_TEX);
                if night {
                    if let Some(Some(t)) = self.textures.get(night_tex as usize).or(self.textures.get(b.tex as usize)) {
                        g::sceGxmSetFragmentTexture(ctx, pass.units[1], &t.gxm);
                    }
                }
                current = Some((b.kind, b.tex));
            }
            let v = Uniforms::vertex(ctx);
            v.set(pass.lamp_map, &self.lamp_map);
            v.set(pass.sun_vp, &sun.read);
            if b.kind == kind::LEAF_INST {
                v.set(pass.cut, &self.cut);
            }
            if matches!(b.kind, kind::WALL | kind::GROUND | kind::PICTURE | kind::PICTURE_INST) {
                let g = self.texs.get(b.tex as usize).map(|e| e.gloss).unwrap_or([204, 0, 0, 0]);
                v.set(pass.gloss, &[g[0] as f32 / 255.0, g[1] as f32 / 255.0, 0.0, 0.0]);
            }
            let mut l = *look;
            if b.kind == kind::WALL {
                // The night page holds its emission over its scale; with no night page, nothing glows.
                let night_tex = self.texs.get(b.tex as usize).map(|e| e.night).unwrap_or(NO_TEX);
                let scale = self.texs.get(night_tex as usize).map(|e| e.metres).unwrap_or(0.0);
                l[7] = frame[11] * scale;
            } else if matches!(b.kind, kind::PICTURE | kind::PICTURE_INST) {
                l[7] = self.texs.get(b.tex as usize).map(|e| e.metres).unwrap_or(1.0);
            }
            // The haze in the draw's direction: its middle; an instanced draw's, its cell's (or ahead).
            let middle = if !kind::instanced(b.kind) {
                V3 { x: b.origin[0] + b.span[0] * 0.5, y: 0.0, z: b.origin[2] + b.span[2] * 0.5 }
            } else if it.cell != RESIDENT {
                let c = &self.cells[it.cell as usize];
                V3 { x: (c.min[0] + c.max[0]) * 0.5, y: 0.0, z: (c.min[2] + c.max[2]) * 0.5 }
            } else {
                haze.eye
            };
            let hc = haze.towards(middle);
            l[9..12].copy_from_slice(&hc);
            Uniforms::fragment(ctx).set(pass.look, &l);
            // The haze's density to the draw's middle height.
            let mut fr = *frame;
            fr[3] = haze.density_to(b.origin[1] + b.span[1] * 0.5);
            v.set(pass.frame, &fr);
            if kind::instanced(b.kind) {
                v.set(pass.vp, vp);
                v.set(pass.bx, &[b.origin[0], b.origin[1], b.origin[2], 0.0, b.span[0], b.span[1], b.span[2], 0.0]);
                let shapes = if matches!(b.kind, kind::LEAF_INST | kind::PICTURE_INST) { self.own.walls } else { self.own.solids };
                g::sceGxmSetVertexStream(ctx, 0, shapes.add(b.vtx_first as usize * 20).cast());
                g::sceGxmSetVertexStream(ctx, 1, instances.add((b.inst_first + it.from) as usize * 52).cast());
                let n = it.to - it.from;
                g::sceGxmDrawInstanced(ctx, g::SceGxmPrimitiveType_SCE_GXM_PRIMITIVE_TRIANGLES, g::SceGxmIndexFormat_SCE_GXM_INDEX_FORMAT_U16, self.own.indices.add(b.idx_first as usize).cast(), b.idx_count * n, b.idx_count);
                stats.tris[2] += b.idx_count / 3 * n;
            } else {
                v.set(pass.mvp, &mat::with_bounds(vp, b.origin, b.span));
                v.set(pass.bx, &[b.origin[0], b.origin[1], b.origin[2], 0.0, b.span[0], b.span[1], b.span[2], 0.0]);
                let (vb, stride) = match b.kind {
                    kind::WALL | kind::PICTURE => (walls, 20),
                    kind::GROUND => {
                        let metres = self.texs.get(b.tex as usize).map(|e| e.metres).unwrap_or(16.0).max(0.5);
                        let k = 1.0 / metres;
                        let (ox, oz) = ((b.origin[0] * k).floor(), (b.origin[2] * k).floor());
                        v.set(pass.tile, &[k, -ox, -oz, 0.0]);
                        (grounds, 16)
                    }
                    _ => (solids, 20),
                };
                gpu::draw(ctx, vb.add(b.vtx_first as usize * stride), indices.add((b.idx_first + it.from) as usize), it.to - it.from);
                stats.tris[if b.kind == kind::WALL { 0 } else { 1 }] += (it.to - it.from) / 3;
            }
            stats.draws += 1;
        }
        self.items = items;
        self.wanted = wanted;
        stats
    }
}

impl CityGpu {
    /// The trees' finer levels among the pack's models.
    pub fn trees(&self) -> Trees {
        Trees::new(&self.models, &self.batches)
    }

    /// Places the trees nearest the eye at their finer levels (`ny_frame::trees`) onto `placed`, from the cells'
    /// records that are in; the cells' crowns of those trees are not drawn this frame.
    pub fn place_trees(&mut self, trees: &mut Trees, eye: V3, planes: &Planes, reach: &TreeReach, placed: &mut Vec<(usize, Instance)>) {
        self.near.receive();
        let (near, near_cells) = (&self.near, &self.near_cells);
        let record = |cell: u32| {
            let base = near.ready(cell)?;
            let c = &near_cells[cell as usize];
            // SAFETY: a ready record stays in its slot for the frames that draw from it.
            Some(unsafe { core::slice::from_raw_parts(base.add(c.starts()[4]), c.parts[4] as usize) })
        };
        let cut = trees.place(eye, planes, reach, &self.city, &self.cells, &self.batches, record, placed);
        self.cut = [eye.x, eye.y, eye.z, if cut > 0.0 { cut * cut } else { -1.0 }];
    }

    /// No tree drawn finer this frame: the cells draw every crown.
    pub fn no_trees(&mut self) {
        self.cut[3] = -1.0;
    }

    /// The distance within which the cells' crowns are not drawn this frame.
    pub fn cut_reach(&self) -> f32 {
        if self.cut[3] > 0.0 { libm::sqrtf(self.cut[3]) } else { 0.0 }
    }

    /// The model named `name`.
    pub fn model(&self, name: &str) -> Option<usize> {
        self.models.iter().position(|m| m.name() == name)
    }

    /// Writes this frame's instances of the models where the game puts them (`(model, instance)` pairs, any order) to
    /// the ring: one group per model, its instances' buffer and count, and where its first stands.
    pub unsafe fn prepare_models(&mut self, placed: &mut Vec<(usize, Instance)>) -> Vec<ModelGroup> {
        self.ring.next_frame();
        // (the trees first: a list too long loses what moves, not a tree whose crown the cells no longer draw)
        let first_tree = self.first_tree;
        placed.sort_by_key(|p| (p.0 < first_tree, p.0));
        placed.truncate(MODEL_INSTANCES);
        let mut out = Vec::new();
        let mut at = 0;
        while at < placed.len() {
            let m = placed[at].0;
            let end = at + placed[at..].iter().take_while(|p| p.0 == m).count();
            let n = end - at;
            let Some(buf) = self.ring.alloc(n * 52, 16) else { break };
            for (k, p) in placed[at..end].iter().enumerate() {
                core::ptr::write_unaligned(buf.add(k * 52).cast::<Instance>(), p.1);
            }
            let r = &placed[at].1.rows;
            let faded = placed[at..end].iter().any(|p| p.1.tint[3] < 255);
            out.push(ModelGroup { model: m, buf: buf.cast_const(), count: n as u32, at: V3 { x: r[0][3], y: r[1][3], z: r[2][3] }, faded });
            at = end;
        }
        out
    }

    /// Draws the models this frame placed (`prepare_models`).
    ///
    /// # Safety
    /// Inside a scene on `ctx`, after `draw` this frame.
    pub unsafe fn draw_models(&mut self, ctx: *mut g::SceGxmContext, vp: &Mat4, frame: &Frame, look: &[f32; 12], haze: &Haze, night: bool, shadow: &g::SceGxmTexture, sun: &SunMap, groups: &[ModelGroup]) -> u32 {
        let mut tris = 0;
        // The opaque parts, then what is seen through (blended over them, not hiding what is behind).
        for seen_through in [false, true] {
            for group in groups {
                let n = group.count;
                let model = self.models[group.model];
                for bi in model.first..model.first + model.count {
                    let b = self.batches[bi as usize];
                    if (b.veil != 0 || group.faded) != seen_through {
                        continue;
                    }
                    let boxed = b.kind == kind::BOXED_INST && !seen_through;
                    let leaf = b.kind == kind::LEAF_INST;
                    let pass = match (b.kind == kind::OPEN_INST, boxed, night) {
                        _ if leaf => if night { &self.leaf_night } else { &self.leaf },
                        _ if seen_through => if night { &self.veil_night } else { &self.veil },
                        (_, true, true) => &self.boxed_night,
                        (_, true, false) => &self.boxed,
                        (true, _, true) => &self.inst_open_night,
                        (true, _, false) => &self.inst_open,
                        (false, _, true) => &self.inst_night,
                        (false, _, false) => &self.inst,
                    };
                    pass.bind(ctx, shadow, &self.lamp, &self.around, sun.texture());
                    // A car's pictures, and what it emits by night over its glow picture's scale.
                    let mut glow = 0.0;
                    if boxed {
                        if let Some(Some(t)) = self.textures.get(b.tex as usize) {
                            g::sceGxmSetFragmentTexture(ctx, pass.units[0], &t.gxm);
                        }
                        let night_tex = self.texs.get(b.tex as usize).map(|e| e.night).unwrap_or(NO_TEX);
                        if let Some(Some(t)) = self.textures.get(night_tex as usize) {
                            g::sceGxmSetFragmentTexture(ctx, pass.units[1], &t.gxm);
                        }
                        glow = self.texs.get(night_tex as usize).map(|e| e.metres).unwrap_or(0.0) * frame[7];
                    }
                    if leaf {
                        if let Some(Some(t)) = self.textures.get(b.tex as usize) {
                            g::sceGxmSetFragmentTexture(ctx, pass.units[0], &t.gxm);
                        }
                    }
                    if seen_through {
                        gpu::state_overlay(ctx, true);
                        g::sceGxmSetCullMode(ctx, g::SceGxmCullMode_SCE_GXM_CULL_NONE);
                    } else {
                        gpu::state_opaque(ctx, if b.kind == kind::OPEN_INST || leaf { Cull::None } else { Cull::Cw });
                    }
                    let v = Uniforms::vertex(ctx);
                    v.set(pass.vp, vp);
                    v.set(pass.lamp_map, &self.lamp_map);
                    v.set(pass.sun_vp, &sun.read);
                    v.set(pass.bx, &[b.origin[0], b.origin[1], b.origin[2], 0.0, b.span[0], b.span[1], b.span[2], 0.0]);
                    v.set(pass.cut, &[0.0, 0.0, 0.0, -1.0]);
                    if boxed {
                        let (lo, size) = (model.box_min, model.box_size);
                        v.set(pass.bake, &[lo[0], lo[1], lo[2], 0.0, 1.0 / size[0].max(1e-3), 1.0 / size[1].max(1e-3), 1.0 / size[2].max(1e-3), 0.0]);
                    }
                    let mut l = *look;
                    l[7] = glow;
                    l[9..12].copy_from_slice(&haze.towards(group.at));
                    let f = Uniforms::fragment(ctx);
                    f.set(pass.look, &l);
                    f.set(pass.veil, &[if b.veil != 0 { b.veil as f32 / 255.0 } else { 1.0 }, 0.0, 0.0, 0.0]);
                    let mut fr = *frame;
                    fr[3] = haze.density_to(group.at.y);
                    v.set(pass.frame, &fr);
                    g::sceGxmSetVertexStream(ctx, 0, if leaf { self.own.walls } else { self.own.solids }.add(b.vtx_first as usize * 20).cast());
                    g::sceGxmSetVertexStream(ctx, 1, group.buf.cast());
                    g::sceGxmDrawInstanced(ctx, g::SceGxmPrimitiveType_SCE_GXM_PRIMITIVE_TRIANGLES, g::SceGxmIndexFormat_SCE_GXM_INDEX_FORMAT_U16, self.own.indices.add(b.idx_first as usize).cast(), b.idx_count * n, b.idx_count);
                    tris += b.idx_count / 3 * n;
                }
            }
        }
        tris
    }

    /// Draws into the sun's depth map (inside its scene) what casts shadows near the eye: the near and detail
    /// levels of the cells the map sees whose records are in, but their ground, and the models this frame placed.
    ///
    /// # Safety
    /// Inside the sun map's scene on `ctx`.
    pub unsafe fn draw_sun(&mut self, ctx: *mut g::SceGxmContext, sun: &SunMap, eye: V3, groups: &[ModelGroup]) -> u32 {
        let planes = mat::planes(&sun.draw);
        let c = &self.city;
        let reach = crate::sunmap::HALF + 140.0;
        let cell_of = |x: f32, z: f32| (((x - c.x0) / c.cell).floor() as i32, ((z - c.z0) / c.cell).floor() as i32);
        let (lo, hi) = (cell_of(eye.x - reach, eye.z - reach), cell_of(eye.x + reach, eye.z + reach));
        let mut tris = 0;
        let mut bound: Option<(u32, u32)> = None;
        for cz in lo.1.max(0)..=hi.1.min(c.cells_z as i32 - 1) {
            for cx in lo.0.max(0)..=hi.0.min(c.cells_x as i32 - 1) {
                let index = cz as u32 * c.cells_x + cx as u32;
                let cell = self.cells[index as usize];
                if cell.near.count + cell.detail.count == 0 || !ny_view::visible(&planes, cell.min, cell.max) {
                    continue;
                }
                let Some(base) = self.near.ready(index) else { continue };
                let s = self.near_cells[index as usize].starts();
                let (solids, walls, indices, instances) = (base.add(s[0]), base.add(s[1]), base.add(s[3]).cast::<u16>(), base.add(s[4]));
                for range in [cell.near, cell.detail] {
                    for bi in range.first..range.first + range.count {
                        let b = self.batches[bi as usize];
                        if b.kind == kind::GROUND {
                            continue;
                        }
                        let (pass, which) = match b.kind {
                            kind::LEAF_INST => (&self.sun_leaf, 2),
                            k if kind::instanced(k) => (&self.sun_inst, 1),
                            _ => (&self.sun_solid, 0),
                        };
                        if bound.map(|x| x.0) != Some(which) || (which == 2 && bound.map(|x| x.1) != Some(b.tex)) {
                            pass.prog.bind(ctx, false);
                            gpu::state_opaque(ctx, Cull::None);
                            if which == 2 {
                                if let Some(Some(t)) = self.textures.get(b.tex as usize) {
                                    g::sceGxmSetFragmentTexture(ctx, pass.units[0], &t.gxm);
                                }
                            }
                            bound = Some((which, b.tex));
                        }
                        let v = Uniforms::vertex(ctx);
                        v.set(pass.sun_vp, &sun.draw);
                        v.set(pass.bx, &[b.origin[0], b.origin[1], b.origin[2], 0.0, b.span[0], b.span[1], b.span[2], 0.0]);
                        if which == 2 {
                            v.set(pass.cut, &self.cut);
                        }
                        if which == 0 {
                            let vb = if matches!(b.kind, kind::WALL | kind::PICTURE) { walls } else { solids };
                            gpu::draw(ctx, vb.add(b.vtx_first as usize * 20), indices.add(b.idx_first as usize), b.idx_count);
                            tris += b.idx_count / 3;
                        } else {
                            let shapes = if matches!(b.kind, kind::LEAF_INST | kind::PICTURE_INST) { self.own.walls } else { self.own.solids };
                            g::sceGxmSetVertexStream(ctx, 0, shapes.add(b.vtx_first as usize * 20).cast());
                            g::sceGxmSetVertexStream(ctx, 1, instances.add(b.inst_first as usize * 52).cast());
                            g::sceGxmDrawInstanced(ctx, g::SceGxmPrimitiveType_SCE_GXM_PRIMITIVE_TRIANGLES, g::SceGxmIndexFormat_SCE_GXM_INDEX_FORMAT_U16, self.own.indices.add(b.idx_first as usize).cast(), b.idx_count * b.inst_count, b.idx_count);
                            tris += b.idx_count / 3 * b.inst_count;
                        }
                    }
                }
            }
        }
        // What moves, and the trees drawn finer.
        bound = None;
        for group in groups.iter().filter(|g| !g.faded) {
            let model = self.models[group.model];
            for bi in model.first..model.first + model.count {
                let b = self.batches[bi as usize];
                if b.veil != 0 {
                    continue;
                }
                let leaf = b.kind == kind::LEAF_INST;
                let pass = if leaf { &self.sun_leaf } else { &self.sun_inst };
                if bound.map(|x| x.0) != Some(leaf as u32) {
                    pass.prog.bind(ctx, false);
                    gpu::state_opaque(ctx, Cull::None);
                    if leaf {
                        if let Some(Some(t)) = self.textures.get(b.tex as usize) {
                            g::sceGxmSetFragmentTexture(ctx, pass.units[0], &t.gxm);
                        }
                    }
                    bound = Some((leaf as u32, b.tex));
                }
                let v = Uniforms::vertex(ctx);
                v.set(pass.sun_vp, &sun.draw);
                v.set(pass.bx, &[b.origin[0], b.origin[1], b.origin[2], 0.0, b.span[0], b.span[1], b.span[2], 0.0]);
                v.set(pass.cut, &[0.0, 0.0, 0.0, -1.0]);
                g::sceGxmSetVertexStream(ctx, 0, if leaf { self.own.walls } else { self.own.solids }.add(b.vtx_first as usize * 20).cast());
                g::sceGxmSetVertexStream(ctx, 1, group.buf.cast());
                g::sceGxmDrawInstanced(ctx, g::SceGxmPrimitiveType_SCE_GXM_PRIMITIVE_TRIANGLES, g::SceGxmIndexFormat_SCE_GXM_INDEX_FORMAT_U16, self.own.indices.add(b.idx_first as usize).cast(), b.idx_count * group.count, b.idx_count);
                tris += b.idx_count / 3 * group.count;
            }
        }
        tris
    }
}

/// One model's instances this frame: their buffer in the ring, how many, and where the first stands.
pub struct ModelGroup {
    pub model: usize,
    pub buf: *const u8,
    pub count: u32,
    pub at: V3,
    /// Whether an instance is seen through (its tint's alpha under 255): the group is drawn blended, each instance
    /// by its alpha, and casts no shadow.
    pub faded: bool,
}
