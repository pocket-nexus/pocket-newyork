//! The city on this GPU: the pack's tables in memory, its own streams (the shapes, the mid and far levels) in one
//! storage buffer, the cells' records of `NEAR` in slots of another as the eye moves (`ny_frame::near`), the
//! pictures as textures, and a frame's draws, chosen by `ny_view::select` and grouped by program and picture, as
//! `vita/src/city.rs` draws them.
//!
//! The programs read a vertex by its index from a storage buffer (`shaders/scene.wgsl`): a draw's uniforms name the
//! buffer and the word its records start at, and a cell's record is read where it went, so no record is laid out
//! again for the GPU.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use ny_frame::horizon::Horizon;
use ny_frame::light::{Frame, Haze, AROUND, LIGHT_SCALE, SHADOW_OUT, SHADOW_SOFT, SUN_BIAS};
use ny_frame::mat::{self, Mat4};
use ny_frame::near::Slots;
use ny_frame::sun::SunView;
use ny_pack::{self as pack, kind, Batch, Block, Cell, City, Heights, Instance, Model, NearCell, Region, TexEntry, NO_TEX};
use ny_sim::math::V3;
use ny_view::{Item, Reach, Tables, RESIDENT};
use pocket_web_wgpu::gpu::Gpu;
use pocket_web_wgpu::task;
use pocket_web_wgpu::wgpu::{self, util::DeviceExt, BindGroup, Buffer, RenderPipeline, Texture, TextureFormat, TextureView};

use crate::pack::Pack;

/// Samples a pixel of the scene (the PS Vita's display takes two; WebGPU offers one or four).
pub const SAMPLES: u32 = 4;
/// The scene's colour as the display shows it, and its depth: 1 at the near plane, 0 at the farthest.
pub const COLOUR: TextureFormat = TextureFormat::Rgba8Unorm;
pub const DEPTH: TextureFormat = TextureFormat::Depth32Float;
/// Texels along a side of the sun's map (the PS Vita's has 1 024 over the same square).
pub const SUN_SIDE: u32 = 2048;
/// Slots for cells' records (each as large as the largest record).
const NEAR_SLOTS: usize = 128;
/// Frames a slot stays untouched after its last draw before another record may go into it.
const HELD: u32 = 3;
/// Instances of models a frame may draw (every car of the traffic, and the player).
pub const MODEL_INSTANCES: usize = 1024;
/// Draws a frame may make, and the bytes between two draws' uniforms (WebGPU's offset alignment).
const DRAWS: usize = 8192;
const DRAW_STRIDE: u64 = 256;
/// Metres beyond the sun's map's square whose cells may still cast into it.
const SUN_MARGIN: f32 = 140.0;
/// The window of the height grid the shadows are swept over, in cells along a side.
pub use ny_frame::shadow::SIDE as SHADOW_SIDE;

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

#[derive(Clone, Copy, Default, Debug)]
pub struct Stats {
    pub draws: u32,
    /// Triangles: walls, solids, instances.
    pub tris: [u32; 3],
    /// Places drawn: near cells, mid blocks, far regions, detail cells.
    pub places: [u32; 4],
    /// Triangles into the sun's map.
    pub sun: u32,
}

/// The frame's uniforms (`Frame` in scene.wgsl), 352 bytes.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FrameU {
    pub vp: Mat4,
    pub sun_read: Mat4,
    pub sun_draw: Mat4,
    pub f: Frame,
    pub tone: [f32; 4],
    pub sun: [f32; 4],
    pub lamps: [f32; 4],
    pub lamp_map: [f32; 4],
}

/// A draw's uniforms (`Draw` in scene.wgsl), 128 bytes.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawU {
    origin: [f32; 4],
    span: [f32; 4],
    haze: [f32; 4],
    gloss: [f32; 4],
    tile: [f32; 4],
    bake0: [f32; 4],
    bake1: [f32; 4],
    src: [u32; 4],
}

/// The buffers a vertex or an instance is read from (`word` in scene.wgsl).
const RESIDENT_BUFFER: u32 = 0;
const ARENA_BUFFER: u32 = 1;
const RING_BUFFER: u32 = 2;

/// The programs a draw takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Program {
    Wall,
    Solid,
    Open,
    Inst,
    InstOpen,
    Ground,
    Leaf,
    Picture,
    PictureInst,
    Boxed,
    Veil,
    SunSolid,
    SunInst,
    SunLeaf,
}

/// Where a draw's indices are: the pack's own, or a cell's record in the arena from this byte.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Indices {
    Own,
    Arena(u64),
}

/// One draw, as the pass will make it.
struct Command {
    program: Program,
    pictures: (u32, u32),
    indices: Indices,
    range: Range<u32>,
    base: i32,
    instances: Range<u32>,
    uniforms: u32,
}

/// One model's instances this frame: where they start in the ring, how many, and where the first stands.
pub struct ModelGroup {
    pub model: usize,
    first: u32,
    pub count: u32,
    pub at: V3,
    /// Whether an instance is seen through (its tint's alpha under 255): the group is drawn blended, each instance
    /// by its alpha, and casts no shadow.
    pub faded: bool,
}

/// What a frame's draws take from the frame.
pub struct Look<'a> {
    pub vp: &'a Mat4,
    pub eye: V3,
    pub frame: &'a Frame,
    pub look: &'a [f32; 12],
    pub haze: &'a Haze,
    pub night: bool,
    pub sun: &'a SunView,
    pub horizon: &'a Horizon,
    pub show: &'a Show,
    pub frame_no: u32,
}

