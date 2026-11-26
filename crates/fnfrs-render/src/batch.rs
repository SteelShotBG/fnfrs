//! Zero-allocation instanced 2D sprite batcher with Normal, Add, and Screen
//! blend pipelines.

use bytemuck::{Pod, Zeroable};
use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};

pub use crate::gpu::FilterMode;
use crate::gpu::{
    orthographic_projection, zoomed_orthographic_projection, GpuContext, TextureHandle,
};
use crate::shaders::create_sprite_shader_module;

/// Flag indicating the subtexture is rotated 90 degrees CCW in the atlas and
/// must be rotated 90 degrees CW when sampled (Sparrow `rotated="true"`).
pub const SPRITE_FLAG_ROTATED_90_CW: u32 = 1 << 0;

/// Flag indicating horizontal mirroring around the upright sprite center.
pub const SPRITE_FLAG_FLIP_X: u32 = 1 << 1;

/// Flag indicating vertical mirroring around the upright sprite center.
pub const SPRITE_FLAG_FLIP_Y: u32 = 1 << 2;

/// Flag indicating final world coordinates should be rounded to integer pixels.
pub const SPRITE_FLAG_PIXEL_SNAP: u32 = 1 << 3;

/// Flag indicating the subtexture is rotated 90 degrees CW in the atlas and
/// must be rotated 90 degrees CCW when sampled (Adobe Animate `spritemap` `rotated: true`,
/// and Sparrow `rotated="true"`).
pub const SPRITE_FLAG_ROTATED_90_CCW: u32 = 1 << 4;

/// Flag indicating `size` stores the X-edge vector `(m.a*w, m.b*w)` and `origin` stores the
/// Y-edge vector `(m.c*h, m.d*h)` for arbitrary 2D affine transforms (including skew).
pub const SPRITE_FLAG_AFFINE: u32 = 1 << 5;

/// Flag enabling Flash `AdjustColor` (`hsbc = [hue_rad, sat_mult, bright_add, contrast_mult]`).
pub const SPRITE_FLAG_ADJUST_COLOR: u32 = 1 << 6;

/// Blend mode used when drawing sprite or mesh batches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BlendMode {
    #[default]
    Normal,
    Add,
    Screen,
}

impl BlendMode {
    /// Converts to the corresponding `wgpu::BlendState` for color targets.
    #[must_use]
    pub const fn to_wgpu_blend_state(self) -> wgpu::BlendState {
        match self {
            Self::Normal => wgpu::BlendState::ALPHA_BLENDING,
            Self::Add => wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            },
            Self::Screen => wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::OneMinusSrc,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            },
        }
    }
}

/// Per-instance GPU data uploaded to the sprite instance buffer.
///
/// Layout is 96 bytes (`#[repr(C)]`, zero implicit padding):
/// - `pos`: top-left position in world/virtual pixels
/// - `size`: unrotated drawn width and height in world/virtual pixels
/// - `uv_rect`: normalized `[u0, v0, u1, v1]`
/// - `color`: linear/normalized RGBA tint `[r, g, b, a]`
/// - `hsv_shift`: `[hue_delta, sat_delta, val_delta]` (`[0.0; 3]` is identity)
/// - `rotation`: clockwise rotation in radians around `origin`
/// - `origin`: local pivot offset relative to top-left `pos`
/// - `flags`: bitmask of `SPRITE_FLAG_*` constants
/// - `hsbc`: Flash `AdjustColor` `[hue_rad, sat_mult, bright_add, contrast_mult]`
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct SpriteInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub uv_rect: [f32; 4],
    pub color: [f32; 4],
    pub hsv_shift: [f32; 3],
    pub rotation: f32,
    pub origin: [f32; 2],
    pub flags: u32,
    pub _pad: f32,
    pub hsbc: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<SpriteInstance>() == 96);

