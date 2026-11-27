//! `wgpu` device, surface, texture registry, samplers, and virtual canvas
//! viewport math.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use rustc_hash::FxHashMap;
use thiserror::Error;
use winit::dpi::PhysicalSize;
use winit::window::Window;

/// Texture filtering mode used when sampling sprites and meshes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FilterMode {
    Nearest,
    #[default]
    Linear,
}

/// Opaque handle to a GPU texture registered inside [`GpuContext`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureHandle(pub u32);

/// Screen-space rectangle in physical pixels for letterboxing the `1280x720`
/// virtual canvas inside a window of arbitrary dimensions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Errors returned during GPU initialization, texture upload, or frame setup.
#[derive(Debug, Error)]
pub enum GpuError {
    #[error("failed to create wgpu surface: {0}")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),

    #[error("failed to request a compatible GPU adapter: {0}")]
    RequestAdapter(#[from] wgpu::RequestAdapterError),

    #[error("failed to create logical GPU device: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),

    #[error("surface reported no supported texture formats")]
    SurfaceUnsupported,

    #[error("invalid texture dimensions {0}x{1}")]
    InvalidDimensions(u32, u32),

    #[error("RGBA8 byte length mismatch: expected {expected} bytes, got {actual}")]
    RgbaByteLengthMismatch { expected: usize, actual: usize },

    #[error("failed to read image file: {0}")]
    Io(#[from] std::io::Error),

    #[error("failed to decode PNG image: {0}")]
    ImageDecode(#[from] image::ImageError),

    #[error("invalid texture handle {0}")]
    InvalidTextureHandle(u32),

    #[error("wgpu surface was lost and must be recreated")]
    SurfaceLost,
}

struct TextureEntry {
    _texture: wgpu::Texture,
    _view: wgpu::TextureView,
    width: u32,
    height: u32,
    pinned: bool,
    bind_group_nearest_clamp: wgpu::BindGroup,
    bind_group_linear_clamp: wgpu::BindGroup,
    bind_group_nearest_repeat: wgpu::BindGroup,
    bind_group_linear_repeat: wgpu::BindGroup,
}

/// Owns the `wgpu` instance, surface, adapter, device, queue, cached samplers,
/// and GPU texture registry.
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub surface: wgpu::Surface<'static>,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub size: PhysicalSize<u32>,
    pub nearest_sampler: wgpu::Sampler,
    pub linear_sampler: wgpu::Sampler,
    pub nearest_repeat_sampler: wgpu::Sampler,
    pub linear_repeat_sampler: wgpu::Sampler,
    pub texture_bind_group_layout: wgpu::BindGroupLayout,
    textures: Vec<Option<TextureEntry>>,
    free_texture_slots: Vec<u32>,
    png_cache: FxHashMap<PathBuf, TextureHandle>,
    white_texture: TextureHandle,
}

impl GpuContext {
    /// Initializes `wgpu` synchronously on `window` using `pollster::block_on`.
    pub fn new(window: Arc<Window>) -> Result<Self, GpuError> {
        let size = window.inner_size();

        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        if std::env::var_os("WGPU_BACKEND").is_none() {
            // Default to Vulkan across desktop & mobile so only one low-overhead
            // driver heap is loaded and shader behavior matches Android.
            instance_desc.backends = wgpu::Backends::VULKAN;
        }

        let mut instance = wgpu::Instance::new(instance_desc);
        let mut surface = instance.create_surface(Arc::clone(&window))?;

        let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::None,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        })) {
            Ok(adapter) => adapter,
            Err(_) => {
                let mut fallback_desc =
                    wgpu::InstanceDescriptor::new_without_display_handle_from_env();
                if std::env::var_os("WGPU_BACKEND").is_none() {
                    fallback_desc.backends = wgpu::Backends::PRIMARY;
                }
                instance = wgpu::Instance::new(fallback_desc);
                surface = instance.create_surface(window)?;
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::None,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: false,
                    apply_limit_buckets: false,
                }))?
            }
        };

        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("fnfrs-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                ..Default::default()
            }))?;

        let caps = surface.get_capabilities(&adapter);
        if caps.formats.is_empty() {
            return Err(GpuError::SurfaceUnsupported);
        }

        // Use a non-sRGB surface/texture pipeline matching OpenFL/HaxeFlixel's
        // native sRGB-domain blending (so `#66FF33` and bitmap text outlines match 1:1).
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Fifo) {
            wgpu::PresentMode::Fifo
        } else {
            caps.present_modes
                .first()
                .copied()
                .unwrap_or(wgpu::PresentMode::Fifo)
        };

        let alpha_mode = caps
            .alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        let nearest_sampler = create_sampler(
            &device,
            "fnfrs-nearest-clamp",
            wgpu::FilterMode::Nearest,
            wgpu::AddressMode::ClampToEdge,
        );
        let linear_sampler = create_sampler(
            &device,
            "fnfrs-linear-clamp",
            wgpu::FilterMode::Linear,
            wgpu::AddressMode::ClampToEdge,
        );
        let nearest_repeat_sampler = create_sampler(
            &device,
            "fnfrs-nearest-repeat",
            wgpu::FilterMode::Nearest,
            wgpu::AddressMode::Repeat,
        );
        let linear_repeat_sampler = create_sampler(
            &device,
            "fnfrs-linear-repeat",
            wgpu::FilterMode::Linear,
            wgpu::AddressMode::Repeat,
        );

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("fnfrs-texture-bind-group-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let mut ctx = Self {
            instance,
            surface,
            adapter,
            device,
            queue,
            config,
            size,
            nearest_sampler,
            linear_sampler,
            nearest_repeat_sampler,
            linear_repeat_sampler,
            texture_bind_group_layout,
            textures: Vec::new(),
            free_texture_slots: Vec::new(),
            png_cache: FxHashMap::default(),
            white_texture: TextureHandle(0),
        };

        let white_handle = ctx.create_solid_white_texture()?;
        ctx.white_texture = white_handle;

        Ok(ctx)
    }

    /// Reconfigures the swapchain surface for a new physical window size.
    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        self.size = new_size;
        if new_size.width > 0 && new_size.height > 0 {
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Returns the configured surface pixel format.
    #[inline]
    #[must_use]
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// Returns the built-in 1x1 solid white texture handle created at init.
    #[inline]
    #[must_use]
    pub fn white_texture(&self) -> TextureHandle {
        self.white_texture
    }

    /// Computes the centered `1280x720` letterboxed viewport for the current
    /// window size.
    #[inline]
    #[must_use]
    pub fn letterbox_viewport(&self) -> ViewportRect {
        compute_letterbox_viewport(self.size.width, self.size.height)
    }

    /// Uploads raw RGBA8 pixel data to a new GPU texture and pre-builds its
    /// sampler bind groups so hot-loop rendering never allocates.
    pub fn create_texture_rgba8(
        &mut self,
        label: Option<&str>,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<TextureHandle, GpuError> {
        if width == 0 || height == 0 {
            return Err(GpuError::InvalidDimensions(width, height));
        }
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or(GpuError::InvalidDimensions(width, height))?;
        if rgba.len() != expected {
            return Err(GpuError::RgbaByteLengthMismatch {
                expected,
                actual: rgba.len(),
            });
        }

        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label,
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            extent,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group_nearest_clamp = self.create_texture_bind_group(
            label,
            &view,
            &self.nearest_sampler,
        );
        let bind_group_linear_clamp = self.create_texture_bind_group(
            label,
            &view,
            &self.linear_sampler,
        );
        let bind_group_nearest_repeat = self.create_texture_bind_group(
            label,
            &view,
            &self.nearest_repeat_sampler,
        );
        let bind_group_linear_repeat = self.create_texture_bind_group(
            label,
            &view,
            &self.linear_repeat_sampler,
        );

        let entry = TextureEntry {
            _texture: texture,
            _view: view,
            width,
            height,
            pinned: false,
            bind_group_nearest_clamp,
            bind_group_linear_clamp,
            bind_group_nearest_repeat,
            bind_group_linear_repeat,
        };

        let index = if let Some(reused) = self.free_texture_slots.pop() {
            self.textures[reused as usize] = Some(entry);
            reused
        } else {
            let idx = u32::try_from(self.textures.len())
                .map_err(|_| GpuError::InvalidTextureHandle(u32::MAX))?;
            self.textures.push(Some(entry));
            idx
        };

        Ok(TextureHandle(index))
    }

    /// Marks `handle` as persistent so [`clear_user_textures`] preserves it across screen transitions.
    pub fn pin_texture(&mut self, handle: TextureHandle) {
        if let Some(Some(entry)) = self.textures.get_mut(handle.0 as usize) {
            entry.pinned = true;
        }
    }

    /// Unloads a GPU texture and recycles its slot index (preserving `white_texture` and pinned textures).
    pub fn unload_texture(&mut self, handle: TextureHandle) -> bool {
        if handle == self.white_texture {
            return false;
        }
        let Some(slot) = self.textures.get_mut(handle.0 as usize) else {
            return false;
        };
        if slot.as_ref().is_some_and(|e| e.pinned) {
            return false;
        }
        if slot.take().is_some() {
            self.free_texture_slots.push(handle.0);
            self.png_cache.retain(|_, &mut h| h != handle);
            true
        } else {
            false
        }
    }

    /// Releases all non-pinned textures except `white_texture` so screen/song
    /// transitions reclaim GPU VRAM without forcing reloads of persistent UI assets.
    pub fn clear_user_textures(&mut self) {
        self.free_texture_slots.clear();
        for (idx, slot) in self.textures.iter_mut().enumerate() {
            if idx as u32 == self.white_texture.0 || slot.as_ref().is_some_and(|e| e.pinned) {
                continue;
            }
            *slot = None;
            self.free_texture_slots.push(idx as u32);
        }
        let textures = &self.textures;
        self.png_cache.retain(|_, &mut handle| {
            textures
                .get(handle.0 as usize)
                .and_then(|s| s.as_ref())
                .is_some()
        });
    }

    /// Creates a 1x1 opaque white RGBA8 texture (`[255, 255, 255, 255]`).
    pub fn create_solid_white_texture(&mut self) -> Result<TextureHandle, GpuError> {
        self.create_texture_rgba8(Some("fnfrs-solid-white"), 1, 1, &[255, 255, 255, 255])
    }

    /// Decodes PNG bytes using the `image` crate and uploads them as an RGBA8
    /// texture.
    pub fn from_png_bytes(
        &mut self,
        label: Option<&str>,
        bytes: &[u8],
    ) -> Result<TextureHandle, GpuError> {
        let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)?;
        let rgba = decoded.to_rgba8();
        let (width, height) = rgba.dimensions();
        self.create_texture_rgba8(label, width, height, rgba.as_raw())
    }

    /// Uploads a PNG from `path`, returning an already-cached [`TextureHandle`] if
    /// the same path was already uploaded during the current screen/stage lifetime.
    pub fn from_png_path_cached(
        &mut self,
        label: Option<&str>,
        path: &Path,
    ) -> Result<TextureHandle, GpuError> {
        if let Some(&handle) = self.png_cache.get(path) {
            if self
                .textures
                .get(handle.0 as usize)
                .and_then(|s| s.as_ref())
                .is_some()
            {
                return Ok(handle);
            }
        }
        let bytes = std::fs::read(path)?;
        let handle = self.from_png_bytes(label, &bytes)?;
        self.png_cache.insert(path.to_path_buf(), handle);
        Ok(handle)
    }

    /// Returns `(width, height)` in pixels for `handle`, if valid.
    #[inline]
    #[must_use]
    pub fn texture_size(&self, handle: TextureHandle) -> Option<(u32, u32)> {
        self.textures
            .get(handle.0 as usize)?
            .as_ref()
            .map(|entry| (entry.width, entry.height))
    }

    /// Updates the pixel buffer of an existing RGBA8 GPU texture in-place without reallocation.
    pub fn update_texture_rgba8(&self, handle: TextureHandle, rgba: &[u8]) -> Result<(), GpuError> {
        let entry = self
            .textures
            .get(handle.0 as usize)
            .and_then(|s| s.as_ref())
            .ok_or(GpuError::InvalidTextureHandle(handle.0))?;
        let expected = (entry.width as usize) * (entry.height as usize) * 4;
        if rgba.len() != expected {
            return Err(GpuError::RgbaByteLengthMismatch {
                expected,
                actual: rgba.len(),
            });
        }
        let extent = wgpu::Extent3d {
            width: entry.width,
            height: entry.height,
            depth_or_array_layers: 1,
        };
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &entry._texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * entry.width),
                rows_per_image: Some(entry.height),
            },
            extent,
        );
        Ok(())
    }

    /// Returns the number of live GPU textures currently allocated.
    #[must_use]
    pub fn active_texture_count(&self) -> usize {
        self.textures.iter().filter(|entry| entry.is_some()).count()
    }

    /// Returns the pre-built `wgpu::BindGroup` for `handle`, `filter`, and wrap
    /// mode (`repeat_v = true` for vertically tiled sustain trails).
    #[inline]
    #[must_use]
    pub fn texture_bind_group(
        &self,
        handle: TextureHandle,
        filter: FilterMode,
        repeat_v: bool,
    ) -> Option<&wgpu::BindGroup> {
        let entry = self.textures.get(handle.0 as usize)?.as_ref()?;
        Some(match (filter, repeat_v) {
            (FilterMode::Nearest, false) => &entry.bind_group_nearest_clamp,
            (FilterMode::Linear, false) => &entry.bind_group_linear_clamp,
            (FilterMode::Nearest, true) => &entry.bind_group_nearest_repeat,
            (FilterMode::Linear, true) => &entry.bind_group_linear_repeat,
        })
    }

    /// Acquires the next swapchain texture, automatically reconfiguring if
    /// outdated and returning `Ok(None)` when the frame should be skipped
    /// (minimized, occluded, timed out, or reconfigured).
    pub fn acquire_frame(&mut self) -> Result<Option<wgpu::SurfaceTexture>, GpuError> {
        if self.size.width == 0 || self.size.height == 0 {
            return Ok(None);
        }

        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => Ok(Some(frame)),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                drop(frame);
                self.surface.configure(&self.device, &self.config);
                Ok(None)
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                Ok(None)
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => Ok(None),
            wgpu::CurrentSurfaceTexture::Lost => Err(GpuError::SurfaceLost),
        }
    }

    fn create_texture_bind_group(
        &self,
        label: Option<&str>,
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label,
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }
}

