//! Offscreen wgpu compositor shared by preview and export.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use wgpu::util::DeviceExt;

const SHADER: &str = r#"
struct Layer {
    corners: array<vec4<f32>, 4>, // xy = clip-space position, zw = uv
    opacity: vec4<f32>,
};
@group(0) @binding(0) var<uniform> layer: Layer;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    let c = layer.corners[i];
    var out: VsOut;
    out.pos = vec4<f32>(c.xy, 0.0, 1.0);
    out.uv = c.zw;
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, in.uv);
    let a = c.a * layer.opacity.x;
    return vec4<f32>(c.rgb * a, a);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LayerUniform {
    corners: [[f32; 4]; 4],
    opacity: [f32; 4],
}

/// Straight-alpha RGBA image shared between frames without copying.
#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub data: Arc<Vec<u8>>,
}

pub struct Layer {
    pub image: Image,
    /// Output pixel positions of the top-left, top-right, bottom-right, bottom-left corners.
    pub corners: [[f32; 2]; 4],
    /// Clockwise rotation of the image inside the quad (0, 90, 180, 270).
    pub uv_rotation: u32,
    pub opacity: f32,
}

struct Target {
    size: (u32, u32),
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    padded_row: u32,
}

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    target: Option<Target>,
    /// Textures from the previous frame, keyed by the image allocation they hold.
    textures: HashMap<usize, (Arc<Vec<u8>>, wgpu::Texture)>,
    pub adapter_name: String,
}

impl Gpu {
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .context("No GPU adapter found")?;
        let adapter_name = adapter.get_info().name;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("capopen"),
            ..Default::default()
        }))
        .context("Cannot open GPU device")?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compositor"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("compositor"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("compositor"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        Ok(Self { device, queue, pipeline, layout, sampler, target: None, textures: HashMap::new(), adapter_name })
    }

    fn target(&mut self, w: u32, h: u32) -> &Target {
        if self.target.as_ref().map(|t| t.size) != Some((w, h)) {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("frame"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let padded_row = (w * 4).div_ceil(256) * 256;
            let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("readback"),
                size: padded_row as u64 * h as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            self.target = Some(Target { size: (w, h), texture, view, readback, padded_row });
        }
        self.target.as_ref().unwrap()
    }

    fn texture_for(&mut self, image: &Image) -> wgpu::Texture {
        let key = Arc::as_ptr(&image.data) as usize;
        if let Some((_, tex)) = self.textures.get(&key) {
            return tex.clone();
        }
        let size = wgpu::Extent3d { width: image.width, height: image.height, depth_or_array_layers: 1 };
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("layer"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &image.data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(image.width * 4), rows_per_image: None },
            size,
        );
        self.textures.insert(key, (image.data.clone(), tex.clone()));
        tex
    }

    /// Composites the layers bottom to top over `background` and returns tight RGBA rows.
    pub fn render(&mut self, w: u32, h: u32, background: [f32; 4], layers: &[Layer]) -> Result<Vec<u8>> {
        let mut used = Vec::with_capacity(layers.len());
        let mut bind_groups = Vec::with_capacity(layers.len());
        for layer in layers {
            if layer.image.width == 0 || layer.image.height == 0 || layer.opacity <= 0.0 {
                continue;
            }
            let tex = self.texture_for(&layer.image);
            used.push(Arc::as_ptr(&layer.image.data) as usize);
            let base = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
            let shift = (layer.uv_rotation / 90) as usize % 4;
            let mut corners = [[0.0; 4]; 4];
            // Triangle strip order: top-left, top-right, bottom-left, bottom-right.
            for (slot, cyclic) in [0usize, 1, 3, 2].into_iter().enumerate() {
                let p = layer.corners[cyclic];
                let uv = base[(cyclic + 4 - shift) % 4];
                corners[slot] = [p[0] / w as f32 * 2.0 - 1.0, 1.0 - p[1] / h as f32 * 2.0, uv[0], uv[1]];
            }
            let uniform = LayerUniform { corners, opacity: [layer.opacity.clamp(0.0, 1.0), 0.0, 0.0, 0.0] };
            let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("layer"),
                contents: bytemuck::bytes_of(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let view = tex.create_view(&Default::default());
            bind_groups.push(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("layer"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            }));
        }
        self.textures.retain(|k, _| used.contains(k));

        self.target(w, h);
        let target = self.target.as_ref().unwrap();
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let [r, g, b, a] = background;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: a as f64 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            for bg in &bind_groups {
                pass.set_bind_group(0, bg, &[]);
                pass.draw(0..4, 0..1);
            }
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &target.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(target.padded_row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = target.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            tx.send(r).ok();
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).context("GPU poll failed")?;
        rx.recv()?.context("Mapping readback buffer failed")?;
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        {
            let data = slice.get_mapped_range().context("Reading back the frame failed")?;
            let row = (w * 4) as usize;
            for y in 0..h as usize {
                let start = y * target.padded_row as usize;
                out.extend_from_slice(&data[start..start + row]);
            }
        }
        target.readback.unmap();
        Ok(out)
    }
}
