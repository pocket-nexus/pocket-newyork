//! The scene's targets and the glow of what is bright (`vita/src/glow.rs`): the scene is drawn into a target of
//! `SAMPLES` samples a pixel and resolved into a texture; a quarter-size chain keeps what is bright in it and blurs it
//! across and down, and the frame goes to the display with the glow laid over it as a screen. The PS Vita lays on the
//! glow of the frame it is showing (lights glow a frame late there, and no pass copies its scene); here it is this
//! frame's.

use pocket_web_wgpu::gpu::{Frame, Gpu};
use pocket_web_wgpu::wgpu::{self, TextureFormat, TextureView};

use crate::city::{COLOUR, DEPTH, SAMPLES};

/// The bright chain runs at a quarter of the frame.
const QUARTER: u32 = 4;

pub struct Post {
    width: u32,
    height: u32,
    samples: TextureView,
    pub depth: TextureView,
    scene: TextureView,
    a: TextureView,
    b: TextureView,
    bright: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    frame: wgpu::RenderPipeline,
    /// The four passes' uniforms: bright, blur across, blur down, the frame; each 256 bytes from the last.
    uniforms: wgpu::Buffer,
    groups: [wgpu::BindGroup; 4],
}

fn target(gpu: &Gpu, label: &str, format: TextureFormat, width: u32, height: u32, samples: u32, sampled: bool) -> TextureView {
    let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | if sampled { wgpu::TextureUsages::TEXTURE_BINDING } else { wgpu::TextureUsages::empty() };
    gpu.device
        .create_texture(&wgpu::TextureDescriptor { label: Some(label), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: samples, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[] })
        .create_view(&Default::default())
}

impl Post {
    /// For a screen of `width` × `height` pixels of `format`.
    pub fn new(gpu: &Gpu, format: TextureFormat, width: u32, height: u32) -> Post {
        let device = &gpu.device;
        let (qw, qh) = (width / QUARTER, height / QUARTER);
        let samples = target(gpu, "the scene's samples", COLOUR, width, height, SAMPLES, false);
        let depth = target(gpu, "the scene's depth", DEPTH, width, height, SAMPLES, false);
        let scene = target(gpu, "the scene", COLOUR, width, height, 1, true);
        let a = target(gpu, "bright", COLOUR, qw, qh, 1, true);
        let b = target(gpu, "bright, blurred across", COLOUR, qw, qh, 1, true);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("post"), source: wgpu::ShaderSource::Wgsl(include_str!("shaders/post.wgsl").into()) });
        let tex = |binding| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                tex(1),
                tex(2),
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("post"), bind_group_layouts: &[&layout], push_constant_ranges: &[] });
        let pipeline = |fs: &str, format: TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_post"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some(fs), compilation_options: Default::default(), targets: &[Some(format.into())] }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("post"), size: 256 * 4, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("post"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let group = |k: u64, source: &TextureView, glow: &TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("post"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &uniforms, offset: k * 256, size: wgpu::BufferSize::new(16) }) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(source) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(glow) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&sampler) },
                ],
            })
        };
        // (a pass binds as its glow a texture it does not draw into)
        let groups = [group(0, &scene, &b), group(1, &a, &a), group(2, &b, &b), group(3, &scene, &a)];
        Post { width, height, bright: pipeline("fs_bright", COLOUR), blur: pipeline("fs_blur", COLOUR), frame: pipeline("fs_frame", format), uniforms, groups, samples, depth, scene, a, b }
    }

    /// The scene's pass: its samples resolved into the scene, cleared first (the sky covers what it does not).
    pub fn scene_pass<'a>(&'a self, encoder: &'a mut wgpu::CommandEncoder) -> wgpu::RenderPass<'a> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &self.samples, resolve_target: Some(&self.scene), ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Discard } })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &self.depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Discard }), stencil_ops: None }),
            timestamp_writes: None,
            occlusion_query_set: None,
        })
    }

    fn pass(encoder: &mut wgpu::CommandEncoder, view: &TextureView, pipeline: &wgpu::RenderPipeline, group: &wgpu::BindGroup) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("post"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// After the scene: the bright chain (when `gain` is over 0), then the frame to the display with its glow.
    pub fn finish(&self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, frame: &Frame, gain: f32, threshold: f32) {
        let (qw, qh) = ((self.width / QUARTER) as f32, (self.height / QUARTER) as f32);
        let mut u = [0.0f32; 256];
        u[..2].copy_from_slice(&[threshold, 1.0 / (1.0 - threshold).max(0.05)]);
        u[64] = 1.0 / qw;
        u[128 + 1] = 1.0 / qh;
        u[192] = gain;
        gpu.queue.write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&u));
        if gain > 0.0 {
            Self::pass(encoder, &self.a, &self.bright, &self.groups[0]);
            Self::pass(encoder, &self.b, &self.blur, &self.groups[1]);
            Self::pass(encoder, &self.a, &self.blur, &self.groups[2]);
        } else {
            // (nothing glows: the glow's texture is cleared)
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("no glow"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &self.a, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        Self::pass(encoder, frame.shown(), &self.frame, &self.groups[3]);
    }
}