/// The pack's own streams in one buffer: where each starts, in words.
#[derive(Clone, Copy, Default)]
struct Bases {
    solids: u32,
    walls: u32,
    grounds: u32,
    instances: u32,
}

pub struct CityGpu {
    pub city: City,
    cells: Vec<Cell>,
    blocks: Vec<Block>,
    regions: Vec<Region>,
    batches: Vec<Batch>,
    spans: Vec<u32>,
    near_cells: Vec<NearCell>,
    texs: Vec<TexEntry>,
    pub models: Vec<Model>,
    _resident: Buffer,
    bases: Bases,
    indices: Buffer,
    arena: Buffer,
    slot_bytes: u64,
    pub slots: Slots,
    near_offset: u64,
    /// Records read and not yet in the arena: the cell, its slot, and its bytes.
    arrived: Rc<RefCell<Vec<(u32, usize, Result<Vec<u8>, String>)>>>,
    pack: Pack,
    ring: Buffer,
    pictures: Vec<Option<TextureView>>,
    picture_layout: wgpu::BindGroupLayout,
    picture_groups: HashMap<(u32, u32), BindGroup>,
    plain: BindGroup,
    repeat: wgpu::Sampler,
    pub lamp_map: [f32; 4],
    /// The light a channel's full value stands for.
    pub lamp_unit: f32,
    /// The shadow window's heights as swept (`shadow.rs` writes them).
    pub heights: Texture,
    pub sun_map: TextureView,
    frame_buffer: Buffer,
    draw_buffer: Buffer,
    frame_main: BindGroup,
    frame_sun: BindGroup,
    draw_group: BindGroup,
    programs: HashMap<&'static str, RenderPipeline>,
    items: Vec<Item>,
    wanted: Vec<(u32, f32)>,
    /// Cells the last frame wanted whose records were not in.
    pub wanting: usize,
    uniforms: Vec<DrawU>,
    commands: Vec<Command>,
    sun_commands: Vec<Command>,
    pub geometry_bytes: u64,
    pub texture_bytes: u64,
}

fn entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty, count: None }
}

fn texture_entry(binding: u32, sample_type: wgpu::TextureSampleType) -> wgpu::BindGroupLayoutEntry {
    entry(binding, wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false })
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    entry(binding, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None })
}

/// Bytes rounded up to a multiple of `k`.
fn up(n: u64, k: u64) -> u64 {
    n.div_ceil(k) * k
}

/// A picture of the pack as a texture with every level it stores: its blocks as they are where the GPU reads them,
/// else taken apart into rows of RGBA.
fn picture(gpu: &Gpu, e: &TexEntry, bytes: &[u8], blocks: bool) -> Texture {
    let bc3 = e.format == pack::tex_format::BC3;
    let format = match (blocks, bc3) {
        (true, true) => TextureFormat::Bc3RgbaUnorm,
        (true, false) => TextureFormat::Bc1RgbaUnorm,
        _ => TextureFormat::Rgba8Unorm,
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("picture"),
        size: wgpu::Extent3d { width: e.width, height: e.height, depth_or_array_layers: 1 },
        mip_level_count: e.mips.max(1),
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut at = 0usize;
    let mut rgba = Vec::new();
    for level in 0..e.mips.max(1) {
        let (w, h) = ((e.width >> level).max(1), (e.height >> level).max(1));
        let n = pack::tex_format::level_bytes(e.format, w, h);
        let Some(data) = bytes.get(at..at + n) else { break };
        at += n;
        let place = wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All };
        let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
        if blocks {
            let per = if bc3 { 16 } else { 8 };
            gpu.queue.write_texture(place, data, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bw * per), rows_per_image: Some(bh) }, wgpu::Extent3d { width: bw * 4, height: bh * 4, depth_or_array_layers: 1 });
        } else {
            crate::blocks::decode(bc3, data, w, h, &mut rgba);
            gpu.queue.write_texture(place, &rgba, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) }, wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 });
        }
    }
    texture
}

/// A texture of `r5 g6 b5` texels (the lamps' light), as RGBA rows, filtered.
fn texture_565(gpu: &Gpu, label: &str, w: u32, h: u32, texels: &[u16]) -> Texture {
    let rgba: Vec<u8> = texels.iter().flat_map(|&t| pocket_web_wgpu::picture::rgba_of(t)).collect();
    gpu.device.create_texture_with_data(
        &gpu.queue,
        &wgpu::TextureDescriptor { label: Some(label), size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] },
        wgpu::util::TextureDataOrder::LayerMajor,
        &rgba,
    )
}

/// What every scene program is compiled with: the constants the PS Vita's programs are given as defines
/// (`scene_defines` of `vita/src/city.rs`).
fn prelude() -> String {
    format!(
        "const UV_RANGE: f32 = {:?};\nconst LIGHT_SCALE: f32 = {LIGHT_SCALE:?};\nconst GLOW_RANGE: f32 = {:?};\nconst SHADOW_K: f32 = {:?};\nconst SHADOW_OUT: f32 = {SHADOW_OUT:?};\nconst AROUND: f32 = {AROUND:?};\nconst SUN_BIAS: f32 = {SUN_BIAS:?};\n",
        pack::UV_RANGE,
        pack::GLOW_RANGE,
        1.0 / SHADOW_SOFT
    )
}

