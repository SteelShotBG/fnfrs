//! Embedded WGSL shader sources and `wgpu::ShaderModule` builders.

use std::borrow::Cow;

/// Embedded WGSL source for the instanced 2D sprite shader.
pub const SPRITE_SHADER_SOURCE: &str = include_str!("shaders/sprite.wgsl");

/// Embedded WGSL source for the 2D sustain/triangle mesh shader.
pub const MESH_SHADER_SOURCE: &str = include_str!("shaders/mesh.wgsl");

/// Embedded WGSL source for the fullscreen `RuntimeRainShader` post-process pass.
pub const RAIN_SHADER_SOURCE: &str = include_str!("shaders/rain.wgsl");

/// Compiles the instanced sprite WGSL shader module on `device`.
#[must_use]
pub fn create_sprite_shader_module(device: &wgpu::Device) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fnfrs-sprite-shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SPRITE_SHADER_SOURCE)),
    })
}

/// Compiles the 2D triangle mesh WGSL shader module on `device`.
#[must_use]
pub fn create_mesh_shader_module(device: &wgpu::Device) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fnfrs-mesh-shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(MESH_SHADER_SOURCE)),
    })
}

/// Compiles the fullscreen rain post-processing WGSL shader module on `device`.
#[must_use]
pub fn create_rain_shader_module(device: &wgpu::Device) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fnfrs-rain-shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(RAIN_SHADER_SOURCE)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wgsl_sources_contain_expected_entry_points() {
        assert!(SPRITE_SHADER_SOURCE.contains("fn vs_main"));
        assert!(SPRITE_SHADER_SOURCE.contains("fn fs_main"));
        assert!(MESH_SHADER_SOURCE.contains("fn vs_main"));
        assert!(MESH_SHADER_SOURCE.contains("fn fs_main"));
        assert!(RAIN_SHADER_SOURCE.contains("fn vs_main"));
        assert!(RAIN_SHADER_SOURCE.contains("fn fs_main"));
    }
}