impl Default for SpriteInstance {
    fn default() -> Self {
        Self {
            pos: [0.0, 0.0],
            size: [0.0, 0.0],
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            color: [1.0, 1.0, 1.0, 1.0],
            hsv_shift: [0.0, 0.0, 0.0],
            rotation: 0.0,
            origin: [0.0, 0.0],
            flags: 0,
            _pad: 0.0,
            hsbc: [0.0, 1.0, 0.0, 1.0],
        }
    }
}

impl SpriteInstance {
    /// Creates an axis-aligned sprite instance with default tint and full UVs.
    #[inline]
    #[must_use]
    pub const fn new(pos: [f32; 2], size: [f32; 2]) -> Self {
        Self {
            pos,
            size,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            color: [1.0, 1.0, 1.0, 1.0],
            hsv_shift: [0.0, 0.0, 0.0],
            rotation: 0.0,
            origin: [0.0, 0.0],
            flags: 0,
            _pad: 0.0,
            hsbc: [0.0, 1.0, 0.0, 1.0],
        }
    }

    /// Sets normalized `[u0, v0, u1, v1]` UVs.
    #[inline]
    #[must_use]
    pub const fn with_uv_rect(mut self, uv_rect: [f32; 4]) -> Self {
        self.uv_rect = uv_rect;
        self
    }

    /// Sets RGBA tint `[r, g, b, a]`.
    #[inline]
    #[must_use]
    pub const fn with_color(mut self, color: [f32; 4]) -> Self {
        self.color = color;
        self
    }

    /// Sets HSV shift `[hue_delta, sat_delta, val_delta]`.
    #[inline]
    #[must_use]
    pub const fn with_hsv_shift(mut self, hsv_shift: [f32; 3]) -> Self {
        self.hsv_shift = hsv_shift;
        self
    }

    /// Sets clockwise rotation in radians around `origin`.
    #[inline]
    #[must_use]
    pub const fn with_rotation(mut self, rotation: f32, origin: [f32; 2]) -> Self {
        self.rotation = rotation;
        self.origin = origin;
        self
    }

    /// Sets the local pivot/Y-edge vector `origin` directly (used with `SPRITE_FLAG_AFFINE`).
    #[inline]
    #[must_use]
    pub const fn with_origin(mut self, origin: [f32; 2]) -> Self {
        self.origin = origin;
        self
    }

    /// Sets `SPRITE_FLAG_*` bitmask.
    #[inline]
    #[must_use]
    pub const fn with_flags(mut self, flags: u32) -> Self {
        self.flags = flags;
        self
    }

    /// Applies Adobe Animate / Flash `AdjustColor` (`hue_deg`, `saturation`, `brightness`, `contrast`).
    #[inline]
    #[must_use]
    pub fn with_adjust_color(
        mut self,
        hue_deg: f32,
        saturation: f32,
        brightness: f32,
        contrast: f32,
    ) -> Self {
        let hue_rad = hue_deg.to_radians();
        let sat_mult = 1.0 + (if saturation > 0.0 { saturation * 3.0 } else { saturation }) / 100.0;
        let bright_add = brightness / 255.0;
        let mut c = 1.0 + contrast / 100.0;
        if c > 1.0 {
            c = (((0.008_522_59 * (4.764_54 * (c - 1.0)).exp()) * 1.01) - 0.008_607_816) * 10.0
                + 1.0;
        }
        self.hsbc = [hue_rad, sat_mult, bright_add, c];
        self.flags |= SPRITE_FLAG_ADJUST_COLOR;
        self
    }

    /// Returns the `wgpu::VertexBufferLayout` describing `SpriteInstance` at
    /// shader locations `0..=8`.
    #[must_use]
    pub const fn vertex_buffer_layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRIBS: [wgpu::VertexAttribute; 9] = [
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 8,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 16,
                shader_location: 2,
                format: wgpu::VertexFormat::Float32x4,
            },
            wgpu::VertexAttribute {
                offset: 32,
                shader_location: 3,
                format: wgpu::VertexFormat::Float32x4,
            },
            wgpu::VertexAttribute {
                offset: 48,
                shader_location: 4,
                format: wgpu::VertexFormat::Float32x3,
            },
            wgpu::VertexAttribute {
                offset: 60,
                shader_location: 5,
                format: wgpu::VertexFormat::Float32,
            },
            wgpu::VertexAttribute {
                offset: 64,
                shader_location: 6,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 72,
                shader_location: 7,
                format: wgpu::VertexFormat::Uint32,
            },
            wgpu::VertexAttribute {
                offset: 80,
                shader_location: 8,
                format: wgpu::VertexFormat::Float32x4,
            },
        ];

        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SpriteInstance>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRIBS,
        }
    }
}

