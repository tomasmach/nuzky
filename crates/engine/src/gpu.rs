//! Offscreen wgpu compositor shared by preview and export.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use wgpu::util::DeviceExt;

use crate::media::Transfer;
use crate::model::Adjust;

const SHADER: &str = r#"
struct Layer {
    corners: array<vec4<f32>, 4>, // xy = clip-space position, zw = uv
    local: array<vec4<f32>, 4>, // xy = output pixels from the centre of the visible part
    opacity: vec4<f32>, // opacity, transfer (0 SDR, 1 PQ, 2 HLG)
    adjust: vec4<f32>,
    effects: vec4<f32>,
    grading: vec4<f32>, // exposure, tint, highlights, shadows
    clip: vec4<f32>,
    mask: vec4<f32>, // half width and height of the visible part, corner radius, border width; no mask at 0 width
    border: vec4<f32>, // premultiplied border colour
    shadow: vec4<f32>, // opacity, blur, offset down
};
@group(0) @binding(0) var<uniform> layer: Layer;
@group(0) @binding(1) var tex: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) local: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    let c = layer.corners[i];
    var out: VsOut;
    out.pos = vec4<f32>(c.xy, 0.0, 1.0);
    out.uv = c.zw;
    out.local = layer.local[i].xy;
    return out;
}

// Signed distance from a box with rounded corners centred on 0, negative inside.
fn rounded_box(p: vec2<f32>, half: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half + radius;
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fn srgb_to_linear(rgb: vec3<f32>) -> vec3<f32> {
    return select(pow((rgb + 0.055) / 1.055, vec3<f32>(2.4)), rgb / 12.92, rgb <= vec3<f32>(0.04045));
}

fn linear_to_srgb(rgb: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(rgb, vec3<f32>(1.0 / 2.4)) - 0.055, rgb * 12.92, rgb <= vec3<f32>(0.0031308));
}

const SDR_WHITE_NITS = 203.0;
const BT2020_LUMA = vec3<f32>(0.2627, 0.678, 0.0593);
// Linear BT.2020 to BT.709 primaries; columns are the BT.2020 red, green and blue.
const BT2020_TO_BT709 = mat3x3<f32>(
    vec3<f32>(1.6605, -0.1246, -0.0182),
    vec3<f32>(-0.5876, 1.1329, -0.1006),
    vec3<f32>(-0.0728, -0.0083, 1.1187),
);
// libplacebo's default "spline" tone curve in PQ space, mapping a 1000 nit source peak (HLG's nominal
// peak) to SDR white with the pivot at 40 % of the source range, as libplacebo does when the scene
// average is unknown. Coefficients: Pa (below the pivot), Qa and Qb (above it), shared slope.
const PQ_SOURCE_PEAK = 0.75183;
const SPLINE_SRC_PIVOT = 0.30073;
const SPLINE_DST_PIVOT = 0.27335;
const SPLINE_SLOPE = 0.97912;
const SPLINE_PA = 0.23334;
const SPLINE_QA = 0.73174;
const SPLINE_QB = -0.99026;

fn pq_to_nits(e: vec3<f32>) -> vec3<f32> {
    let p = pow(e, vec3<f32>(1.0 / 78.84375));
    return 10000.0 * pow(max(p - 0.8359375, vec3<f32>(0.0)) / (18.8515625 - 18.6875 * p), vec3<f32>(1.0 / 0.1593017578125));
}

fn nits_to_pq(nits: f32) -> f32 {
    let y = pow(nits / 10000.0, 0.1593017578125);
    return pow((0.8359375 + 18.8515625 * y) / (1.0 + 18.6875 * y), 78.84375);
}

fn tone_map_nits(nits: f32) -> f32 {
    let x = min(nits_to_pq(nits), PQ_SOURCE_PEAK) - SPLINE_SRC_PIVOT;
    let below = (SPLINE_PA * x + SPLINE_SLOPE) * x;
    let above = ((SPLINE_QA * x + SPLINE_QB) * x + SPLINE_SLOPE) * x;
    return pq_to_nits(vec3<f32>(select(below, above, x > 0.0) + SPLINE_DST_PIVOT)).x;
}

