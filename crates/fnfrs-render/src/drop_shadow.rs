//! Per-sprite drop-shadow / rim-light shader parameters (`DropShadowShader.hx`).

use bytemuck::{Pod, Zeroable};

use crate::gpu::TextureHandle;

/// High-level rim-light / drop-shadow configuration matching `DropShadowShader.hx`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropShadowParams {
    /// Rim / drop-shadow RGB color in `[0.0, 1.0]`.
    pub color: [f32; 3],
    /// Light angle in degrees (`0` = right, `90` = up, `180` = left, `270` = down).
    pub angle_deg: f32,
    /// Offset distance in texels.
    pub distance: f32,
    /// Shadow alpha multiplier (default `1.0`).
    pub strength: f32,
    /// Base luma threshold below which pixels do not receive rim light (default `0.1`).
    pub threshold: f32,
    /// Alternate luma threshold used where `alt_mask_tex` blue channel > 0.
    pub mask_threshold: f32,
    /// Whether `alt_mask_tex` is active.
    pub use_alt_mask: bool,
    /// Optional alternate mask texture (`blue > 0` selects `mask_threshold`).
    pub alt_mask_tex: Option<TextureHandle>,
    /// Raw Flash `AdjustColor` `[hue_deg, saturation, brightness, contrast]`.
    pub adjust_color: [f32; 4],
}

impl Default for DropShadowParams {
    fn default() -> Self {
        Self {
            color: [1.0, 1.0, 1.0],
            angle_deg: 0.0,
            distance: 15.0,
            strength: 1.0,
            threshold: 0.1,
            mask_threshold: 0.0,
            use_alt_mask: false,
            alt_mask_tex: None,
            adjust_color: [0.0; 4],
        }
    }
}

impl DropShadowParams {
    /// Converts `0xRRGGBB` / `0xAARRGGBB` hex integer into normalized `[r, g, b]`.
    #[inline]
    #[must_use]
    pub const fn color_from_hex(rgb: u32) -> [f32; 3] {
        [
            ((rgb >> 16) & 0xFF) as f32 / 255.0,
            ((rgb >> 8) & 0xFF) as f32 / 255.0,
            (rgb & 0xFF) as f32 / 255.0,
        ]
    }

    /// Converts to the 64-byte GPU uniform representation.
    #[must_use]
    pub fn to_uniform(&self) -> DropShadowUniform {
        let [hue_deg, sat, bright, contrast] = self.adjust_color;
        let hue_rad = hue_deg.to_radians();
        let sat_mult = 1.0 + (if sat > 0.0 { sat * 3.0 } else { sat }) / 100.0;
        let bright_add = bright / 255.0;
        let mut c = 1.0 + contrast / 100.0;
        if c > 1.0 {
            c = (((0.008_522_59 * (4.764_54 * (c - 1.0)).exp()) * 1.01) - 0.008_607_816) * 10.0
                + 1.0;
        }
        DropShadowUniform {
            drop_color: [self.color[0], self.color[1], self.color[2], self.strength],
            params: [
                self.angle_deg.to_radians(),
                self.distance,
                self.threshold,
                self.mask_threshold,
            ],
            hsbc: [hue_rad, sat_mult, bright_add, c],
            flags: [u32::from(self.use_alt_mask && self.alt_mask_tex.is_some()), 0, 0, 0],
        }
    }
}

/// GPU uniform layout for `drop_shadow.wgsl` (`@group(2) @binding(0)`), 64 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct DropShadowUniform {
    pub drop_color: [f32; 4],
    pub params: [f32; 4],
    pub hsbc: [f32; 4],
    pub flags: [u32; 4],
}

const _: () = assert!(std::mem::size_of::<DropShadowUniform>() == 64);

const INITIAL_DROP_SHADOW_SLOTS: usize = 32;

struct DropShadowSlot {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// Pre-allocated GPU uniform pool and render pipeline for `DropShadowShader`.
pub(crate) struct DropShadowBatchPool {
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline_normal: wgpu::RenderPipeline,
    slots: Vec<DropShadowSlot>,
    queued: Vec<DropShadowParams>,
}

impl DropShadowBatchPool {
    pub(crate) fn new(
        device: &wgpu::Device,
        camera_bgl: &wgpu::BindGroupLayout,
        texture_bgl: &wgpu::BindGroupLayout,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("fnfrs-drop-shadow-bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fnfrs-drop-shadow-pipeline-layout"),
            bind_group_layouts: &[
                Some(camera_bgl),
                Some(texture_bgl),
                Some(&bind_group_layout),
                Some(texture_bgl),
            ],
            immediate_size: 0,
        });

        let shader = crate::shaders::create_drop_shadow_shader_module(device);
        let pipeline_normal = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fnfrs-drop-shadow-pipeline-normal"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(crate::batch::SpriteInstance::vertex_buffer_layout())],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
        });

        let mut slots = Vec::with_capacity(INITIAL_DROP_SHADOW_SLOTS);
        for _ in 0..INITIAL_DROP_SHADOW_SLOTS {
            slots.push(create_slot(device, &bind_group_layout));
        }

        Self {
            bind_group_layout,
            pipeline_normal,
            slots,
            queued: Vec::with_capacity(INITIAL_DROP_SHADOW_SLOTS),
        }
    }

    #[inline]
    pub(crate) fn begin_frame(&mut self) {
        self.queued.clear();
    }

    #[inline]
    pub(crate) fn push(&mut self, params: DropShadowParams) -> u32 {
        if let Some((idx, _)) = self
            .queued
            .iter()
            .enumerate()
            .find(|(_, existing)| **existing == params)
        {
            return idx as u32;
        }
        let idx = self.queued.len() as u32;
        self.queued.push(params);
        idx
    }

    pub(crate) fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        while self.slots.len() < self.queued.len() {
            self.slots.push(create_slot(device, &self.bind_group_layout));
        }
        for (idx, params) in self.queued.iter().enumerate() {
            let uniform = params.to_uniform();
            queue.write_buffer(&self.slots[idx].buffer, 0, bytemuck::bytes_of(&uniform));
        }
    }

    #[inline]
    pub(crate) fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline_normal
    }

    #[inline]
    pub(crate) fn bind_group(&self, slot_idx: u32) -> Option<&wgpu::BindGroup> {
        self.slots.get(slot_idx as usize).map(|s| &s.bind_group)
    }

    #[inline]
    pub(crate) fn params(&self, slot_idx: u32) -> Option<&DropShadowParams> {
        self.queued.get(slot_idx as usize)
    }
}

fn create_slot(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> DropShadowSlot {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fnfrs-drop-shadow-uniform"),
        size: std::mem::size_of::<DropShadowUniform>() as wgpu::BufferAddress,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("fnfrs-drop-shadow-bg"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }],
    });
    DropShadowSlot { buffer, bind_group }
}