/// Contiguous range of `SpriteInstance`s sharing texture, blend mode, and
/// filter mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpriteDrawGroup {
    pub texture: TextureHandle,
    pub blend: BlendMode,
    pub filter: FilterMode,
    pub instance_start: u32,
    pub instance_end: u32,
}

/// Pre-allocated CPU sprite queue that coalesces adjacent compatible draw calls
/// with zero per-frame heap allocations.
#[derive(Debug)]
pub struct SpriteBatchQueue {
    instances: Vec<SpriteInstance>,
    groups: Vec<SpriteDrawGroup>,
}

impl SpriteBatchQueue {
    /// Pre-allocates backing storage for `instance_capacity` instances and
    /// `group_capacity` draw groups.
    #[must_use]
    pub fn with_capacity(instance_capacity: usize, group_capacity: usize) -> Self {
        Self {
            instances: Vec::with_capacity(instance_capacity),
            groups: Vec::with_capacity(group_capacity),
        }
    }

    /// Clears recorded instances and draw groups without freeing or shrinking
    /// backing allocations.
    #[inline]
    pub fn begin_frame(&mut self) {
        self.instances.clear();
        self.groups.clear();
    }

    /// Appends one sprite instance, coalescing into the trailing draw group
    /// when `(texture, blend, filter)` matches.
    #[inline]
    pub fn push(
        &mut self,
        texture: TextureHandle,
        blend: BlendMode,
        filter: FilterMode,
        instance: SpriteInstance,
    ) {
        let start = self.instances.len() as u32;
        self.instances.push(instance);
        let end = start + 1;

        if let Some(last) = self.groups.last_mut() {
            if last.texture == texture
                && last.blend == blend
                && last.filter == filter
                && last.instance_end == start
            {
                last.instance_end = end;
                return;
            }
        }

        self.groups.push(SpriteDrawGroup {
            texture,
            blend,
            filter,
            instance_start: start,
            instance_end: end,
        });
    }

    /// Slice of queued instances for the current frame.
    #[inline]
    #[must_use]
    pub fn instances(&self) -> &[SpriteInstance] {
        &self.instances
    }

    /// Slice of coalesced draw groups for the current frame.
    #[inline]
    #[must_use]
    pub fn groups(&self) -> &[SpriteDrawGroup] {
        &self.groups
    }

    /// Allocated capacity of the CPU instance `Vec`.
    #[inline]
    #[must_use]
    pub fn instance_capacity(&self) -> usize {
        self.instances.capacity()
    }

    /// Allocated capacity of the CPU draw-group `Vec`.
    #[inline]
    #[must_use]
    pub fn group_capacity(&self) -> usize {
        self.groups.capacity()
    }
}

/// GPU instanced sprite batcher holding pre-allocated CPU queues, a dynamic
/// GPU instance buffer, camera uniform buffer, and `Normal`/`Add`/`Screen`
/// render pipelines.
pub struct SpriteBatcher {
    queue: SpriteBatchQueue,
    instance_buffer: wgpu::Buffer,
    instance_buffer_capacity: usize,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    pipeline_normal: wgpu::RenderPipeline,
    pipeline_add: wgpu::RenderPipeline,
    pipeline_screen: wgpu::RenderPipeline,
}