// PQ or HLG BT.2020 colour to SDR BT.709. The curve compresses the whole range instead of keeping
// everything up to HDR reference white (203 nits) and clipping above it: phone HLG puts walls and
// faces above reference white, which then washed out. Luminance is mapped and the colour scaled
// with it to keep hue and saturation; the result is encoded for a BT.1886 display like SDR video.
fn hdr_to_sdr(rgb: vec3<f32>, transfer: f32) -> vec3<f32> {
    let e = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    var nits: vec3<f32>;
    if transfer < 1.5 {
        nits = pq_to_nits(e);
    } else {
        let scene = select((exp((e - 0.55991073) / 0.17883277) + 0.28466892) / 12.0, e * e / 3.0, e <= vec3<f32>(0.5));
        // HLG's OOTF on a 1000 nit display (system gamma 1.2).
        nits = 1000.0 * pow(dot(scene, BT2020_LUMA), 0.2) * scene;
    }
    let luma = dot(nits, BT2020_LUMA);
    let mapped = nits * (tone_map_nits(luma) / max(luma, 1e-6));
    let sdr = clamp(BT2020_TO_BT709 * (mapped / SDR_WHITE_NITS), vec3<f32>(0.0), vec3<f32>(1.0));
    return pow(sdr, vec3<f32>(1.0 / 2.4));
}

fn premultiplied_texel(p: vec2<i32>, size: vec2<i32>) -> vec4<f32> {
    let c = textureLoad(tex, clamp(p, vec2<i32>(0), size - 1), 0);
    return select(vec4<f32>(c.rgb * c.a, c.a), c, layer.effects.z > 0.0);
}

