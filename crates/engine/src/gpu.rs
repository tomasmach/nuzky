//! Offscreen wgpu compositor shared by preview and export.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use wgpu::util::DeviceExt;

use crate::model::Adjust;

const SHADER: &str = r#"
struct Layer {
    corners: array<vec4<f32>, 4>, // xy = clip-space position, zw = uv
    opacity: vec4<f32>,
    adjust: vec4<f32>,
    effects: vec4<f32>,
    clip: vec4<f32>,
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
    if in.pos.x < layer.clip.x || in.pos.y < layer.clip.y || in.pos.x >= layer.clip.z || in.pos.y >= layer.clip.w {
        discard;
    }
    var c = textureSample(tex, samp, in.uv);
    if layer.effects.y > 0.0 {
        let step = vec2<f32>(layer.effects.y) / vec2<f32>(textureDimensions(tex));
        c = vec4<f32>(0.0);
        for (var y = -2; y <= 2; y += 1) {
            for (var x = -2; x <= 2; x += 1) {
                c += textureSample(tex, samp, in.uv + vec2<f32>(f32(x), f32(y)) * step) / 25.0;
            }
        }
    }
    var rgb = c.rgb;
    if any(layer.adjust != vec4<f32>(0.0)) || layer.effects.x != 0.0 {
        rgb += layer.adjust.x * 0.4;
        rgb = (rgb - 0.5) * (1.0 + layer.adjust.y * 0.8) + 0.5;
        let luma = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        rgb = mix(vec3<f32>(luma), rgb, 1.0 + layer.adjust.z);
        rgb += vec3<f32>(0.15, 0.025, -0.15) * layer.adjust.w;
        let edge = smoothstep(0.2, 0.72, distance(in.uv, vec2<f32>(0.5)));
        rgb *= 1.0 - edge * layer.effects.x * 0.85;
        rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    let a = c.a * layer.opacity.x;
    let premult = select(a, layer.opacity.x, layer.effects.z > 0.0);
    return vec4<f32>(rgb * premult, a);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LayerUniform {
    corners: [[f32; 4]; 4],
    opacity: [f32; 4],
    adjust: [f32; 4],
    effects: [f32; 4],
    clip: [f32; 4],
}

/// Straight-alpha RGBA image shared between frames without copying.
#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub data: Arc<Vec<u8>>,
}

#[derive(Clone)]
pub struct Layer {
    pub image: Image,
    /// Output pixel positions of the top-left, top-right, bottom-right, bottom-left corners.
    pub corners: [[f32; 2]; 4],
    /// Clockwise rotation of the image inside the quad (0, 90, 180, 270).
    pub uv_rotation: u32,
    pub opacity: f32,
    pub adjust: Adjust,
    pub blur: f32,
    pub clip: Option<[f32; 4]>,
}

pub enum Draw {
    Layer(Layer),
    /// Both layers contribute premultiplied colour to a transparent intermediate.
    Transition([Layer; 2]),
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
    additive: wgpu::RenderPipeline,
    transitions: Vec<(u32, u32, wgpu::Texture)>,
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
        let make_pipeline = |blend| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let pipeline = make_pipeline(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        let component = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let additive = make_pipeline(wgpu::BlendState { color: component, alpha: component });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        Ok(Self { device, queue, pipeline, additive, transitions: Vec::new(), layout, sampler, target: None, textures: HashMap::new(), adapter_name })
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

    fn bind(&self, layer: &Layer, tex: &wgpu::Texture, w: u32, h: u32, premult: bool) -> wgpu::BindGroup {
        let base = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let shift = (layer.uv_rotation / 90) as usize % 4;
        let mut corners = [[0.0; 4]; 4];
        // Triangle strip order: top-left, top-right, bottom-left, bottom-right.
        for (slot, cyclic) in [0usize, 1, 3, 2].into_iter().enumerate() {
            let p = layer.corners[cyclic];
            let uv = base[(cyclic + 4 - shift) % 4];
            corners[slot] = [p[0] / w as f32 * 2.0 - 1.0, 1.0 - p[1] / h as f32 * 2.0, uv[0], uv[1]];
        }
        let a = layer.adjust;
        let uniform = LayerUniform {
            corners, opacity: [layer.opacity.clamp(0.0, 1.0), 0.0, 0.0, 0.0],
            adjust: [a.brightness, a.contrast, a.saturation, a.temperature],
            effects: [a.vignette, layer.blur, premult as u8 as f32, 0.0],
            clip: layer.clip.unwrap_or([0.0, 0.0, w as f32, h as f32]),
        };
        let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("layer"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let view = tex.create_view(&Default::default());
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("layer"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        })
    }

    fn draw_pass(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, background: [f32; 4], groups: &[wgpu::BindGroup], additive: bool) {
        let [r, g, b, a] = background;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
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
        pass.set_pipeline(if additive { &self.additive } else { &self.pipeline });
        for bg in groups {
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..4, 0..1);
        }
    }

