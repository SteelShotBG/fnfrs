//! Zero-allocation 2D indexed triangle mesh batcher for V-Slice sustain trails
//! (`SustainTrail.hx` 8-column UV layout and bottom-clip vertex quads).

use bytemuck::{Pod, Zeroable};
use fnfrs_core::constants::{
    PIXELS_PER_MS, SUSTAIN_BOTTOM_CLIP, SUSTAIN_END_OFFSET, SUSTAIN_PIXEL_BOTTOM_CLIP,
    SUSTAIN_PIXEL_END_OFFSET, VIRTUAL_HEIGHT, VIRTUAL_WIDTH,
};

use crate::batch::{BlendMode, FilterMode};
use crate::gpu::{orthographic_projection, GpuContext, TextureHandle};
use crate::shaders::create_mesh_shader_module;

/// Triangle indices for one sustain trail (4 hold body vertices + 4 endcap
/// vertices = 2 quads = 4 triangles = 12 indices), matching
/// `SustainTrail.TRIANGLE_VERTEX_INDICES`.
pub const SUSTAIN_QUAD_INDICES: [u32; 12] = [0, 1, 2, 1, 2, 3, 4, 5, 6, 5, 6, 7];

/// Per-vertex GPU payload for textured, tinted 2D triangle meshes.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct MeshVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<MeshVertex>() == 32);

impl MeshVertex {
    /// Returns the `wgpu::VertexBufferLayout` for `MeshVertex`.
    #[must_use]
    pub const fn vertex_buffer_layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRIBS: [wgpu::VertexAttribute; 3] = [
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
        ];

        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MeshVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRIBS,
        }
    }
}

/// Contiguous range of mesh indices sharing texture, blend mode, and filter
/// mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshDrawGroup {
    pub texture: TextureHandle,
    pub blend: BlendMode,
    pub filter: FilterMode,
    pub index_start: u32,
    pub index_end: u32,
}

/// Parameters required to emit a V-Slice sustain trail matching
/// `SustainTrail.hx`.
#[derive(Debug, Clone, Copy)]
pub struct SustainTrailParams {
    /// Screen/world X coordinate of the trail's left edge.
    pub x: f32,
    /// Screen/world Y coordinate of the trail's anchor top (upscroll) or bottom
    /// reference (downscroll).
    pub y: f32,
    /// Lane index (`0..4`), selecting column pair `lane % 4` in the 8-column
    /// hold texture.
    pub lane: usize,
    /// Full sustain duration in milliseconds (`fullSustainLength`).
    pub full_sustain_ms: f64,
    /// Remaining sustain duration in milliseconds (`sustainLength`).
    pub sustain_ms: f64,
    /// Current song position in milliseconds.
    pub song_time_ms: f64,
    /// Note strum time in milliseconds.
    pub strum_time_ms: f64,
    /// Current scroll speed multiplier.
    pub scroll_speed: f64,
    /// Raw texture width in pixels.
    pub tex_width: f32,
    /// Raw texture height in pixels.
    pub tex_height: f32,
    /// Note style scale multiplier (`zoom`).
    pub zoom: f32,
    /// Uses pixel-art `endOffset = 1.0` / `bottomClip = 1.0` when `true`.
    pub is_pixel: bool,
    /// Downscroll mode flips vertical orientation.
    pub flip_y: bool,
    /// Vertex color tint `[r, g, b, a]`.
    pub color: [f32; 4],
}

/// Pre-allocated CPU mesh queue that collects vertices, indices, and coalesced
/// draw groups with zero per-frame heap allocations.
#[derive(Debug)]
pub struct MeshBatchQueue {
    vertices: Vec<MeshVertex>,
    indices: Vec<u32>,
    groups: Vec<MeshDrawGroup>,
}

impl MeshBatchQueue {
    /// Pre-allocates backing storage for vertices, indices, and draw groups.
    #[must_use]
    pub fn with_capacity(
        vertex_capacity: usize,
        index_capacity: usize,
        group_capacity: usize,
    ) -> Self {
        Self {
            vertices: Vec::with_capacity(vertex_capacity),
            indices: Vec::with_capacity(index_capacity),
            groups: Vec::with_capacity(group_capacity),
        }
    }

