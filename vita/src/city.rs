//! The city on the GPU: the pack's tables in memory, its own streams (shapes, the mid and far levels) in one block
//! of GPU-mapped memory, the facade pages in video memory, the cells' records read as the eye moves (`near.rs`),
//! and a frame's draws, chosen by `ny_view::select` and grouped by program and picture.

use pocket_vita_gxm::mem::{Arena, Block as Memory, Kind};
use pocket_vita_gxm::program::{F32, S16N, S8N, U16N, U8N};
use pocket_vita_gxm::texture::{Format, Texture, Uploader, Wrap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use ny_pack::{self as pack, kind, Batch, Block, Cell, City, NearCell, Region, TexEntry, NO_TEX};
use ny_sim::math::V3;
use ny_view::{Item, Reach, Tables, RESIDENT};
use vita2d_sys as g;

use crate::gpu::{self, Blend, Cull, Gpu, Param, Program, Stream, Uniforms};
use crate::mat::{self, Mat4};
use crate::near::Near;

const FRAME: &str = include_str!("../shaders/frame.cg");
const WALL_V: &str = include_str!("../shaders/wall_v.cg");
const WALL_F: &str = include_str!("../shaders/wall_f.cg");
const SOLID_V: &str = include_str!("../shaders/solid_v.cg");
const SOLID_F: &str = include_str!("../shaders/solid_f.cg");
const INST_V: &str = include_str!("../shaders/inst_v.cg");

/// Light reaches the fragments as a colour, which holds 0..1: it is carried divided by this.
pub const LIGHT_SCALE: f32 = 2.0;
/// Slots for cells' records (each as large as the largest record).
const NEAR_SLOTS: usize = 72;

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
    vp: Param,
    bx: Param,
    frame: Param,
    look: Param,
    units: [u32; 2],
}