fn create_sampler(
    device: &wgpu::Device,
    label: &str,
    filter: wgpu::FilterMode,
    address_mode: wgpu::AddressMode,
) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some(label),
        address_mode_u: address_mode,
        address_mode_v: address_mode,
        address_mode_w: address_mode,
        mag_filter: filter,
        min_filter: filter,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        lod_min_clamp: 0.0,
        lod_max_clamp: 32.0,
        compare: None,
        anisotropy_clamp: 1,
        border_color: None,
    })
}

/// Computes a centered aspect-preserving viewport for the `1280x720` virtual
/// canvas inside a physical surface of `width x height`.
#[must_use]
pub fn compute_letterbox_viewport(width: u32, height: u32) -> ViewportRect {
    if width == 0 || height == 0 {
        return ViewportRect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        };
    }

    let win_w = width as f32;
    let win_h = height as f32;
    let target_aspect = VIRTUAL_WIDTH / VIRTUAL_HEIGHT;
    let window_aspect = win_w / win_h;

    if window_aspect > target_aspect {
        let vp_h = win_h;
        let vp_w = vp_h * target_aspect;
        ViewportRect {
            x: (win_w - vp_w) * 0.5,
            y: 0.0,
            width: vp_w,
            height: vp_h,
        }
    } else {
        let vp_w = win_w;
        let vp_h = vp_w / target_aspect;
        ViewportRect {
            x: 0.0,
            y: (win_h - vp_h) * 0.5,
            width: vp_w,
            height: vp_h,
        }
    }
}