// Bilinear filtering of premultiplied colour, so the colour of transparent texels cannot darken
// soft edges. Textures hold straight alpha except the premultiplied transition intermediate.
fn sample_premultiplied(uv: vec2<f32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(tex));
    let p = uv * vec2<f32>(size) - 0.5;
    let i = vec2<i32>(floor(p));
    let f = fract(p);
    let top = mix(premultiplied_texel(i, size), premultiplied_texel(i + vec2<i32>(1, 0), size), f.x);
    let bottom = mix(premultiplied_texel(i + vec2<i32>(0, 1), size), premultiplied_texel(i + 1, size), f.x);
    return mix(top, bottom, f.y);
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    if in.pos.x < layer.clip.x || in.pos.y < layer.clip.y || in.pos.x >= layer.clip.z || in.pos.y >= layer.clip.w {
        discard;
    }
    var c = sample_premultiplied(in.uv);
    if layer.effects.y > 0.0 {
        let step = vec2<f32>(layer.effects.y) / vec2<f32>(textureDimensions(tex));
        c = vec4<f32>(0.0);
        for (var y = -2; y <= 2; y += 1) {
            for (var x = -2; x <= 2; x += 1) {
                c += sample_premultiplied(in.uv + vec2<f32>(f32(x), f32(y)) * step) / 25.0;
            }
        }
    }
    var rgb = c.rgb;
    let adjusting = any(layer.adjust != vec4<f32>(0.0)) || any(layer.grading != vec4<f32>(0.0)) || layer.effects.x != 0.0 || layer.effects.w != 0.0;
    let hdr = layer.opacity.y > 0.0;
    if adjusting || hdr {
        // Tone mapping and adjustments work on straight colour.
        rgb = select(vec3<f32>(0.0), rgb / c.a, c.a > 0.0);
        if hdr {
            rgb = hdr_to_sdr(rgb, layer.opacity.y);
        }
        if adjusting {
            const LUMA = vec3<f32>(0.2126, 0.7152, 0.0722);
            const EXPOSURE_STOPS = 2.0;
            const TONE_STRENGTH = 0.25;
            const FADE_BLACK = 0.25;
            const FADE_WHITE = 0.05;
            if layer.grading.x != 0.0 {
                // Exposure multiplies linear light by 2^stops; skipping zero avoids a lossy round-trip.
                rgb = linear_to_srgb(srgb_to_linear(rgb) * exp2(layer.grading.x * EXPOSURE_STOPS));
            }
            rgb += vec3<f32>(0.15, 0.025, -0.15) * layer.adjust.w;
            // Tint opposes green to equal red/blue shifts, with the temperature control's strength.
            rgb += vec3<f32>(0.075, -0.15, 0.075) * layer.grading.y;
            let tone_luma = dot(rgb, LUMA);
            // Highlights smoothly approach white/black only above mid-grey; the bounded mix avoids hard clipping.
            let highlights = smoothstep(0.5, 1.0, tone_luma) * layer.grading.z * TONE_STRENGTH;
            rgb = mix(rgb, vec3<f32>(select(0.0, 1.0, highlights > 0.0)), abs(highlights));
            // Shadows use the mirrored mask below mid-grey; 0.25 keeps the grey ramp monotonic even at full strength.
            let shadows = (1.0 - smoothstep(0.0, 0.5, tone_luma)) * layer.grading.w * TONE_STRENGTH;
            rgb = mix(rgb, vec3<f32>(select(0.0, 1.0, shadows > 0.0)), abs(shadows));
            rgb = (rgb - 0.5) * (1.0 + layer.adjust.y * 0.8) + 0.5;
            rgb += layer.adjust.x * 0.4;
            let luma = dot(rgb, LUMA);
            rgb = mix(vec3<f32>(luma), rgb, 1.0 + layer.adjust.z);
            // Fade maps black to 0.25 and white to 0.95 at full strength, without changing hue.
            rgb = rgb * (1.0 - layer.effects.w * (FADE_BLACK + FADE_WHITE)) + layer.effects.w * FADE_BLACK;
            let edge = smoothstep(0.2, 0.72, distance(in.uv, vec2<f32>(0.5)));
            rgb *= 1.0 - edge * layer.effects.x * 0.85;
        }
        rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * c.a;
    }
    var color = vec4<f32>(rgb, c.a);
    if layer.mask.x > 0.0 {
        // Coverage of the visible part and of it with the border, smoothed over one pixel.
        let d = rounded_box(in.local, layer.mask.xy, layer.mask.z);
        let inside = clamp(0.5 - d, 0.0, 1.0);
        let bordered = clamp(0.5 - d + layer.mask.w, 0.0, 1.0);
        color = color * inside + layer.border * (bordered - inside);
        if layer.shadow.x > 0.0 {
            let outer = layer.mask.xy + layer.mask.w;
            let s = rounded_box(in.local - vec2<f32>(0.0, layer.shadow.z), outer, layer.mask.z + layer.mask.w);
            color += vec4<f32>(0.0, 0.0, 0.0, layer.shadow.x * (1.0 - smoothstep(-layer.shadow.y, layer.shadow.y, s))) * (1.0 - color.a);
        }
    }
    return color * layer.opacity.x;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LayerUniform {
    corners: [[f32; 4]; 4],
    local: [[f32; 4]; 4],
    opacity: [f32; 4],
    adjust: [f32; 4],
    effects: [f32; 4],
    grading: [f32; 4],
    clip: [f32; 4],
    mask: [f32; 4],
    border: [f32; 4],
    shadow: [f32; 4],
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
    /// Mirrors the rotated image left to right.
    pub mirror: bool,
    pub opacity: f32,
    pub adjust: Adjust,
    pub blur: f32,
    pub clip: Option<[f32; 4]>,
    pub transfer: Transfer,
    pub mask: Option<Mask>,
}

/// The visible part of a layer and how its edge is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mask {
    /// Left, top, right and bottom edges of the visible part, in 0..1 of the quad.
    pub rect: [f32; 4],
    /// Share of half the shorter visible side, 0 to 1; it scales with the quad, as in a zoom transition.
    pub radius: f32,
    /// Output pixels, outside the edge.
    pub border: f32,
    /// Straight RGBA.
    pub border_color: [f32; 4],
    /// Opacity of the shadow, 0 for none.
    pub shadow: f32,
    /// Output pixels.
    pub shadow_blur: f32,
}

