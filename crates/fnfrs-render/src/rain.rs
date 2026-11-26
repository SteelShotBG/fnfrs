//! Fullscreen rain post-processing pass (`rain.wgsl`) mirroring V-Slice `RuntimeRainShader`.

use bytemuck::{Pod, Zeroable};

use crate::gpu::GpuContext;
use crate::shaders::create_rain_shader_module;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct RainUniformData {
    time: f32,
    scale: f32,
    intensity: f32,
    puddle_y: f32,
    rain_color: [f32; 4],
    screen_res: [f32; 2],
    cam_pos: [f32; 2],
    cam_zoom: f32,
    _pad: [f32; 3],
}

const _: () = assert!(std::mem::size_of::<RainUniformData>() == 64);

/// Pre-allocated offscreen render target + fullscreen rain post-process pipeline.
/// Allocates the offscreen target only at init and window resize (zero per-frame heap allocs).
pub struct RainPostPass {
    uniform_buffer: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    _target_tex: wgpu::Texture,
    target_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
}

impl RainPostPass {
    /// Initializes the rain post-processing pipeline matching `gpu.surface_format()`.
    #[must_use]
    pub fn new(gpu: &GpuContext) -> Self {
        let format = gpu.surface_format();
        let width = gpu.size.width.max(1);
        let height = gpu.size.height.max(1);

        let initial_uniforms = RainUniformData {
            time: 0.0,
            scale: 3.6,
            intensity: 0.0,
            puddle_y: 0.0,
            rain_color: [0.659, 0.678, 0.710, 1.0],
            screen_res: [1280.0, 720.0],
            cam_pos: [640.0, 360.0],
            cam_zoom: 1.0,
            _pad: [0.0; 3],
        };
        let uniform_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fnfrs-rain-uniforms"),
            size: std::mem::size_of::<RainUniformData>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue
            .write_buffer(&uniform_buffer, 0, bytemuck::bytes_of(&initial_uniforms));

        let bind_group_layout =
            gpu.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("fnfrs-rain-bgl"),
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

        let pipeline_layout =
            gpu.device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("fnfrs-rain-pipeline-layout"),
                    bind_group_layouts: &[Some(&bind_group_layout)],
                    immediate_size: 0,
                });

        let shader = create_rain_shader_module(&gpu.device);
        let pipeline = gpu
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("fnfrs-rain-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::REPLACE),
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

        let (target_tex, target_view, bind_group) = create_offscreen_target(
            gpu,
            &bind_group_layout,
            &uniform_buffer,
            format,
            width,
            height,
        );

        Self {
            uniform_buffer,
            bind_group_layout,
            pipeline,
            _target_tex: target_tex,
            target_view,
            bind_group,
            format,
            width,
            height,
        }
    }

    /// Recreates the offscreen target texture only when swapchain dimensions change.
    pub fn resize_if_needed(&mut self, gpu: &GpuContext) {
        let w = gpu.size.width.max(1);
        let h = gpu.size.height.max(1);
        if self.width == w && self.height == h {
            return;
        }
        let (tex, view, bg) = create_offscreen_target(
            gpu,
            &self.bind_group_layout,
            &self.uniform_buffer,
            self.format,
            w,
            h,
        );
        self._target_tex = tex;
        self.target_view = view;
        self.bind_group = bg;
        self.width = w;
        self.height = h;
    }

    /// Returns the offscreen world target `TextureView` for `camGame` rendering.
    #[inline]
    #[must_use]
    pub fn target_view(&self) -> &wgpu::TextureView {
        &self.target_view
    }

    /// Updates rain parameters on the GPU uniform buffer (zero heap allocations).
    #[allow(clippy::too_many_arguments)]
    pub fn update_uniforms(
        &self,
        queue: &wgpu::Queue,
        time: f32,
        scale: f32,
        intensity: f32,
        rain_color: [f32; 3],
        screen_res: [f32; 2],
        cam_pos: [f32; 2],
        cam_zoom: f32,
    ) {
        let data = RainUniformData {
            time,
            scale,
            intensity,
            puddle_y: 0.0,
            rain_color: [rain_color[0], rain_color[1], rain_color[2], 1.0],
            screen_res,
            cam_pos,
            cam_zoom,
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&data));
    }

    /// Draws the fullscreen rain post-process pass onto the active render pass.
    pub fn draw<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

fn create_offscreen_target(
    gpu: &GpuContext,
    bgl: &wgpu::BindGroupLayout,
    uniform_buffer: &wgpu::Buffer,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::BindGroup) {
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fnfrs-rain-scene-target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("fnfrs-rain-bg"),
        layout: bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&gpu.linear_sampler),
            },
        ],
    });
    (tex, view, bind_group)
}
