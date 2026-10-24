//! The sky behind the city (`vita/src/sky.rs`): a dome around the eye, at the far plane, under the hour's panorama of
//! the reference's sky (`SKYP`), and the sun's disc.

use ny_frame::mat::Mat4;
use pocket_web_wgpu::gpu::Gpu;
use pocket_web_wgpu::wgpu::{self, util::DeviceExt};

use crate::city::{COLOUR, DEPTH, SAMPLES};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SkyU {
    mvp: Mat4,
    sun: [f32; 8],
    hour: [f32; 4],
}

pub struct Sky {
    pipeline: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
    hours: u32,
}

impl Sky {
    /// `skyp`: the pack's `SKYP` section.
    pub fn new(gpu: &Gpu, skyp: &[u8]) -> Result<Sky, String> {
        let device = &gpu.device;
        let head: ny_pack::SkyHead = ny_pack::read(skyp, 0).ok_or("sky record")?;
        let bytes = (head.w * head.h * 4) as usize;
        if skyp.len() < 16 + bytes * head.count as usize || head.count == 0 {
            return Err("the skies are short".into());
        }
        let panorama = device.create_texture_with_data(
            &gpu.queue,
            &wgpu::TextureDescriptor { label: Some("the skies"), size: wgpu::Extent3d { width: head.w, height: head.h, depth_or_array_layers: head.count }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] },
            wgpu::util::TextureDataOrder::LayerMajor,
            &skyp[16..16 + bytes * head.count as usize],
        );
        let view = panorama.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        // Around the panorama it repeats; at the poles it holds.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("sky"), address_mode_u: wgpu::AddressMode::Repeat, mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        // A unit sphere by longitude and latitude, its rings closer together towards the horizon, where the colour
        // changes fastest; the first column again at the end, so that no triangle wraps round the panorama.
        const RINGS: usize = 24;
        const AROUND: usize = 32;
        const COLS: usize = AROUND + 1;
        let mut v = Vec::with_capacity((RINGS + 1) * COLS * 5);
        for r in 0..=RINGS {
            let t = 1.0 - r as f32 / RINGS as f32 * 2.0;
            let elevation = t * t * t.signum() * core::f32::consts::FRAC_PI_2;
            for a in 0..COLS {
                let u = a as f32 / AROUND as f32;
                let turn = u * core::f32::consts::TAU;
                v.extend_from_slice(&[elevation.cos() * turn.cos(), elevation.sin(), elevation.cos() * turn.sin(), u, 0.5 - elevation / core::f32::consts::PI]);
            }
        }
        let mut i: Vec<u16> = Vec::with_capacity(RINGS * AROUND * 6);
        for r in 0..RINGS {
            for a in 0..AROUND {
                let (p, q) = ((r * COLS + a) as u16, (r * COLS + a + 1) as u16);
                i.extend_from_slice(&[p, q, p + COLS as u16, q, q + COLS as u16, p + COLS as u16]);
            }
        }
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("sky dome"), contents: bytemuck::cast_slice(&v), usage: wgpu::BufferUsages::VERTEX });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("sky dome"), contents: bytemuck::cast_slice(&i), usage: wgpu::BufferUsages::INDEX });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("sky"), size: std::mem::size_of::<SkyU>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) }, wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) }],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("sky"), source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sky.wgsl").into()) });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("sky"), bind_group_layouts: &[&layout], push_constant_ranges: &[] })),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_sky"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout { array_stride: 20, step_mode: wgpu::VertexStepMode::Vertex, attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2] }],
            },
            fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs_sky"), compilation_options: Default::default(), targets: &[Some(COLOUR.into())] }),
            primitive: Default::default(),
            // (the sky covers the whole frame, behind everything: it neither tests nor writes depth)
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: false, depth_compare: wgpu::CompareFunction::Always, stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
            multiview: None,
            cache: None,
        });
        Ok(Sky { pipeline, group, uniforms, vertices, indices, count: i.len() as u32, hours: head.count })
    }

    /// `mvp`: the view and projection with the eye at the origin; `hour`: which of the pack's hours; `sun`: towards
    /// the sun (or the moon), and its colour as the display shows it.
    pub fn prepare(&self, gpu: &Gpu, mvp: &Mat4, hour: usize, sun: &[f32; 8]) {
        let u = SkyU { mvp: *mvp, sun: *sun, hour: [hour.min(self.hours as usize - 1) as f32, 0.0, 0.0, 0.0] };
        gpu.queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&u));
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed(0..self.count, 0, 0..1);
    }
}