    /// Composites the layers bottom to top over `background` and returns tight RGBA rows.
    pub fn render(&mut self, w: u32, h: u32, background: [f32; 4], layers: &[Draw]) -> Result<Vec<u8>> {
        let mut used = Vec::new();
        let mut bind_groups = Vec::new();
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let mut transition_index = 0;
        for draw in layers {
            match draw {
                Draw::Layer(layer) => {
                    let tex = self.texture_for(&layer.image);
                    used.push(Arc::as_ptr(&layer.image.data) as usize);
                    bind_groups.push(self.bind(layer, &tex, w, h, false));
                }
                Draw::Transition(pair) => {
                    let mut groups = Vec::new();
                    for layer in pair {
                        let tex = self.texture_for(&layer.image);
                        used.push(Arc::as_ptr(&layer.image.data) as usize);
                        groups.push(self.bind(layer, &tex, w, h, false));
                    }
                    if self.transitions.get(transition_index).map(|(tw, th, _)| (*tw, *th)) != Some((w, h)) {
                        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                            label: Some("transition"), size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
                        });
                        if transition_index == self.transitions.len() {
                            self.transitions.push((w, h, texture));
                        } else {
                            self.transitions[transition_index] = (w, h, texture);
                        }
                    }
                    let (_, _, texture) = &self.transitions[transition_index];
                    transition_index += 1;
                    let view = texture.create_view(&Default::default());
                    self.draw_pass(&mut encoder, &view, [0.0; 4], &groups, true);
                    let mut layer = pair[0].clone();
                    layer.corners = [[0.0, 0.0], [w as f32, 0.0], [w as f32, h as f32], [0.0, h as f32]];
                    layer.opacity = 1.0;
                    layer.uv_rotation = 0;
                    layer.adjust = Adjust::default();
                    layer.blur = 0.0;
                    layer.clip = None;
                    bind_groups.push(self.bind(&layer, texture, w, h, true));
                }
            }
        }
        self.textures.retain(|k, _| used.contains(k));

        self.target(w, h);
        let target = self.target.as_ref().unwrap();
        self.draw_pass(&mut encoder, &target.view, background, &bind_groups, false);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_adjust_preserves_pixels_and_crossfade_preserves_colour() {
        let mut gpu = Gpu::new().unwrap();
        let pixels = vec![30, 80, 170, 255, 250, 130, 20, 255, 0, 255, 90, 255, 128, 128, 128, 255];
        let layer = Layer {
            image: Image { width: 2, height: 2, data: Arc::new(pixels.clone()) },
            corners: [[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]],
            uv_rotation: 0, opacity: 1.0, adjust: Adjust::default(), blur: 0.0, clip: None,
        };
        assert_eq!(gpu.render(2, 2, [0.0; 4], &[Draw::Layer(layer.clone())]).unwrap(), pixels);
        let mut half = layer.clone();
        half.opacity = 0.5;
        let mixed = gpu.render(2, 2, [1.0, 0.0, 1.0, 1.0], &[Draw::Transition([half.clone(), half])]).unwrap();
        for (a, b) in mixed.iter().zip(&pixels) { assert!((*a as i16 - *b as i16).abs() <= 1); }
        let mut grey = layer;
        grey.adjust.saturation = -1.0;
        let output = gpu.render(2, 2, [0.0; 4], &[Draw::Layer(grey)]).unwrap();
        for pixel in output.chunks_exact(4) { assert_eq!(pixel[0], pixel[1]); assert_eq!(pixel[1], pixel[2]); }
    }
}