    /// Clears recorded geometry for the next frame without freeing or shrinking
    /// backing allocations.
    #[inline]
    pub fn begin_frame(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        self.groups.clear();
    }

    /// Appends arbitrary indexed triangles (where `local_indices` are relative
    /// to the start of `verts`), coalescing consecutive compatible groups.
    pub fn push_mesh(
        &mut self,
        texture: TextureHandle,
        blend: BlendMode,
        filter: FilterMode,
        verts: &[MeshVertex],
        local_indices: &[u32],
    ) {
        if verts.is_empty() || local_indices.is_empty() {
            return;
        }

        let base_vertex = self.vertices.len() as u32;
        let index_start = self.indices.len() as u32;

        self.vertices.extend_from_slice(verts);
        for &idx in local_indices {
            self.indices.push(base_vertex + idx);
        }
        let index_end = self.indices.len() as u32;

        self.append_group(texture, blend, filter, index_start, index_end);
    }

    /// Computes and appends the 8 vertices + 12 indices for a V-Slice sustain
    /// trail (`SustainTrail.hx` 8-column UV layout and bottom clipping) with zero
    /// heap allocations.
    pub fn push_sustain_trail(
        &mut self,
        texture: TextureHandle,
        blend: BlendMode,
        filter: FilterMode,
        params: &SustainTrailParams,
    ) {
        if params.tex_width <= 0.0 || params.tex_height <= 0.0 || params.zoom <= 0.0 {
            return;
        }

        let scroll = params.scroll_speed.max(0.0);
        let graphic_width = (params.tex_width / 8.0) * params.zoom;
        let graphic_height = (params.full_sustain_ms.max(0.0) * PIXELS_PER_MS * scroll) as f32;

        let elapsed_hold = params.song_time_ms - params.strum_time_ms;
        let remaining_ms = (params.sustain_ms - elapsed_hold).max(0.0);
        let clip_height =
            ((remaining_ms * PIXELS_PER_MS * scroll) as f32).clamp(0.0, graphic_height);

        if clip_height <= 0.1 {
            return;
        }

        let (end_offset, bottom_clip) = if params.is_pixel {
            (SUSTAIN_PIXEL_END_OFFSET, SUSTAIN_PIXEL_BOTTOM_CLIP)
        } else {
            (SUSTAIN_END_OFFSET, SUSTAIN_BOTTOM_CLIP)
        };

        let bottom_height = params.tex_height * params.zoom * end_offset;
        let part_height = clip_height - bottom_height;

        // Hold body vertex Y positions.
        let hold_top_y = if params.flip_y {
            clip_height
        } else {
            graphic_height - clip_height
        };
        let hold_bot_y = if part_height > 0.0 {
            if params.flip_y {
                bottom_height
            } else {
                hold_top_y + part_height
            }
        } else {
            hold_top_y
        };

        // 8-column horizontal UV slice: 4 lanes * 2 columns (hold + endcap).
        let u_hold_left = 0.25 * ((params.lane % 4) as f32);
        let u_hold_right = u_hold_left + 0.125;
        let v_hold_top = (-part_height) / params.tex_height / params.zoom;
        let v_hold_bot = 0.0;

        // Endcap vertex Y positions & UVs.
        let cap_top_y = hold_bot_y;
        let cap_bot_y = if params.flip_y {
            params.tex_height * (-bottom_clip + end_offset) * params.zoom
        } else {
            graphic_height + params.tex_height * (bottom_clip - end_offset) * params.zoom
        };

        let u_cap_left = u_hold_left + 0.125;
        let u_cap_right = u_cap_left + 0.125;
        let v_cap_top = if part_height > 0.0 {
            0.0
        } else {
            (bottom_height - clip_height) / params.zoom / params.tex_height
        };
        let v_cap_bot = bottom_clip;

        let x0 = params.x;
        let x1 = params.x + graphic_width;
        let y = params.y;
        let color = params.color;

        let verts = [
            // Hold body (0..4)
            MeshVertex {
                pos: [x0, y + hold_top_y],
                uv: [u_hold_left, v_hold_top],
                color,
            },
            MeshVertex {
                pos: [x1, y + hold_top_y],
                uv: [u_hold_right, v_hold_top],
                color,
            },
            MeshVertex {
                pos: [x0, y + hold_bot_y],
                uv: [u_hold_left, v_hold_bot],
                color,
            },
            MeshVertex {
                pos: [x1, y + hold_bot_y],
                uv: [u_hold_right, v_hold_bot],
                color,
            },
            // End cap (4..8)
            MeshVertex {
                pos: [x0, y + cap_top_y],
                uv: [u_cap_left, v_cap_top],
                color,
            },
            MeshVertex {
                pos: [x1, y + cap_top_y],
                uv: [u_cap_right, v_cap_top],
                color,
            },
            MeshVertex {
                pos: [x0, y + cap_bot_y],
                uv: [u_cap_left, v_cap_bot],
                color,
            },
            MeshVertex {
                pos: [x1, y + cap_bot_y],
                uv: [u_cap_right, v_cap_bot],
                color,
            },
        ];

        self.push_mesh(texture, blend, filter, &verts, &SUSTAIN_QUAD_INDICES);
    }