/// Builds a 2D orthographic projection matrix with `(0, 0)` at top-left and
/// `(width, height)` at bottom-right in `wgpu` `[0, 1]` NDC depth.
#[inline]
#[must_use]
pub fn orthographic_projection(width: f32, height: f32) -> glam::Mat4 {
    let sx = 2.0 / width;
    let sy = -2.0 / height;
    glam::Mat4::from_cols(
        glam::Vec4::new(sx, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, sy, 0.0, 0.0),
        glam::Vec4::new(0.0, 0.0, -0.5, 0.0),
        glam::Vec4::new(-1.0, 1.0, 0.5, 1.0),
    )
}

/// Builds a 2D orthographic projection matrix with centered camera zoom around
/// `(width * 0.5, height * 0.5)`.
#[inline]
#[must_use]
pub fn zoomed_orthographic_projection(width: f32, height: f32, zoom: f32) -> glam::Mat4 {
    glam::Mat4::from_scale(glam::Vec3::new(zoom, zoom, 1.0))
        * orthographic_projection(width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn letterbox_viewport_exact_16_9() {
        let vp = compute_letterbox_viewport(1280, 720);
        assert!(approx_eq(vp.x, 0.0));
        assert!(approx_eq(vp.y, 0.0));
        assert!(approx_eq(vp.width, 1280.0));
        assert!(approx_eq(vp.height, 720.0));
    }

    #[test]
    fn letterbox_viewport_ultrawide_pillarboxes() {
        let vp = compute_letterbox_viewport(2560, 1080);
        assert!(approx_eq(vp.height, 1080.0));
        assert!(approx_eq(vp.width, 1920.0));
        assert!(approx_eq(vp.x, 320.0));
        assert!(approx_eq(vp.y, 0.0));
    }

    #[test]
    fn letterbox_viewport_tall_letterboxes() {
        let vp = compute_letterbox_viewport(1280, 1024);
        assert!(approx_eq(vp.width, 1280.0));
        assert!(approx_eq(vp.height, 720.0));
        assert!(approx_eq(vp.x, 0.0));
        assert!(approx_eq(vp.y, 152.0));
    }

    #[test]
    fn letterbox_viewport_zero_dimensions() {
        let vp = compute_letterbox_viewport(0, 720);
        assert!(approx_eq(vp.width, 0.0));
        assert!(approx_eq(vp.height, 0.0));
    }

    #[test]
    fn orthographic_projection_maps_virtual_corners_to_ndc() {
        let proj = orthographic_projection(VIRTUAL_WIDTH, VIRTUAL_HEIGHT);
        let top_left = proj * glam::Vec4::new(0.0, 0.0, 0.0, 1.0);
        let bottom_right = proj * glam::Vec4::new(VIRTUAL_WIDTH, VIRTUAL_HEIGHT, 0.0, 1.0);
        assert!(approx_eq(top_left.x, -1.0));
        assert!(approx_eq(top_left.y, 1.0));
        assert!(top_left.z >= 0.0 && top_left.z <= 1.0);
        assert!(approx_eq(bottom_right.x, 1.0));
        assert!(approx_eq(bottom_right.y, -1.0));
        assert!(bottom_right.z >= 0.0 && bottom_right.z <= 1.0);
    }
}