impl SpriteBatcher {
    /// Creates a new `SpriteBatcher` with `initial_capacity` pre-allocated
    /// instance slots on both CPU and GPU.
    #[must_use]
    pub fn new(gpu: &GpuContext, initial_capacity: usize) -> Self {
        let cap = initial_capacity.max(64);
        let group_cap = (cap / 2).max(64);
        let queue = SpriteBatchQueue::with_capacity(cap, group_cap);

        let instance_buffer = create_instance_buffer(&gpu.device, cap);

        let proj = orthographic_projection(VIRTUAL_WIDTH, VIRTUAL_HEIGHT).to_cols_array_2d();
        let camera_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fnfrs-sprite-camera-uniform"),
            size: std::mem::size_of::<[[f32; 4]; 4]>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue
            .write_buffer(&camera_buffer, 0, bytemuck::cast_slice(&proj));

        let camera_bind_group_layout =
            gpu.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("fnfrs-sprite-camera-bgl"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    }],
                });

        let camera_bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fnfrs-sprite-camera-bg"),
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout =
            gpu.device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("fnfrs-sprite-pipeline-layout"),
                    bind_group_layouts: &[
                        Some(&camera_bind_group_layout),
                        Some(&gpu.texture_bind_group_layout),
                    ],
                    immediate_size: 0,
                });

        let shader = create_sprite_shader_module(&gpu.device);
        let format = gpu.surface_format();

        let pipeline_normal = create_sprite_pipeline(
            &gpu.device,
            &pipeline_layout,
            &shader,
            format,
            BlendMode::Normal,
            "fnfrs-sprite-pipeline-normal",
        );
        let pipeline_add = create_sprite_pipeline(
            &gpu.device,
            &pipeline_layout,
            &shader,
            format,
            BlendMode::Add,
            "fnfrs-sprite-pipeline-add",
        );
        let pipeline_screen = create_sprite_pipeline(
            &gpu.device,
            &pipeline_layout,
            &shader,
            format,
            BlendMode::Screen,
            "fnfrs-sprite-pipeline-screen",
        );

        Self {
            queue,
            instance_buffer,
            instance_buffer_capacity: cap,
            camera_buffer,
            camera_bind_group,
            pipeline_normal,
            pipeline_add,
            pipeline_screen,
        }
    }

    /// Resets CPU sprite lists for a new frame without freeing capacity.
    #[inline]
    pub fn begin_frame(&mut self) {
        self.queue.begin_frame();
    }

    /// Updates the orthographic/camera projection uniform matrix.
    #[inline]
    pub fn set_projection(&self, queue: &wgpu::Queue, view_proj: glam::Mat4) {
        let cols = view_proj.to_cols_array_2d();
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::cast_slice(&cols));
    }

    /// Resets the projection uniform to the standard `1280x720` virtual canvas.
    #[inline]
    pub fn set_virtual_projection(&self, queue: &wgpu::Queue) {
        self.set_projection(queue, orthographic_projection(VIRTUAL_WIDTH, VIRTUAL_HEIGHT));
    }

    /// Sets the projection uniform to the `1280x720` virtual canvas with centered zoom.
    #[inline]
    pub fn set_zoomed_virtual_projection(&self, queue: &wgpu::Queue, zoom: f32) {
        self.set_projection(
            queue,
            zoomed_orthographic_projection(VIRTUAL_WIDTH, VIRTUAL_HEIGHT, zoom),
        );
    }

    /// Enqueues a sprite instance for the current frame.
    #[inline]
    pub fn push(
        &mut self,
        texture: TextureHandle,
        blend: BlendMode,
        filter: FilterMode,
        instance: SpriteInstance,
    ) {
        self.queue.push(texture, blend, filter, instance);
    }

    /// Returns the number of queued instances in the current frame.
    #[inline]
    #[must_use]
    pub fn instance_count(&self) -> usize {
        self.queue.instances().len()
    }

    /// Returns the number of coalesced draw groups in the current frame.
    #[inline]
    #[must_use]
    pub fn group_count(&self) -> usize {
        self.queue.groups().len()
    }

    /// Returns the CPU instance capacity.
    #[inline]
    #[must_use]
    pub fn instance_capacity(&self) -> usize {
        self.queue.instance_capacity()
    }

    /// Uploads queued sprite instances to the GPU instance buffer, growing the
    /// GPU buffer only if `instance_count > instance_buffer_capacity`.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let instances = self.queue.instances();
        if instances.is_empty() {
            return;
        }
        if instances.len() > self.instance_buffer_capacity {
            let new_cap = instances.len().next_power_of_two();
            self.instance_buffer = create_instance_buffer(device, new_cap);
            self.instance_buffer_capacity = new_cap;
        }
        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(instances));
    }

    /// Records all queued sprite draw calls into `render_pass`.
    pub fn render(&self, render_pass: &mut wgpu::RenderPass<'_>, gpu: &GpuContext) {
        let groups = self.queue.groups();
        if groups.is_empty() {
            return;
        }

        render_pass.set_bind_group(0, &self.camera_bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.instance_buffer.slice(..));

        let mut active_blend: Option<BlendMode> = None;

        for group in groups {
            let Some(tex_bg) = gpu.texture_bind_group(group.texture, group.filter, false) else {
                continue;
            };

            if active_blend != Some(group.blend) {
                let pipeline = match group.blend {
                    BlendMode::Normal => &self.pipeline_normal,
                    BlendMode::Add => &self.pipeline_add,
                    BlendMode::Screen => &self.pipeline_screen,
                };
                render_pass.set_pipeline(pipeline);
                active_blend = Some(group.blend);
            }

            render_pass.set_bind_group(1, tex_bg, &[]);
            render_pass.draw(0..6, group.instance_start..group.instance_end);
        }
    }
}