impl Mask {
    /// How far the border and shadow reach past the visible part, plus a pixel of edge smoothing.
    fn reach(&self) -> f32 {
        let shadow = if self.shadow > 0.0 { self.shadow_blur * SHADOW_OFFSET + self.shadow_blur } else { 0.0 };
        self.border + shadow + 1.0
    }
}

/// How far a shadow falls below the layer, as a share of its blur.
const SHADOW_OFFSET: f32 = 0.4;

/// Where the image point shown at `(u, v)` of the quad lies in the texture, for an image rotated clockwise by
/// `rotation` and then mirrored.
fn texture_uv(u: f32, v: f32, rotation: u32, mirror: bool) -> [f32; 2] {
    let u = if mirror { 1.0 - u } else { u };
    match rotation % 360 {
        90 => [v, 1.0 - u],
        180 => [1.0 - u, 1.0 - v],
        270 => [1.0 - v, u],
        _ => [u, v],
    }
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

struct TransitionTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    additive: wgpu::RenderPipeline,
    transitions: Vec<TransitionTarget>,
    layout: wgpu::BindGroupLayout,
    target: Option<Target>,
    /// Textures from the previous frame, keyed by the image allocation they hold.
    textures: HashMap<usize, (Arc<Vec<u8>>, wgpu::Texture)>,
    pub adapter_name: String,
}

impl Gpu {
    pub fn new() -> Result<Self> {
        // Debug builds name every GPU object by default. Vulkan loaders before 1.4.345 look the device
        // up for each name without the lock that guards another renderer's new instance freeing unused
        // drivers, so preview and export starting together crashed. WGPU_DEBUG=1 still names them.
        let mut options = wgpu::InstanceDescriptor::new_without_display_handle();
        options.flags.remove(wgpu::InstanceFlags::DEBUG);
        let instance = wgpu::Instance::new(options.with_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .context("No GPU adapter found")?;
        let adapter_name = adapter.get_info().name;
        let (device, queue) = pollster::block_on(
            adapter.request_device(&wgpu::DeviceDescriptor { label: Some("nuzky"), ..Default::default() }),
        )
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
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("compositor"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let make_pipeline = |blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
            })
        };
        let pipeline = make_pipeline(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        let component = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let additive = make_pipeline(wgpu::BlendState { color: component, alpha: component });
        Ok(Self {
            device,
            queue,
            pipeline,
            additive,
            transitions: Vec::new(),
            layout,
            target: None,
            textures: HashMap::new(),
            adapter_name,
        })
    }