    fn append_group(
        &mut self,
        texture: TextureHandle,
        blend: BlendMode,
        filter: FilterMode,
        index_start: u32,
        index_end: u32,
    ) {
        if let Some(last) = self.groups.last_mut() {
            if last.texture == texture
                && last.blend == blend
                && last.filter == filter
                && last.index_end == index_start
            {
                last.index_end = index_end;
                return;
            }
        }
        self.groups.push(MeshDrawGroup {
            texture,
            blend,
            filter,
            index_start,
            index_end,
        });
    }

    /// Slice of queued vertices for the current frame.
    #[inline]
    #[must_use]
    pub fn vertices(&self) -> &[MeshVertex] {
        &self.vertices
    }

    /// Slice of queued indices for the current frame.
    #[inline]
    #[must_use]
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    /// Slice of coalesced mesh draw groups for the current frame.
    #[inline]
    #[must_use]
    pub fn groups(&self) -> &[MeshDrawGroup] {
        &self.groups
    }

    /// Allocated capacity of the CPU vertex `Vec`.
    #[inline]
    #[must_use]
    pub fn vertex_capacity(&self) -> usize {
        self.vertices.capacity()
    }

    /// Allocated capacity of the CPU index `Vec`.
    #[inline]
    #[must_use]
    pub fn index_capacity(&self) -> usize {
        self.indices.capacity()
    }

    /// Allocated capacity of the CPU draw-group `Vec`.
    #[inline]
    #[must_use]
    pub fn group_capacity(&self) -> usize {
        self.groups.capacity()
    }
}

/// GPU 2D triangle mesh batcher holding pre-allocated vertex/index pools,
/// dynamic GPU vertex/index buffers, camera uniform, and `Normal`/`Add`/`Screen`
/// render pipelines.
pub struct MeshBatcher {
    queue: MeshBatchQueue,
    vertex_buffer: wgpu::Buffer,
    vertex_buffer_capacity: usize,
    index_buffer: wgpu::Buffer,
    index_buffer_capacity: usize,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    pipeline_normal: wgpu::RenderPipeline,
    pipeline_add: wgpu::RenderPipeline,
    pipeline_screen: wgpu::RenderPipeline,
}