/// Computes the transformed `[u, v]` corner of a unit quad `(cx, cy)` inside
/// `uv_rect = [u0, v0, u1, v1]` for the given `SPRITE_FLAG_*` bitmask.
///
/// Mirrors the exact UV math in `shaders/sprite.wgsl` for unit testing.
#[must_use]
pub fn compute_sprite_corner_uv(uv_rect: [f32; 4], corner: [f32; 2], flags: u32) -> [f32; 2] {
    let flip_x = (flags & SPRITE_FLAG_FLIP_X) != 0;
    let flip_y = (flags & SPRITE_FLAG_FLIP_Y) != 0;
    let fx = if flip_x { 1.0 - corner[0] } else { corner[0] };
    let fy = if flip_y { 1.0 - corner[1] } else { corner[1] };

    let (lu, lv) = if (flags & SPRITE_FLAG_ROTATED_90_CW) != 0 {
        (fy, 1.0 - fx)
    } else if (flags & SPRITE_FLAG_ROTATED_90_CCW) != 0 {
        (1.0 - fy, fx)
    } else {
        (fx, fy)
    };

    [
        uv_rect[0] + (uv_rect[2] - uv_rect[0]) * lu,
        uv_rect[1] + (uv_rect[3] - uv_rect[1]) * lv,
    ]
}

fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    let byte_size = (capacity * std::mem::size_of::<SpriteInstance>()) as wgpu::BufferAddress;
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fnfrs-sprite-instance-buffer"),
        size: byte_size.max(80),
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_sprite_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    target_format: wgpu::TextureFormat,
    blend: BlendMode,
    label: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[Some(SpriteInstance::vertex_buffer_layout())],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: Some(blend.to_wgpu_blend_state()),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn sprite_instance_layout_and_alignment() {
        assert_eq!(std::mem::size_of::<SpriteInstance>(), 80);
        assert_eq!(std::mem::align_of::<SpriteInstance>(), 4);
        assert!(std::mem::size_of::<SpriteInstance>().is_multiple_of(16));
        assert_eq!(bytemuck::offset_of!(SpriteInstance, pos), 0);
        assert_eq!(bytemuck::offset_of!(SpriteInstance, size), 8);
        assert_eq!(bytemuck::offset_of!(SpriteInstance, uv_rect), 16);
        assert_eq!(bytemuck::offset_of!(SpriteInstance, color), 32);
        assert_eq!(bytemuck::offset_of!(SpriteInstance, hsv_shift), 48);
        assert_eq!(bytemuck::offset_of!(SpriteInstance, rotation), 60);
        assert_eq!(bytemuck::offset_of!(SpriteInstance, origin), 64);
        assert_eq!(bytemuck::offset_of!(SpriteInstance, flags), 72);
        assert_eq!(bytemuck::offset_of!(SpriteInstance, _pad), 76);
    }

    #[test]
    fn uv_rotation_and_flip_math() {
        let rect = [0.2, 0.4, 0.6, 0.8];

        // Unrotated corners
        let tl = compute_sprite_corner_uv(rect, [0.0, 0.0], 0);
        let br = compute_sprite_corner_uv(rect, [1.0, 1.0], 0);
        assert!(approx_eq(tl[0], 0.2) && approx_eq(tl[1], 0.4));
        assert!(approx_eq(br[0], 0.6) && approx_eq(br[1], 0.8));

        // Sparrow rotated="true" (packed 90 deg CCW -> rotated 90 deg CW on screen):
        // Upright TL (0, 0) -> Atlas BL (u0, v1)
        // Upright TR (1, 0) -> Atlas TL (u0, v0)
        // Upright BL (0, 1) -> Atlas BR (u1, v1)
        // Upright BR (1, 1) -> Atlas TR (u1, v0)
        let r_tl = compute_sprite_corner_uv(rect, [0.0, 0.0], SPRITE_FLAG_ROTATED_90_CW);
        let r_tr = compute_sprite_corner_uv(rect, [1.0, 0.0], SPRITE_FLAG_ROTATED_90_CW);
        let r_bl = compute_sprite_corner_uv(rect, [0.0, 1.0], SPRITE_FLAG_ROTATED_90_CW);
        let r_br = compute_sprite_corner_uv(rect, [1.0, 1.0], SPRITE_FLAG_ROTATED_90_CW);
        assert!(approx_eq(r_tl[0], 0.2) && approx_eq(r_tl[1], 0.8));
        assert!(approx_eq(r_tr[0], 0.2) && approx_eq(r_tr[1], 0.4));
        assert!(approx_eq(r_bl[0], 0.6) && approx_eq(r_bl[1], 0.8));
        assert!(approx_eq(r_br[0], 0.6) && approx_eq(r_br[1], 0.4));

        // Flip X on unrotated
        let fx_tl = compute_sprite_corner_uv(rect, [0.0, 0.0], SPRITE_FLAG_FLIP_X);
        assert!(approx_eq(fx_tl[0], 0.6) && approx_eq(fx_tl[1], 0.4));
    }

    #[test]
    fn sprite_batch_queue_coalesces_and_preserves_capacity() {
        let mut queue = SpriteBatchQueue::with_capacity(32, 16);
        let initial_inst_cap = queue.instance_capacity();
        let initial_group_cap = queue.group_capacity();
        assert!(initial_inst_cap >= 32);
        assert!(initial_group_cap >= 16);

        let tex0 = TextureHandle(0);
        let tex1 = TextureHandle(1);

        for frame in 0..10 {
            queue.begin_frame();
            assert_eq!(queue.instances().len(), 0);
            assert_eq!(queue.groups().len(), 0);

            for i in 0..8 {
                let inst = SpriteInstance::new([i as f32 * 10.0, frame as f32], [32.0, 32.0]);
                queue.push(tex0, BlendMode::Normal, FilterMode::Linear, inst);
            }
            for i in 0..4 {
                let inst = SpriteInstance::new([i as f32 * 10.0, 100.0], [16.0, 16.0]);
                queue.push(tex1, BlendMode::Add, FilterMode::Nearest, inst);
            }

            assert_eq!(queue.instances().len(), 12);
            assert_eq!(queue.groups().len(), 2);
            assert_eq!(queue.groups()[0].instance_start, 0);
            assert_eq!(queue.groups()[0].instance_end, 8);
            assert_eq!(queue.groups()[1].instance_start, 8);
            assert_eq!(queue.groups()[1].instance_end, 12);
            assert_eq!(queue.instance_capacity(), initial_inst_cap);
            assert_eq!(queue.group_capacity(), initial_group_cap);
        }
    }
}