impl CityGpu {
    /// Reads the city from `pack` onto the GPU. `say` hears what is being read.
    pub async fn load(gpu: &Gpu, pack: &Pack, say: &dyn Fn(String)) -> Result<CityGpu, String> {
        let device = &gpu.device;
        say("Reading the city".into());
        let mut tables = pack.read_run(&[pack::CITY, pack::CELL, pack::BLCK, pack::REGN]).await?;
        let mut take = |t: u32| tables.remove(&t).ok_or_else(|| format!("the pack has no {}", String::from_utf8_lossy(&t.to_le_bytes())));
        let city: City = pack::read(&take(pack::CITY)?, 0).ok_or("city record")?;
        let cells: Vec<Cell> = pack::table(&take(pack::CELL)?);
        let blocks: Vec<Block> = pack::table(&take(pack::BLCK)?);
        let regions: Vec<Region> = pack::table(&take(pack::REGN)?);
        say("Reading the city's batches".into());
        let batches: Vec<Batch> = pack::table(&pack.read(pack::BTCH).await?);
        let mut small = pack.read_run(&[pack::NCEL]).await?;
        small.extend(pack.read_run(&[pack::SPAN, pack::PRST, pack::SKYP, pack::MODL]).await?);
        small.extend(pack.read_run(&[pack::TEXS]).await?);
        let near_cells: Vec<NearCell> = pack::table(small.get(&pack::NCEL).ok_or("NCEL")?);
        let spans: Vec<u32> = pack::table(small.get(&pack::SPAN).ok_or("SPAN")?);
        let texs: Vec<TexEntry> = pack::table(small.get(&pack::TEXS).ok_or("TEXS")?);
        let models: Vec<Model> = pack::table(small.get(&pack::MODL).ok_or("MODL")?);

        // The pack's own streams, one after another in one buffer.
        let tags = [pack::VSOL, pack::VWAL, pack::VGND, pack::INST];
        let sizes: Vec<u64> = tags.iter().map(|&t| pack.range(t).map(|r| r.1 as u64).unwrap_or(0)).collect();
        let mut starts = [0u64; 4];
        let mut total = 0u64;
        for k in 0..4 {
            starts[k] = total;
            total += up(sizes[k].max(16), 16);
        }
        let idx_size = pack.range(pack::IDX0)?.1 as u64;
        let all = total + idx_size;
        let resident = device.create_buffer(&wgpu::BufferDescriptor { label: Some("the pack's streams"), size: total, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let indices = device.create_buffer(&wgpu::BufferDescriptor { label: Some("the pack's indices"), size: up(idx_size.max(4), 4), usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let mut done = 0u64;
        let chunk = 8usize << 20;
        for (k, &tag) in tags.iter().chain([&pack::IDX0]).enumerate() {
            let size = if k < 4 { sizes[k] } else { idx_size } as usize;
            let mut at = 0;
            while at < size {
                let n = (size - at).min(chunk);
                let mut bytes = pack.read_part(tag, at, n).await?;
                bytes.resize(up(n as u64, 4) as usize, 0);
                if k < 4 {
                    gpu.queue.write_buffer(&resident, starts[k] + at as u64, &bytes);
                } else {
                    gpu.queue.write_buffer(&indices, at as u64, &bytes);
                }
                at += n;
                done += n as u64;
                say(format!("Reading the city's geometry: {:.0} of {:.0} MB", done as f64 / 1e6, all as f64 / 1e6));
            }
        }
        let bases = Bases { solids: (starts[0] / 4) as u32, walls: (starts[1] / 4) as u32, grounds: (starts[2] / 4) as u32, instances: (starts[3] / 4) as u32 };

        // The pictures: the facades, the ground, the leaves, the signs, the cars.
        let blocks_ok = gpu.features.contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
        let (_, tdat_size) = pack.range(pack::TDAT)?;
        let tdat_mb = tdat_size as f64 / 1e6;
        let mut pictures = Vec::with_capacity(texs.len());
        let mut texture_bytes = 0u64;
        // (read in runs of pictures that lie together, a few megabytes at a time)
        let mut i = 0;
        while i < texs.len() {
            let first = texs[i].offset as usize;
            let mut j = i;
            let mut end = first;
            while j < texs.len() && (texs[j].offset as usize) == end && end - first < (6 << 20) {
                end += texs[j].size as usize;
                j += 1;
            }
            if j == i {
                j = i + 1;
                end = first + texs[i].size as usize;
            }
            if end > tdat_size {
                return Err("a picture past the end of TDAT".into());
            }
            let run = pack.read_part(pack::TDAT, first, end - first).await?;
            for e in &texs[i..j] {
                let at = e.offset as usize - first;
                let t = picture(gpu, e, &run[at..at + e.size as usize], blocks_ok);
                pictures.push(Some(t.create_view(&Default::default())));
                texture_bytes += e.size as u64;
            }
            i = j;
            say(format!("Reading the pictures: {:.0} of {:.0} MB", end as f64 / 1e6, tdat_mb));
        }

        say("Laying the lamps' light".into());
        let lamp_bytes = pack.read(pack::LAMP).await?;
        let head: Heights = pack::read(&lamp_bytes, 0).ok_or("lamp record")?;
        let texels = (head.w * head.h) as usize * 2;
        if lamp_bytes.len() < 32 + texels {
            return Err("the lamps' light is short".into());
        }
        let under: Vec<u16> = lamp_bytes[32..32 + texels].chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
        let lamp = texture_565(gpu, "the lamps' light", head.w, head.h, &under);
        let (cw, ch, around) = ny_frame::lamps::around(&head, &lamp_bytes[32..]);
        let around = texture_565(gpu, "the lamps' light around", cw, ch, &around);
        texture_bytes += (head.w * head.h * 4 + cw * ch * 4) as u64;

        // The cells' records: slots of the largest record's size.
        let largest = near_cells.iter().map(|c| c.size).max().unwrap_or(0) as u64;
        let slot_bytes = up(largest.max(4096), 256);
        let arena = device.create_buffer(&wgpu::BufferDescriptor { label: Some("the cells' records"), size: slot_bytes * NEAR_SLOTS as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let ring = device.create_buffer(&wgpu::BufferDescriptor { label: Some("this frame's models"), size: up((MODEL_INSTANCES * 52) as u64, 16), usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });

        let heights_texture = device.create_texture(&wgpu::TextureDescriptor { label: Some("the shadow window"), size: wgpu::Extent3d { width: SHADOW_SIDE as u32, height: SHADOW_SIDE as u32, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: TextureFormat::R32Float, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] });
        let depth = |label, side| device.create_texture(&wgpu::TextureDescriptor { label: Some(label), size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: DEPTH, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] });
        let sun_texture = depth("the sun's map", SUN_SIDE);
        let sun_map = sun_texture.create_view(&Default::default());
        let stand_in = depth("no sun's map", 1).create_view(&Default::default());

        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("frame"), size: std::mem::size_of::<FrameU>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let draw_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("draws"), size: DRAWS as u64 * DRAW_STRIDE, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let clamped = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("clamped"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let compare = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("sun"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, compare: Some(wgpu::CompareFunction::LessEqual), ..Default::default() });
        let repeat = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("repeat"), address_mode_u: wgpu::AddressMode::Repeat, address_mode_v: wgpu::AddressMode::Repeat, mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, mipmap_filter: wgpu::FilterMode::Linear, anisotropy_clamp: 4, ..Default::default() });

        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[
                entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
                texture_entry(1, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(2, wgpu::TextureSampleType::Depth),
                entry(3, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison)),
                texture_entry(4, wgpu::TextureSampleType::Float { filterable: true }),
                texture_entry(5, wgpu::TextureSampleType::Float { filterable: true }),
                entry(6, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                storage_entry(7),
                storage_entry(8),
                storage_entry(9),
            ],
        });
        let heights_view = heights_texture.create_view(&Default::default());
        let (lamp_view, around_view) = (lamp.create_view(&Default::default()), around.create_view(&Default::default()));
        let frame_group = |label, sun: &TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &frame_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: frame_buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&heights_view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(sun) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&compare) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&lamp_view) },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&around_view) },
                    wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&clamped) },
                    wgpu::BindGroupEntry { binding: 7, resource: resident.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 8, resource: arena.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 9, resource: ring.as_entire_binding() },
                ],
            })
        };
        let frame_main = frame_group("frame", &sun_map);
        let frame_sun = frame_group("frame, into the sun's map", &stand_in);
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("draw"),
            entries: &[entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<DrawU>() as u64) })],
        });
        let draw_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("draw"),
            layout: &draw_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &draw_buffer, offset: 0, size: wgpu::BufferSize::new(std::mem::size_of::<DrawU>() as u64) }) }],
        });
        let picture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pictures"),
            entries: &[texture_entry(0, wgpu::TextureSampleType::Float { filterable: true }), texture_entry(1, wgpu::TextureSampleType::Float { filterable: true }), entry(2, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering))],
        });
        let white = device.create_texture_with_data(&gpu.queue, &wgpu::TextureDescriptor { label: Some("white"), size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] }, wgpu::util::TextureDataOrder::LayerMajor, &[255, 255, 255, 255]);
        let white = white.create_view(&Default::default());
        let plain = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("no pictures"),
            layout: &picture_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&white) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&white) }, wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&repeat) }],
        });

        // The programs.
        say("Preparing programs".into());
        let source = format!("{}{}", prelude(), include_str!("shaders/scene.wgsl"));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("scene"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("scene"), bind_group_layouts: &[&frame_layout, &draw_layout, &picture_layout], push_constant_ranges: &[] });
        let mut programs = HashMap::new();
        let blend = wgpu::BlendState::ALPHA_BLENDING;
        let back = Some(wgpu::Face::Back);
        #[rustfmt::skip]
        let scene: [(&'static str, &str, &str, Option<wgpu::Face>, bool); 11] = [
            ("wall", "vs_wall", "fs_wall", back, false),
            ("solid", "vs_solid", "fs_solid", back, false),
            ("open", "vs_solid", "fs_solid", None, false),
            ("inst", "vs_inst", "fs_solid", back, false),
            ("inst_open", "vs_inst", "fs_solid", None, false),
            ("ground", "vs_ground", "fs_ground", back, false),
            ("leaf", "vs_leaf", "fs_leaf", None, false),
            ("picture", "vs_picture", "fs_picture", None, false),
            ("picture_inst", "vs_picture_inst", "fs_picture", None, false),
            ("boxed", "vs_box", "fs_box", back, false),
            ("veil", "vs_inst", "fs_veil", None, true),
        ];
        for (name, vs, fs, cull, seen_through) in scene {
            programs.insert(
                name,
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(name),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState { module: &module, entry_point: Some(vs), compilation_options: Default::default(), buffers: &[] },
                    fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some(fs), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format: COLOUR, blend: seen_through.then_some(blend), write_mask: wgpu::ColorWrites::ALL })] }),
                    primitive: wgpu::PrimitiveState { cull_mode: cull, front_face: wgpu::FrontFace::Ccw, ..Default::default() },
                    depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: !seen_through, depth_compare: wgpu::CompareFunction::Greater, stencil: Default::default(), bias: Default::default() }),
                    multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
                    multiview: None,
                    cache: None,
                }),
            );
        }
        let sun: [(&'static str, &str, Option<&str>); 3] = [("sun_solid", "vs_sun", None), ("sun_inst", "vs_sun_inst", None), ("sun_leaf", "vs_sun_leaf", Some("fs_sun_leaf"))];
        for (name, vs, fs) in sun {
            programs.insert(
                name,
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(name),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState { module: &module, entry_point: Some(vs), compilation_options: Default::default(), buffers: &[] },
                    fragment: fs.map(|fs| wgpu::FragmentState { module: &module, entry_point: Some(fs), compilation_options: Default::default(), targets: &[] }),
                    primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                    depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: true, depth_compare: wgpu::CompareFunction::Less, stencil: Default::default(), bias: Default::default() }),
                    multisample: Default::default(),
                    multiview: None,
                    cache: None,
                }),
            );
        }

        let geometry_bytes = total + idx_size + slot_bytes * NEAR_SLOTS as u64;
        let (near_offset, _) = pack.range(pack::NEAR)?;
        Ok(CityGpu {
            city,
            cells,
            blocks,
            regions,
            batches,
            spans,
            near_cells,
            texs,
            models,
            _resident: resident,
            bases,
            indices,
            arena,
            slot_bytes,
            slots: Slots::new(NEAR_SLOTS, HELD),
            near_offset: near_offset as u64,
            arrived: Rc::new(RefCell::new(Vec::new())),
            pack: pack.clone(),
            ring,
            pictures,
            picture_layout,
            picture_groups: HashMap::new(),
            plain,
            repeat,
            lamp_map: ny_frame::lamps::mapping(&head),
            lamp_unit: head.unit,
            heights: heights_texture,
            sun_map,
            frame_buffer,
            draw_buffer,
            frame_main,
            frame_sun,
            draw_group,
            programs,
            items: Vec::with_capacity(4096),
            wanted: Vec::with_capacity(256),
            wanting: 0,
            uniforms: Vec::with_capacity(DRAWS),
            commands: Vec::with_capacity(DRAWS),
            sun_commands: Vec::with_capacity(DRAWS),
            geometry_bytes,
            texture_bytes,
        })
    }

    /// The model named `name`.
    pub fn model(&self, name: &str) -> Option<usize> {
        self.models.iter().position(|m| m.name() == name)
    }

    /// The bind group of two pictures (`NO_TEX`: white).
    fn pictures(&mut self, gpu: &Gpu, a: u32, b: u32) -> (u32, u32) {
        if (a == NO_TEX && b == NO_TEX) || self.picture_groups.contains_key(&(a, b)) {
            return (a, b);
        }
        let view = |t: u32| self.pictures.get(t as usize).and_then(|v| v.as_ref());
        let (Some(va), Some(vb)) = (view(a).or(view(b)), view(b).or(view(a))) else { return (NO_TEX, NO_TEX) };
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pictures"),
            layout: &self.picture_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(va) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(vb) }, wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.repeat) }],
        });
        self.picture_groups.insert((a, b), group);
        (a, b)
    }

    /// Takes in the records that have arrived: into their slots in the arena.
    pub fn receive(&mut self, gpu: &Gpu) {
        let arrived: Vec<_> = self.arrived.borrow_mut().drain(..).collect();
        for (cell, slot, bytes) in arrived {
            match bytes {
                Ok(mut bytes) => {
                    bytes.resize(up(bytes.len() as u64, 4) as usize, 0);
                    gpu.queue.write_buffer(&self.arena, slot as u64 * self.slot_bytes, &bytes);
                    self.slots.arrived(cell, slot, true);
                }
                Err(_) => self.slots.arrived(cell, slot, false),
            }
        }
    }

    /// Starts the reads of the cells this frame wanted.
    fn request(&mut self, frame_no: u32) {
        let mut wanted = std::mem::take(&mut self.wanted);
        let (pack, at, arrived) = (self.pack.clone(), self.near_offset, self.arrived.clone());
        self.slots.request(&mut wanted, &self.near_cells, frame_no, |r| {
            let (source, arrived) = (pack.source.clone(), arrived.clone());
            task::spawn(async move {
                let bytes = source.range(at + r.offset, r.size as u64).await;
                arrived.borrow_mut().push((r.cell, r.slot, bytes));
            });
        });
        self.wanted = wanted;
    }

    /// A draw's uniforms, kept for this frame; its slot.
    fn uniforms(&mut self, u: DrawU) -> Option<u32> {
        if self.uniforms.len() >= DRAWS {
            return None;
        }
        self.uniforms.push(u);
        Some(self.uniforms.len() as u32 - 1)
    }

    /// Where the streams an item draws from are: the vertices' buffer and first word, the instances' too, and
    /// where its indices are.
    fn sources(&self, b: &Batch, cell: u32) -> Option<([u32; 4], Indices)> {
        let walls = matches!(b.kind, kind::WALL | kind::PICTURE | kind::LEAF_INST | kind::PICTURE_INST);
        let own = if b.kind == kind::GROUND { self.bases.grounds } else if walls { self.bases.walls } else { self.bases.solids };
        if cell == RESIDENT {
            return Some(([RESIDENT_BUFFER, own, RESIDENT_BUFFER, self.bases.instances], Indices::Own));
        }
        let slot = self.slots.ready(cell)? as u64;
        let s = self.near_cells[cell as usize].starts();
        let base = slot * self.slot_bytes;
        let word = |part: usize| ((base + s[part] as u64) / 4) as u32;
        if kind::instanced(b.kind) {
            // (an instanced batch's shape is in the pack's own streams; its instances are the cell's)
            Some(([RESIDENT_BUFFER, own, ARENA_BUFFER, word(4)], Indices::Own))
        } else {
            let part = if b.kind == kind::GROUND { 2 } else if walls { 1 } else { 0 };
            Some(([ARENA_BUFFER, word(part), ARENA_BUFFER, 0], Indices::Arena(base + s[3] as u64)))
        }
    }

    /// Chooses this frame's draws of the city and of what moves (`placed`), and the sun's (when its map is drawn),
    /// and writes their uniforms.
    pub fn prepare(&mut self, gpu: &Gpu, look: &Look, sun_drawn: bool, placed: &mut Vec<(usize, Instance)>) -> (Stats, Vec<ModelGroup>) {
        let mut stats = Stats::default();
        self.uniforms.clear();
        self.commands.clear();
        self.sun_commands.clear();
        self.receive(gpu);
        let planes = mat::planes(look.vp);
        let mut items = std::mem::take(&mut self.items);
        items.clear();
        self.wanted.clear();
        let tables = Tables { city: &self.city, cells: &self.cells, blocks: &self.blocks, regions: &self.regions, batches: &self.batches, spans: &self.spans };
        let slots = &self.slots;
        let wanted = &mut self.wanted;
        let eye = look.eye;
        let counts = ny_view::select(&tables, &planes, [eye.x, eye.y, eye.z], &look.show.reach, &|c| slots.ready(c).is_some(), &mut |c, d| wanted.push((c, d)), &|lo, hi| look.horizon.sees(lo, hi), &mut items);
        stats.places = counts.places;
        self.wanting = self.wanted.len();
        self.request(look.frame_no);
        for it in &items {
            if it.cell != RESIDENT {
                self.slots.touch(it.cell, look.frame_no);
            }
        }
        // By program, then by picture.
        let batches = &self.batches;
        items.sort_unstable_by_key(|it| {
            let b = &batches[it.batch as usize];
            (b.kind, b.tex)
        });
        let show = look.show;
        for it in &items {
            let b = self.batches[it.batch as usize];
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
            let Some((src, indices)) = self.sources(&b, it.cell) else { continue };
            let program = match b.kind {
                kind::WALL => Program::Wall,
                kind::SOLID => Program::Solid,
                kind::OPEN => Program::Open,
                kind::SOLID_INST => Program::Inst,
                kind::GROUND => Program::Ground,
                kind::LEAF_INST => Program::Leaf,
                kind::PICTURE => Program::Picture,
                kind::PICTURE_INST => Program::PictureInst,
                _ => Program::InstOpen,
            };
            let night_tex = self.texs.get(b.tex as usize).map(|e| e.night).unwrap_or(NO_TEX);
            let pictures = match b.kind {
                kind::WALL => {
                    let n = if look.night && self.pictures.get(night_tex as usize).is_some_and(|v| v.is_some()) { night_tex } else { b.tex };
                    self.pictures(gpu, b.tex, n)
                }
                kind::GROUND | kind::LEAF_INST | kind::PICTURE | kind::PICTURE_INST => self.pictures(gpu, b.tex, b.tex),
                _ => (NO_TEX, NO_TEX),
            };
            let mut u = DrawU { origin: [b.origin[0], b.origin[1], b.origin[2], 0.0], span: [b.span[0], b.span[1], b.span[2], 0.0], src, ..Default::default() };
            if matches!(b.kind, kind::WALL | kind::GROUND | kind::PICTURE | kind::PICTURE_INST) {
                let g = self.texs.get(b.tex as usize).map(|e| e.gloss).unwrap_or([204, 0, 0, 0]);
                u.gloss = [g[0] as f32 / 255.0, g[1] as f32 / 255.0, 0.0, 0.0];
            }
            if matches!(b.kind, kind::OPEN | kind::OPEN_INST) {
                u.gloss[2] = 1.0;
            }
            if b.kind == kind::WALL {
                // The night page holds its emission over its scale; with no night page, nothing glows.
                let scale = self.texs.get(night_tex as usize).map(|e| e.metres).unwrap_or(0.0);
                u.span[3] = look.frame[11] * scale;
            } else if matches!(b.kind, kind::PICTURE | kind::PICTURE_INST) {
                u.span[3] = self.texs.get(b.tex as usize).map(|e| e.metres).unwrap_or(1.0);
            }
            if b.kind == kind::GROUND {
                let metres = self.texs.get(b.tex as usize).map(|e| e.metres).unwrap_or(16.0).max(0.5);
                let k = 1.0 / metres;
                u.tile = [k, -(b.origin[0] * k).floor(), -(b.origin[2] * k).floor(), 0.0];
            }
            // The haze in the draw's direction: its middle; an instanced draw's, its cell's (or ahead).
            let middle = if !kind::instanced(b.kind) {
                V3 { x: b.origin[0] + b.span[0] * 0.5, y: 0.0, z: b.origin[2] + b.span[2] * 0.5 }
            } else if it.cell != RESIDENT {
                let c = &self.cells[it.cell as usize];
                V3 { x: (c.min[0] + c.max[0]) * 0.5, y: 0.0, z: (c.min[2] + c.max[2]) * 0.5 }
            } else {
                look.haze.eye
            };
            let hc = look.haze.towards(middle);
            u.haze = [hc[0], hc[1], hc[2], 1.0];
            u.origin[3] = look.haze.density_to(b.origin[1] + b.span[1] * 0.5);
            let Some(slot) = self.uniforms(u) else { break };
            let (range, instances) = if kind::instanced(b.kind) {
                stats.tris[2] += b.idx_count / 3 * (it.to - it.from);
                (b.idx_first..b.idx_first + b.idx_count, b.inst_first + it.from..b.inst_first + it.to)
            } else {
                stats.tris[if b.kind == kind::WALL { 0 } else { 1 }] += (it.to - it.from) / 3;
                (b.idx_first + it.from..b.idx_first + it.to, 0..1)
            };
            self.commands.push(Command { program, pictures, indices, range, base: b.vtx_first as i32, instances, uniforms: slot });
            stats.draws += 1;
        }
        self.items = items;

        // What moves: one group per model, its instances in this frame's ring.
        placed.sort_by_key(|p| p.0);
        placed.truncate(MODEL_INSTANCES);
        let mut ring = Vec::with_capacity(placed.len() * 52);
        let mut groups = Vec::new();
        let mut at = 0;
        while at < placed.len() {
            let m = placed[at].0;
            let end = at + placed[at..].iter().take_while(|p| p.0 == m).count();
            let r = &placed[at].1.rows;
            let faded = placed[at..end].iter().any(|p| p.1.tint[3] < 255);
            groups.push(ModelGroup { model: m, first: at as u32, count: (end - at) as u32, at: V3 { x: r[0][3], y: r[1][3], z: r[2][3] }, faded });
            for p in &placed[at..end] {
                ring.extend_from_slice(pack::bytes_of(&p.1));
            }
            at = end;
        }
        if !ring.is_empty() {
            ring.resize(up(ring.len() as u64, 4) as usize, 0);
            gpu.queue.write_buffer(&self.ring, 0, &ring);
        }
        // The opaque parts, then what is seen through (blended over them, not hiding what is behind).
        for seen_through in [false, true] {
            for g in &groups {
                let model = self.models[g.model];
                for bi in model.first..model.first + model.count {
                    let b = self.batches[bi as usize];
                    if (b.veil != 0 || g.faded) != seen_through {
                        continue;
                    }
                    let boxed = b.kind == kind::BOXED_INST && !seen_through;
                    let program = match (seen_through, boxed, b.kind == kind::OPEN_INST) {
                        (true, _, _) => Program::Veil,
                        (_, true, _) => Program::Boxed,
                        (_, _, true) => Program::InstOpen,
                        _ => Program::Inst,
                    };
                    let night_tex = self.texs.get(b.tex as usize).map(|e| e.night).unwrap_or(NO_TEX);
                    let pictures = if boxed { self.pictures(gpu, b.tex, night_tex) } else { (NO_TEX, NO_TEX) };
                    let mut u = DrawU { origin: [b.origin[0], b.origin[1], b.origin[2], look.haze.density_to(g.at.y)], span: [b.span[0], b.span[1], b.span[2], 0.0], src: [RESIDENT_BUFFER, self.bases.solids, RING_BUFFER, g.first * 13], ..Default::default() };
                    if boxed {
                        // A car's pictures, and what it emits by night over its glow picture's scale.
                        u.span[3] = self.texs.get(night_tex as usize).map(|e| e.metres).unwrap_or(0.0) * look.frame[7];
                        let (lo, size) = (model.box_min, model.box_size);
                        u.bake0 = [lo[0], lo[1], lo[2], 0.0];
                        u.bake1 = [1.0 / size[0].max(1e-3), 1.0 / size[1].max(1e-3), 1.0 / size[2].max(1e-3), 0.0];
                    }
                    if b.kind == kind::OPEN_INST {
                        u.gloss[2] = 1.0;
                    }
                    let hc = look.haze.towards(g.at);
                    u.haze = [hc[0], hc[1], hc[2], if b.veil != 0 { b.veil as f32 / 255.0 } else { 1.0 }];
                    let Some(slot) = self.uniforms(u) else { break };
                    self.commands.push(Command { program, pictures, indices: Indices::Own, range: b.idx_first..b.idx_first + b.idx_count, base: b.vtx_first as i32, instances: 0..g.count, uniforms: slot });
                    stats.tris[2] += b.idx_count / 3 * g.count;
                    stats.draws += 1;
                }
            }
        }
        if sun_drawn {
            stats.sun = self.prepare_sun(gpu, look.sun, eye, &groups);
        }
        gpu.queue.write_buffer(&self.draw_buffer, 0, &self.uniform_bytes());
        (stats, groups)
    }

    /// The uniforms of this frame's draws, each at its slot.
    fn uniform_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![0u8; self.uniforms.len().max(1) * DRAW_STRIDE as usize];
        for (k, u) in self.uniforms.iter().enumerate() {
            let at = k * DRAW_STRIDE as usize;
            bytes[at..at + std::mem::size_of::<DrawU>()].copy_from_slice(bytemuck::bytes_of(u));
        }
        bytes
    }

    /// Draws into the sun's map: the near and detail levels of the cells it sees whose records are in, but their
    /// ground, and the models this frame placed.
    fn prepare_sun(&mut self, gpu: &Gpu, sun: &SunView, eye: V3, groups: &[ModelGroup]) -> u32 {
        let planes = mat::planes(&sun.draw);
        let c = self.city;
        let reach = ny_frame::sun::HALF + SUN_MARGIN;
        let cell_of = |x: f32, z: f32| (((x - c.x0) / c.cell).floor() as i32, ((z - c.z0) / c.cell).floor() as i32);
        let (lo, hi) = (cell_of(eye.x - reach, eye.z - reach), cell_of(eye.x + reach, eye.z + reach));
        let mut tris = 0;
        for cz in lo.1.max(0)..=hi.1.min(c.cells_z as i32 - 1) {
            for cx in lo.0.max(0)..=hi.0.min(c.cells_x as i32 - 1) {
                let index = cz as u32 * c.cells_x + cx as u32;
                let cell = self.cells[index as usize];
                if cell.near.count + cell.detail.count == 0 || !ny_view::visible(&planes, cell.min, cell.max) || self.slots.ready(index).is_none() {
                    continue;
                }
                for range in [cell.near, cell.detail] {
                    for bi in range.first..range.first + range.count {
                        let b = self.batches[bi as usize];
                        if b.kind == kind::GROUND {
                            continue;
                        }
                        let Some((src, indices)) = self.sources(&b, index) else { continue };
                        let (program, pictures) = match b.kind {
                            kind::LEAF_INST => (Program::SunLeaf, self.pictures(gpu, b.tex, b.tex)),
                            k if kind::instanced(k) => (Program::SunInst, (NO_TEX, NO_TEX)),
                            _ => (Program::SunSolid, (NO_TEX, NO_TEX)),
                        };
                        let u = DrawU { origin: [b.origin[0], b.origin[1], b.origin[2], 0.0], span: [b.span[0], b.span[1], b.span[2], 0.0], src, ..Default::default() };
                        let Some(slot) = self.uniforms(u) else { return tris };
                        let (range, instances) = if kind::instanced(b.kind) { (b.idx_first..b.idx_first + b.idx_count, b.inst_first..b.inst_first + b.inst_count) } else { (b.idx_first..b.idx_first + b.idx_count, 0..1) };
                        tris += b.idx_count / 3 * (instances.end - instances.start);
                        self.sun_commands.push(Command { program, pictures, indices, range, base: b.vtx_first as i32, instances, uniforms: slot });
                    }
                }
            }
        }
        // What moves.
        for g in groups.iter().filter(|g| !g.faded) {
            let model = self.models[g.model];
            for bi in model.first..model.first + model.count {
                let b = self.batches[bi as usize];
                if b.veil != 0 {
                    continue;
                }
                let u = DrawU { origin: [b.origin[0], b.origin[1], b.origin[2], 0.0], span: [b.span[0], b.span[1], b.span[2], 0.0], src: [RESIDENT_BUFFER, self.bases.solids, RING_BUFFER, g.first * 13], ..Default::default() };
                let Some(slot) = self.uniforms(u) else { return tris };
                self.sun_commands.push(Command { program: Program::SunInst, pictures: (NO_TEX, NO_TEX), indices: Indices::Own, range: b.idx_first..b.idx_first + b.idx_count, base: b.vtx_first as i32, instances: 0..g.count, uniforms: slot });
                tris += b.idx_count / 3 * g.count;
            }
        }
        tris
    }

    /// Writes the frame's uniforms.
    pub fn frame_uniforms(&self, gpu: &Gpu, u: &FrameU) {
        gpu.queue.write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(u));
    }

    fn program_of(p: Program) -> &'static str {
        match p {
            Program::Wall => "wall",
            Program::Solid => "solid",
            Program::Open => "open",
            Program::Inst => "inst",
            Program::InstOpen => "inst_open",
            Program::Ground => "ground",
            Program::Leaf => "leaf",
            Program::Picture => "picture",
            Program::PictureInst => "picture_inst",
            Program::Boxed => "boxed",
            Program::Veil => "veil",
            Program::SunSolid => "sun_solid",
            Program::SunInst => "sun_inst",
            Program::SunLeaf => "sun_leaf",
        }
    }

    fn encode(&self, pass: &mut wgpu::RenderPass, commands: &[Command], frame: &BindGroup) {
        pass.set_bind_group(0, frame, &[]);
        let mut program = None;
        let mut pictures = None;
        let mut indices = None;
        for c in commands {
            if program != Some(c.program) {
                pass.set_pipeline(&self.programs[Self::program_of(c.program)]);
                program = Some(c.program);
            }
            if pictures != Some(c.pictures) {
                pass.set_bind_group(2, self.picture_groups.get(&c.pictures).unwrap_or(&self.plain), &[]);
                pictures = Some(c.pictures);
            }
            if indices != Some(c.indices) {
                match c.indices {
                    Indices::Own => pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16),
                    Indices::Arena(at) => pass.set_index_buffer(self.arena.slice(at..), wgpu::IndexFormat::Uint16),
                }
                indices = Some(c.indices);
            }
            pass.set_bind_group(1, &self.draw_group, &[c.uniforms * DRAW_STRIDE as u32]);
            pass.draw_indexed(c.range.clone(), c.base, c.instances.clone());
        }
    }

    /// Draws the city and what moves into the scene's pass.
    pub fn draw(&self, pass: &mut wgpu::RenderPass) {
        self.encode(pass, &self.commands, &self.frame_main);
    }

    /// Draws into the sun's map's pass.
    pub fn draw_sun(&self, pass: &mut wgpu::RenderPass) {
        self.encode(pass, &self.sun_commands, &self.frame_sun);
    }

    /// Requests made of the pack and bytes received since the start.
    pub fn read_so_far(&self) -> pocket_web_wgpu::source::Read {
        self.pack.source.read_so_far()
    }

    /// Whether this frame draws into the sun's map.
    pub fn sun_draws(&self) -> bool {
        !self.sun_commands.is_empty()
    }
}