impl MeshBatcher {
    /// Creates a new `MeshBatcher` with pre-allocated CPU and GPU buffers.
    #[must_use]
    pub fn new(gpu: &GpuContext, vertex_capacity: usize, index_capacity: usize) -> Self {
        let v_cap = vertex_capacity.max(256);
        let i_cap = index_capacity.max(384);
        let g_cap = (i_cap / 12).max(64);
        let queue = MeshBatchQueue::with_capacity(v_cap, i_cap, g_cap);

        let vertex_buffer = create_vertex_buffer(&gpu.device, v_cap);
        let index_buffer = create_index_buffer(&gpu.device, i_cap);

        let proj = orthographic_projection(VIRTUAL_WIDTH, VIRTUAL_HEIGHT).to_cols_array_2d();
        let camera_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fnfrs-mesh-camera-uniform"),
            size: std::mem::size_of::<[[f32; 4]; 4]>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue
            .write_buffer(&camera_buffer, 0, bytemuck::cast_slice(&proj));

        let camera_bind_group_layout =
            gpu.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("fnfrs-mesh-camera-bgl"),
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
            label: Some("fnfrs-mesh-camera-bg"),
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout =
            gpu.device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("fnfrs-mesh-pipeline-layout"),
                    bind_group_layouts: &[
                        Some(&camera_bind_group_layout),
                        Some(&gpu.texture_bind_group_layout),
                    ],
                    immediate_size: 0,
                });

        let shader = create_mesh_shader_module(&gpu.device);
        let format = gpu.surface_format();

        let pipeline_normal = create_mesh_pipeline(
            &gpu.device,
            &pipeline_layout,
            &shader,
            format,
            BlendMode::Normal,
            "fnfrs-mesh-pipeline-normal",
        );
        let pipeline_add = create_mesh_pipeline(
            &gpu.device,
            &pipeline_layout,
            &shader,
            format,
            BlendMode::Add,
            "fnfrs-mesh-pipeline-add",
        );
        let pipeline_screen = create_mesh_pipeline(
            &gpu.device,
            &pipeline_layout,
            &shader,
            format,
            BlendMode::Screen,
            "fnfrs-mesh-pipeline-screen",
        );

        Self {
            queue,
            vertex_buffer,
            vertex_buffer_capacity: v_cap,
            index_buffer,
            index_buffer_capacity: i_cap,
            camera_buffer,
            camera_bind_group,
            pipeline_normal,
            pipeline_add,
            pipeline_screen,
        }
    }

    /// Clears CPU mesh lists for a new frame without freeing capacity.
    #[inline]
    pub fn begin_frame(&mut self) {
        self.queue.begin_frame();
    }

    /// Updates the camera projection uniform matrix.
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

    /// Pushes indexed mesh geometry for the current frame.
    #[inline]
    pub fn push_mesh(
        &mut self,
        texture: TextureHandle,
        blend: BlendMode,
        filter: FilterMode,
        verts: &[MeshVertex],
        local_indices: &[u32],
    ) {
        self.queue
            .push_mesh(texture, blend, filter, verts, local_indices);
    }

    /// Pushes a V-Slice sustain trail quad pair for the current frame.
    #[inline]
    pub fn push_sustain_trail(
        &mut self,
        texture: TextureHandle,
        blend: BlendMode,
        filter: FilterMode,
        params: &SustainTrailParams,
    ) {
        self.queue
            .push_sustain_trail(texture, blend, filter, params);
    }

    /// Returns the number of queued vertices in the current frame.
    #[inline]
    #[must_use]
    pub fn vertex_count(&self) -> usize {
        self.queue.vertices().len()
    }

    /// Returns the number of queued indices in the current frame.
    #[inline]
    #[must_use]
    pub fn index_count(&self) -> usize {
        self.queue.indices().len()
    }

    /// Uploads queued mesh geometry to the GPU vertex and index buffers,
    /// reallocating GPU buffers only if capacity is exceeded.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let verts = self.queue.vertices();
        let indices = self.queue.indices();
        if verts.is_empty() || indices.is_empty() {
            return;
        }

        if verts.len() > self.vertex_buffer_capacity {
            let new_cap = verts.len().next_power_of_two();
            self.vertex_buffer = create_vertex_buffer(device, new_cap);
            self.vertex_buffer_capacity = new_cap;
        }
        if indices.len() > self.index_buffer_capacity {
            let new_cap = indices.len().next_power_of_two();
            self.index_buffer = create_index_buffer(device, new_cap);
            self.index_buffer_capacity = new_cap;
        }

        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(verts));
        queue.write_buffer(&self.index_buffer, 0, bytemuck::cast_slice(indices));
    }

    /// Records all queued mesh draw calls into `render_pass` using
    /// `repeat_v = true` samplers for seamless sustain tiling.
    pub fn render(&self, render_pass: &mut wgpu::RenderPass<'_>, gpu: &GpuContext) {
        let groups = self.queue.groups();
        if groups.is_empty() {
            return;
        }

        render_pass.set_bind_group(0, &self.camera_bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

        let mut active_blend: Option<BlendMode> = None;

        for group in groups {
            let Some(tex_bg) = gpu.texture_bind_group(group.texture, group.filter, true) else {
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
            render_pass.draw_indexed(group.index_start..group.index_end, 0, 0..1);
        }
    }
}

fn create_vertex_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    let byte_size = (capacity * std::mem::size_of::<MeshVertex>()) as wgpu::BufferAddress;
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fnfrs-mesh-vertex-buffer"),
        size: byte_size.max(32),
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_index_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    let byte_size = (capacity * std::mem::size_of::<u32>()) as wgpu::BufferAddress;
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fnfrs-mesh-index-buffer"),
        size: byte_size.max(16),
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_mesh_pipeline(
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
            buffers: &[Some(MeshVertex::vertex_buffer_layout())],
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
    fn mesh_vertex_layout_and_alignment() {
        assert_eq!(std::mem::size_of::<MeshVertex>(), 32);
        assert_eq!(std::mem::align_of::<MeshVertex>(), 4);
        assert!(std::mem::size_of::<MeshVertex>().is_multiple_of(16));
        assert_eq!(bytemuck::offset_of!(MeshVertex, pos), 0);
        assert_eq!(bytemuck::offset_of!(MeshVertex, uv), 8);
        assert_eq!(bytemuck::offset_of!(MeshVertex, color), 16);
    }

    #[test]
    fn sustain_trail_8_column_uv_and_zero_realloc() {
        let mut queue = MeshBatchQueue::with_capacity(64, 96, 16);
        let v_cap = queue.vertex_capacity();
        let i_cap = queue.index_capacity();
        let g_cap = queue.group_capacity();

        let tex = TextureHandle(2);

        for _ in 0..8 {
            queue.begin_frame();
            for lane in 0..4 {
                let params = SustainTrailParams {
                    x: 100.0 + lane as f32 * 112.0,
                    y: 50.0,
                    lane,
                    full_sustain_ms: 800.0,
                    sustain_ms: 800.0,
                    song_time_ms: 1000.0,
                    strum_time_ms: 1000.0,
                    scroll_speed: 1.0,
                    tex_width: 400.0,
                    tex_height: 80.0,
                    zoom: 1.0,
                    is_pixel: false,
                    flip_y: false,
                    color: [1.0, 1.0, 1.0, 0.8],
                };
                queue.push_sustain_trail(tex, BlendMode::Normal, FilterMode::Linear, &params);
            }

            assert_eq!(queue.vertices().len(), 32);
            assert_eq!(queue.indices().len(), 48);
            assert_eq!(queue.groups().len(), 1);
            assert_eq!(queue.vertex_capacity(), v_cap);
            assert_eq!(queue.index_capacity(), i_cap);
            assert_eq!(queue.group_capacity(), g_cap);
        }

        // Verify lane 2 (3rd lane -> columns 4 and 5: U in [0.50..0.625] and [0.625..0.75]).
        let lane2_base = 16;
        assert!(approx_eq(queue.vertices()[lane2_base].uv[0], 0.5));
        assert!(approx_eq(queue.vertices()[lane2_base + 1].uv[0], 0.625));
        assert!(approx_eq(queue.vertices()[lane2_base + 4].uv[0], 0.625));
        assert!(approx_eq(queue.vertices()[lane2_base + 5].uv[0], 0.75));
        assert!(approx_eq(
            queue.vertices()[lane2_base + 6].uv[1],
            SUSTAIN_BOTTOM_CLIP
        ));
    }
}