    pub fn max_texture_dimension(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
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

    fn texture_for(&mut self, image: &Image) -> Result<wgpu::Texture> {
        let key = Arc::as_ptr(&image.data) as usize;
        if let Some((_, tex)) = self.textures.get(&key) {
            return Ok(tex.clone());
        }
        // wgpu panics on invalid textures; callers size layers to the limit, so this is a bug guard.
        let max = self.max_texture_dimension();
        if image.width == 0
            || image.height == 0
            || image.width.max(image.height) > max
            || image.data.len() < image.width as usize * image.height as usize * 4
        {
            anyhow::bail!("Cannot draw a {}×{} layer (GPU limit {max} px)", image.width, image.height);
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
        Ok(tex)
    }

    fn bind(&self, layer: &Layer, tex: &wgpu::Texture, w: u32, h: u32, premult: bool) -> wgpu::BindGroup {
        let base = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let shift = (layer.uv_rotation / 90) as usize % 4;
        let mut corners = [[0.0; 4]; 4];
        let mut local = [[0.0; 4]; 4];
        let clip_space = |p: [f32; 2]| [p[0] / w as f32 * 2.0 - 1.0, 1.0 - p[1] / h as f32 * 2.0];
        // Triangle strip order: top-left, top-right, bottom-left, bottom-right.
        let strip = [0usize, 1, 3, 2];
        let (mask, border, shadow) = match layer.mask {
            None => {
                for (slot, cyclic) in strip.into_iter().enumerate() {
                    let p = clip_space(layer.corners[cyclic]);
                    // Mirrored, each corner shows what its left-right partner would.
                    let source = if layer.mirror { [1, 0, 3, 2][cyclic] } else { cyclic };
                    let uv = base[(source + 4 - shift) % 4];
                    corners[slot] = [p[0], p[1], uv[0], uv[1]];
                }
                ([0.0; 4], [0.0; 4], [0.0; 4])
            }
            Some(mask) => {
                // The quad is a parallelogram: a point at (u, v) of it is the top-left corner plus u of
                // the top edge and v of the left one. The drawn quad is the visible part plus the reach
                // of the border and shadow.
                let [tl, tr, _, bl] = layer.corners;
                let across = [tr[0] - tl[0], tr[1] - tl[1]];
                let down = [bl[0] - tl[0], bl[1] - tl[1]];
                let size = [across[0].hypot(across[1]).max(1e-3), down[0].hypot(down[1]).max(1e-3)];
                let [left, top, right, bottom] = mask.rect;
                let centre = [(left + right) / 2.0, (top + bottom) / 2.0];
                let reach = [mask.reach() / size[0], mask.reach() / size[1]];
                let drawn = [
                    [left - reach[0], top - reach[1]],
                    [right + reach[0], top - reach[1]],
                    [right + reach[0], bottom + reach[1]],
                    [left - reach[0], bottom + reach[1]],
                ];
                for (slot, cyclic) in strip.into_iter().enumerate() {
                    let [u, v] = drawn[cyclic];
                    let p = clip_space([tl[0] + u * across[0] + v * down[0], tl[1] + u * across[1] + v * down[1]]);
                    let uv = texture_uv(u, v, layer.uv_rotation, layer.mirror);
                    corners[slot] = [p[0], p[1], uv[0], uv[1]];
                    local[slot] = [(u - centre[0]) * size[0], (v - centre[1]) * size[1], 0.0, 0.0];
                }
                // min and max rather than clamp, which panics on the NaN of a broken project.
                let half = [((right - left) * size[0] / 2.0).max(1e-3), ((bottom - top) * size[1] / 2.0).max(1e-3)];
                let [r, g, b, a] = mask.border_color;
                (
                    [half[0], half[1], mask.radius.clamp(0.0, 1.0) * half[0].min(half[1]), mask.border],
                    [r * a, g * a, b * a, a],
                    [mask.shadow, mask.shadow_blur.max(1e-3), mask.shadow_blur * SHADOW_OFFSET, 0.0],
                )
            }
        };
        let a = layer.adjust;
        let uniform = LayerUniform {
            corners,
            local,
            opacity: [layer.opacity.clamp(0.0, 1.0), layer.transfer as u8 as f32, 0.0, 0.0],
            adjust: [a.brightness, a.contrast, a.saturation, a.temperature],
            effects: [a.vignette, layer.blur, premult as u8 as f32, a.fade],
            grading: [a.exposure, a.tint, a.highlights, a.shadows],
            clip: layer.clip.unwrap_or([0.0, 0.0, w as f32, h as f32]),
            mask,
            border,
            shadow,
        };
        let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("layer"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let view = tex.create_view(&Default::default());
        self.bind_group(&buffer, &view)
    }

    fn bind_group(&self, buffer: &wgpu::Buffer, view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("layer"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
            ],
        })
    }

    fn draw_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        background: [f32; 4],
        groups: &[wgpu::BindGroup],
        additive: bool,
    ) {
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
        let max = self.max_texture_dimension();
        if w > max || h > max {
            anyhow::bail!("This resolution is larger than your GPU supports (max {max} px)");
        }
        let mut used = Vec::new();
        let mut bind_groups = Vec::new();
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let mut transition_index = 0;
        for draw in layers {
            match draw {
                Draw::Layer(layer) => {
                    let tex = self.texture_for(&layer.image)?;
                    used.push(Arc::as_ptr(&layer.image.data) as usize);
                    bind_groups.push(self.bind(layer, &tex, w, h, false));
                }
                Draw::Transition(pair) => {
                    let mut groups = Vec::new();
                    for layer in pair {
                        let tex = self.texture_for(&layer.image)?;
                        used.push(Arc::as_ptr(&layer.image.data) as usize);
                        groups.push(self.bind(layer, &tex, w, h, false));
                    }
                    if self.transitions.get(transition_index).map(|t| (t.texture.width(), t.texture.height()))
                        != Some((w, h))
                    {
                        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                            label: Some("transition"),
                            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        });
                        let view = texture.create_view(&Default::default());
                        let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("transition"),
                            size: std::mem::size_of::<LayerUniform>() as u64,
                            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        });
                        let bind_group = self.bind_group(&uniform, &view);
                        let target = TransitionTarget { texture, view, uniform, bind_group };
                        if transition_index == self.transitions.len() {
                            self.transitions.push(target);
                        } else {
                            self.transitions[transition_index] = target;
                        }
                    }
                    let target = &self.transitions[transition_index];
                    transition_index += 1;
                    self.draw_pass(&mut encoder, &target.view, [0.0; 4], &groups, true);
                    let uniform = LayerUniform {
                        corners: [
                            [-1.0, 1.0, 0.0, 0.0],
                            [1.0, 1.0, 1.0, 0.0],
                            [-1.0, -1.0, 0.0, 1.0],
                            [1.0, -1.0, 1.0, 1.0],
                        ],
                        local: [[0.0; 4]; 4],
                        opacity: [1.0, 0.0, 0.0, 0.0],
                        adjust: [0.0; 4],
                        effects: [0.0, 0.0, 1.0, 0.0],
                        grading: [0.0; 4],
                        clip: [0.0, 0.0, w as f32, h as f32],
                        mask: [0.0; 4],
                        border: [0.0; 4],
                        shadow: [0.0; 4],
                    };
                    self.queue.write_buffer(&target.uniform, 0, bytemuck::bytes_of(&uniform));
                    bind_groups.push(target.bind_group.clone());
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
    fn transition_resources_reuse_and_resize() {
        let mut gpu = Gpu::new().unwrap();
        let layer = Layer {
            image: Image { width: 1, height: 1, data: Arc::new(vec![80, 100, 120, 255]) },
            corners: [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]],
            uv_rotation: 0,
            mirror: false,
            opacity: 0.5,
            adjust: Adjust::default(),
            blur: 0.0,
            clip: None,
            transfer: Transfer::Sdr,
            mask: None,
        };
        let draws = [Draw::Transition([layer.clone(), layer])];
        let first = gpu.render(2, 2, [0.0; 4], &draws).unwrap();
        let group = gpu.transitions[0].bind_group.clone();
        assert_eq!(gpu.render(2, 2, [0.0; 4], &draws).unwrap(), first);
        assert_eq!(gpu.transitions[0].bind_group, group);
        let resized = gpu.render(4, 4, [0.0; 4], &draws).unwrap();
        assert_ne!(gpu.transitions[0].bind_group, group);
        assert!(resized.as_chunks::<4>().0.iter().all(|p| p == &first[..4]));
        let max = gpu.max_texture_dimension();
        assert_eq!(
            gpu.render(max + 1, 2, [0.0; 4], &[]).unwrap_err().to_string(),
            format!("This resolution is larger than your GPU supports (max {max} px)")
        );
    }

    /// Text and PNG edges store transparent texels as black; filtering them unpremultiplied
    /// drew a dark fringe around scaled white text over a light background.
    #[test]
    fn scaled_soft_edges_have_no_dark_fringe() {
        let mut gpu = Gpu::new().unwrap();
        let mut layer = Layer {
            image: Image { width: 2, height: 1, data: Arc::new(vec![255, 255, 255, 255, 0, 0, 0, 0]) },
            corners: [[0.0, 0.0], [64.0, 0.0], [64.0, 2.0], [0.0, 2.0]],
            uv_rotation: 0,
            mirror: false,
            opacity: 1.0,
            adjust: Adjust::default(),
            blur: 0.0,
            clip: None,
            transfer: Transfer::Sdr,
            mask: None,
        };
        for adjust in [false, true] {
            layer.adjust.contrast = if adjust { 0.2 } else { 0.0 };
            let out = gpu.render(64, 2, [1.0; 4], &[Draw::Layer(layer.clone())]).unwrap();
            let darkest = out.as_chunks::<4>().0.iter().map(|p| p[0].min(p[1]).min(p[2])).min().unwrap();
            assert!(darkest >= 254, "adjust={adjust}: darkest edge pixel {darkest}");
        }
        // Half-covered texels keep their straight colour through adjustments.
        layer.image = Image { width: 1, height: 1, data: Arc::new(vec![200, 100, 50, 128]) };
        layer.adjust = Adjust { saturation: -1.0, ..Adjust::default() };
        let out = gpu.render(2, 2, [0.0, 0.0, 0.0, 1.0], &[Draw::Layer(layer)]).unwrap();
        let grey = (0.2126 * 200.0 + 0.7152 * 100.0 + 0.0722 * 50.0) * 128.0 / 255.0;
        assert!(out[..3].iter().all(|&v| (v as f32 - grey).abs() <= 1.5), "{:?} vs {grey}", &out[..4]);
    }

    /// Preview keeps drawing while an export or an agent's contact sheet starts its own renderer.
    /// The Vulkan loader freed driver records under the drawing threads and crashed the process.
    #[test]
    fn renderers_start_and_stop_while_others_draw() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let draw = |gpu: &mut Gpu| {
            let layer = Layer {
                image: Image { width: 1, height: 1, data: Arc::new(vec![80, 100, 120, 255]) },
                corners: [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]],
                uv_rotation: 0,
                mirror: false,
                opacity: 1.0,
                adjust: Adjust::default(),
                blur: 0.0,
                clip: None,
                transfer: Transfer::Sdr,
                mask: None,
            };
            assert_eq!(gpu.render(1, 1, [0.0; 4], &[Draw::Layer(layer)]).unwrap(), [80, 100, 120, 255]);
        };
        let done = AtomicBool::new(false);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    let mut gpu = Gpu::new().unwrap();
                    while !done.load(Ordering::Relaxed) {
                        draw(&mut gpu);
                    }
                });
            }
            let starters: Vec<_> =
                (0..4).map(|_| scope.spawn(|| (0..8).for_each(|_| draw(&mut Gpu::new().unwrap())))).collect();
            let started: Vec<_> = starters.into_iter().map(|starter| starter.join()).collect();
            done.store(true, Ordering::Relaxed);
            for result in started {
                if let Err(panic) = result {
                    std::panic::resume_unwind(panic);
                }
            }
        });
    }

    #[test]
    fn zero_adjust_preserves_pixels_and_crossfade_preserves_colour() {
        let mut gpu = Gpu::new().unwrap();
        let pixels = vec![30, 80, 170, 255, 250, 130, 20, 255, 0, 255, 90, 255, 128, 128, 128, 255];
        let layer = Layer {
            image: Image { width: 2, height: 2, data: Arc::new(pixels.clone()) },
            corners: [[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]],
            uv_rotation: 0,
            mirror: false,
            opacity: 1.0,
            adjust: Adjust::default(),
            blur: 0.0,
            clip: None,
            transfer: Transfer::Sdr,
            mask: None,
        };
        assert_eq!(gpu.render(2, 2, [0.0; 4], &[Draw::Layer(layer.clone())]).unwrap(), pixels);
        let mut half = layer.clone();
        half.opacity = 0.5;
        let mixed = gpu.render(2, 2, [1.0, 0.0, 1.0, 1.0], &[Draw::Transition([half.clone(), half])]).unwrap();
        for (a, b) in mixed.iter().zip(&pixels) {
            assert!((*a as i16 - *b as i16).abs() <= 1);
        }
        let mut grey = layer;
        grey.adjust.saturation = -1.0;
        let output = gpu.render(2, 2, [0.0; 4], &[Draw::Layer(grey)]).unwrap();
        for pixel in output.as_chunks::<4>().0 {
            assert_eq!(pixel[0], pixel[1]);
            assert_eq!(pixel[1], pixel[2]);
        }
    }
}