impl Pass {
    unsafe fn new(prog: Program, pictures: [&str; 2]) -> Pass {
        let units = pictures.map(|name| if name.is_empty() { 0 } else { prog.fs.sampler_index(name).unwrap_or(0) });
        Pass { mvp: prog.vs.param("uMvp"), vp: prog.vs.param("uVp"), bx: prog.vs.param("uBox"), frame: prog.vs.param("uFrame"), look: prog.fs.param("uLook"), units, prog }
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
    items: Vec<Item>,
    wanted: Vec<(u32, f32)>,
    pub geometry_bytes: usize,
    pub texture_bytes: usize,
}

/// What every scene program is compiled with.
pub fn scene_defines() -> String {
    format!("#define UV_RANGE {:.1}\n#define LIGHT_SCALE {LIGHT_SCALE:.1}\n#define GLOW_RANGE {:.1}\n{FRAME}", pack::UV_RANGE, pack::GLOW_RANGE)
}

const WALL_ATTRS: [(&str, u16, u32, u8); 5] = [("aPosition", 0, U16N, 3), ("aNormal", 6, S8N, 3), ("aAo", 9, U8N, 1), ("aUv", 10, S16N, 2), ("aTint", 14, U8N, 4)];
const SOLID_ATTRS: [(&str, u16, u32, u8); 5] = [("aPosition", 0, U16N, 3), ("aNormal", 6, S8N, 3), ("aAo", 9, U8N, 1), ("aColor", 10, U8N, 4), ("aGlow", 14, U8N, 4)];
const INST_ATTRS: [(&str, u16, u32, u8); 4] = [("aRow0", 0, F32, 4), ("aRow1", 16, F32, 4), ("aRow2", 32, F32, 4), ("aTint", 48, U8N, 4)];

impl CityGpu {
    /// # Safety
    /// GXM is initialized; `vram` outlives the city.
    pub unsafe fn load(p: &mut PackFile, gpu: &mut Gpu, vram: &mut Arena, msaa: u32, mut progress: impl FnMut(&str)) -> Result<CityGpu, String> {
        let city: City = pack::read(&p.read(pack::CITY)?, 0).ok_or("city record")?;
        let cells: Vec<Cell> = pack::table(&p.read(pack::CELL)?);
        let blocks: Vec<Block> = pack::table(&p.read(pack::BLCK)?);
        let regions: Vec<Region> = pack::table(&p.read(pack::REGN)?);
        let batches: Vec<Batch> = pack::table(&p.read(pack::BTCH)?);
        let spans: Vec<u32> = pack::table(&p.read(pack::SPAN)?);
        let near_cells: Vec<NearCell> = pack::table(&p.read(pack::NCEL)?);
        let texs: Vec<TexEntry> = pack::table(&p.read(pack::TEXS)?);

        progress("Preparing programs");
        let defines = scene_defines();
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
            let mut t = up.texture(vram, Format::Bc1, e.width, e.height, e.mips, &bytes)?;
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
            items: Vec::with_capacity(4096),
            wanted: Vec::with_capacity(256),
            texture_bytes,
        })
    }

    /// Draws the city from `eye`.
    ///
    /// # Safety
    /// Inside a scene on `ctx`.
    pub unsafe fn draw(&mut self, ctx: *mut g::SceGxmContext, vp: &Mat4, eye: V3, frame: &[f32; 16], look: &[f32; 8], night: bool, show: &Show, frame_no: u32) -> Stats {
        let planes = mat::planes(vp);
        let mut stats = Stats::default();
        let mut items = core::mem::take(&mut self.items);
        let mut wanted = core::mem::take(&mut self.wanted);
        items.clear();
        wanted.clear();
        self.near.receive();
        let tables = Tables { city: &self.city, cells: &self.cells, blocks: &self.blocks, regions: &self.regions, batches: &self.batches, spans: &self.spans };
        let near = &self.near;
        let counts = ny_view::select(&tables, &planes, [eye.x, eye.y, eye.z], &show.reach, &|c| near.ready(c).is_some(), &mut |c, d| wanted.push((c, d)), &mut items);
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
                _ => false,
            };
            if !on {
                continue;
            }
            // The streams this item reads: the pack's own, or its cell's record.
            let (solids, walls, indices, instances) = if it.cell == RESIDENT {
                (self.own.solids, self.own.walls, self.own.indices, self.own.instances)
            } else {
                let Some(base) = self.near.ready(it.cell) else { continue };
                let s = self.near_cells[it.cell as usize].starts();
                (base.add(s[0]), base.add(s[1]), base.add(s[3]).cast::<u16>(), base.add(s[4]))
            };
            let pass = match b.kind {
                kind::WALL => {
                    if night {
                        &self.wall_night
                    } else {
                        &self.wall
                    }
                }
                kind::SOLID => &self.solid,
                kind::OPEN => &self.open,
                kind::SOLID_INST => &self.inst,
                _ => &self.inst_open,
            };
            if current.map(|c| c.0) != Some(b.kind) {
                pass.prog.bind(ctx, false);
                gpu::state_opaque(ctx, if b.kind == kind::OPEN || b.kind == kind::OPEN_INST { Cull::None } else { Cull::Cw });
                current = Some((b.kind, NO_TEX));
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
            v.set(pass.frame, frame);
            let f = Uniforms::fragment(ctx);
            if b.kind == kind::WALL {
                // The night page holds its emission over its scale; with no night page, nothing glows.
                let night_tex = self.texs.get(b.tex as usize).map(|e| e.night).unwrap_or(NO_TEX);
                let scale = self.texs.get(night_tex as usize).map(|e| e.metres).unwrap_or(0.0);
                let mut l = *look;
                l[3] = frame[11] * scale;
                f.set(pass.look, &l);
            } else {
                f.set(pass.look, look);
            }
            if kind::instanced(b.kind) {
                v.set(pass.vp, vp);
                v.set(pass.bx, &[b.origin[0], b.origin[1], b.origin[2], 0.0, b.span[0], b.span[1], b.span[2], 0.0]);
                g::sceGxmSetVertexStream(ctx, 0, self.own.solids.add(b.vtx_first as usize * 20).cast());
                g::sceGxmSetVertexStream(ctx, 1, instances.add((b.inst_first + it.from) as usize * 52).cast());
                let n = it.to - it.from;
                g::sceGxmDrawInstanced(ctx, g::SceGxmPrimitiveType_SCE_GXM_PRIMITIVE_TRIANGLES, g::SceGxmIndexFormat_SCE_GXM_INDEX_FORMAT_U16, self.own.indices.add(b.idx_first as usize).cast(), b.idx_count * n, b.idx_count);
                stats.tris[2] += b.idx_count / 3 * n;
            } else {
                v.set(pass.mvp, &mat::with_bounds(vp, b.origin, b.span));
                let vb = if b.kind == kind::WALL { walls } else { solids };
                gpu::draw(ctx, vb.add(b.vtx_first as usize * 20), indices.add((b.idx_first + it.from) as usize), it.to - it.from);
                stats.tris[if b.kind == kind::WALL { 0 } else { 1 }] += (it.to - it.from) / 3;
            }
            stats.draws += 1;
        }
        self.items = items;
        self.wanted = wanted;
        stats
    }
}
