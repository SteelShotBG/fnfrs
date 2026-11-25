use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use fnfrs_audio::AudioEngine;
use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{
    format_hud_score, BitmapFont, BlendMode, FilterMode, GpuContext, GpuError, SpriteBatcher,
    SpriteInstance, StackTextBuffer, TextAlign, TextStyle, TextureHandle,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

struct RenderState {
    window: Arc<Window>,
    gpu: GpuContext,
    batcher: SpriteBatcher,
    hud_font: BitmapFont,
    hud_font_tex: TextureHandle,
    start_time: Instant,
    frame_count: u64,
    audio_status: StackTextBuffer<96>,
    assets_status: StackTextBuffer<128>,
    score_buf: StackTextBuffer<96>,
    fps_buf: StackTextBuffer<64>,
}

struct App {
    _audio: Option<AudioEngine>,
    audio_status: StackTextBuffer<96>,
    assets_status: StackTextBuffer<128>,
    state: Option<RenderState>,
}

impl App {
    fn new() -> Self {
        let assets = AssetLocator::discover_or_default();
        let mut assets_status = StackTextBuffer::<128>::new();
        let _ = write!(assets_status, "Assets: {}", assets.root().display());

        let mut audio_status = StackTextBuffer::<96>::new();
        let audio = match AudioEngine::try_new() {
            Ok(engine) => {
                let _ = write!(audio_status, "Audio: cpal active @ {} Hz", engine.sample_rate());
                Some(engine)
            }
            Err(err) => {
                let _ = write!(audio_status, "Audio: unavailable ({err})");
                None
            }
        };

        println!(
            "fnfrs-app initialized ({}x{}, {}, {})",
            VIRTUAL_WIDTH,
            VIRTUAL_HEIGHT,
            assets_status.as_str(),
            audio_status.as_str(),
        );

        Self {
            _audio: audio,
            audio_status,
            assets_status,
            state: None,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let attrs = Window::default_attributes()
            .with_title("fnfrs — Friday Night Funkin' (Rust V-Slice)")
            .with_inner_size(LogicalSize::new(VIRTUAL_WIDTH, VIRTUAL_HEIGHT));

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(err) => {
                eprintln!("failed to create window: {err}");
                event_loop.exit();
                return;
            }
        };

        let mut gpu = match GpuContext::new(Arc::clone(&window)) {
            Ok(ctx) => ctx,
            Err(err) => {
                eprintln!("failed to initialize wgpu context: {err}");
                event_loop.exit();
                return;
            }
        };

        let batcher = SpriteBatcher::new(&gpu, 2048);
        let (hud_font, hud_font_tex) = match BitmapFont::create_embedded_on_gpu(&mut gpu) {
            Ok(pair) => pair,
            Err(err) => {
                eprintln!("failed to upload embedded HUD font: {err}");
                event_loop.exit();
                return;
            }
        };

        window.request_redraw();

        self.state = Some(RenderState {
            window,
            gpu,
            batcher,
            hud_font,
            hud_font_tex,
            start_time: Instant::now(),
            frame_count: 0,
            audio_status: self.audio_status.clone(),
            assets_status: self.assets_status.clone(),
            score_buf: StackTextBuffer::new(),
            fps_buf: StackTextBuffer::new(),
        });
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        if state.window.id() != window_id {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && event.physical_key == PhysicalKey::Code(KeyCode::Escape) =>
            {
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                state.gpu.resize(new_size);
            }
            WindowEvent::RedrawRequested => {
                render_smoke_frame(state, event_loop);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

fn render_smoke_frame(state: &mut RenderState, event_loop: &ActiveEventLoop) {
    state.frame_count += 1;
    let elapsed = state.start_time.elapsed().as_secs_f32();

    state.batcher.begin_frame();
    state.batcher.set_virtual_projection(&state.gpu.queue);

    let white = state.gpu.white_texture();

    // 1. Dark V-Slice virtual canvas backdrop (1280x720).
    state.batcher.push(
        white,
        BlendMode::Normal,
        FilterMode::Nearest,
        SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
            .with_color([0.06, 0.05, 0.11, 1.0]),
    );

    // Subtle top & bottom HUD framing bars.
    state.batcher.push(
        white,
        BlendMode::Normal,
        FilterMode::Nearest,
        SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, 74.0])
            .with_color([0.11, 0.09, 0.20, 0.92]),
    );
    state.batcher.push(
        white,
        BlendMode::Normal,
        FilterMode::Nearest,
        SpriteInstance::new([0.0, VIRTUAL_HEIGHT - 56.0], [VIRTUAL_WIDTH, 56.0])
            .with_color([0.11, 0.09, 0.20, 0.92]),
    );

    // 2. Animated V-Slice test quads exercising rotation, origin pivot, and additive glow.
    let quad_size = 136.0 + 10.0 * (elapsed * 3.0).sin();
    let cx = VIRTUAL_WIDTH * 0.5;
    let cy = VIRTUAL_HEIGHT * 0.5;

    state.batcher.push(
        white,
        BlendMode::Normal,
        FilterMode::Nearest,
        SpriteInstance::new([cx - quad_size * 0.5, cy - quad_size * 0.5], [quad_size, quad_size])
            .with_color([0.18, 0.85, 0.96, 1.0])
            .with_rotation(elapsed * 0.85, [quad_size * 0.5, quad_size * 0.5]),
    );

    let inner_size = 76.0;
    state.batcher.push(
        white,
        BlendMode::Add,
        FilterMode::Nearest,
        SpriteInstance::new(
            [cx - inner_size * 0.5, cy - inner_size * 0.5],
            [inner_size, inner_size],
        )
        .with_color([0.98, 0.25, 0.65, 0.75])
        .with_rotation(-elapsed * 1.4, [inner_size * 0.5, inner_size * 0.5]),
    );

    // 3. Embedded Score / HUD bitmap text (zero per-frame heap allocations).
    let title_style = TextStyle {
        scale: 3.0,
        color: [1.0, 1.0, 1.0, 1.0],
        ..Default::default()
    };
    state.hud_font.draw_str(
        &mut state.batcher,
        state.hud_font_tex,
        "FNFRS MILESTONE 3 // V-SLICE 1280x720",
        24.0,
        14.0,
        &title_style,
    );

    let sub_style = TextStyle {
        scale: 2.0,
        color: [0.70, 0.84, 0.96, 1.0],
        ..Default::default()
    };
    state.hud_font.draw_bytes(
        &mut state.batcher,
        state.hud_font_tex,
        state.audio_status.as_bytes(),
        24.0,
        46.0,
        &sub_style,
    );

    state.fps_buf.clear();
    let _ = write!(
        state.fps_buf,
        "Frame: {} | Time: {:.1}s",
        state.frame_count, elapsed
    );
    let right_style = TextStyle {
        scale: 2.0,
        color: [0.85, 0.85, 0.95, 1.0],
        align: TextAlign::Right,
        ..Default::default()
    };
    state.hud_font.draw_bytes(
        &mut state.batcher,
        state.hud_font_tex,
        state.fps_buf.as_bytes(),
        VIRTUAL_WIDTH - 24.0,
        28.0,
        &right_style,
    );

    state.hud_font.draw_bytes(
        &mut state.batcher,
        state.hud_font_tex,
        state.assets_status.as_bytes(),
        24.0,
        88.0,
        &sub_style,
    );

    let demo_score = (state.frame_count as i64) * 10;
    format_hud_score(&mut state.score_buf, demo_score, 0, 100.0);
    let score_style = TextStyle {
        scale: 2.5,
        color: [1.0, 0.95, 0.45, 1.0],
        align: TextAlign::Center,
        ..Default::default()
    };
    state.hud_font.draw_bytes(
        &mut state.batcher,
        state.hud_font_tex,
        state.score_buf.as_bytes(),
        VIRTUAL_WIDTH * 0.5,
        VIRTUAL_HEIGHT - 38.0,
        &score_style,
    );

    state.batcher.prepare(&state.gpu.device, &state.gpu.queue);

    let frame = match state.gpu.acquire_frame() {
        Ok(Some(frame)) => frame,
        Ok(None) => return,
        Err(GpuError::SurfaceLost) => {
            state.gpu.resize(state.gpu.size);
            return;
        }
        Err(err) => {
            eprintln!("swapchain acquire failed: {err}");
            event_loop.exit();
            return;
        }
    };

    let view = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = state
        .gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fnfrs-frame-encoder"),
        });

    {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("fnfrs-main-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        let vp = state.gpu.letterbox_viewport();
        if vp.width > 0.0 && vp.height > 0.0 {
            render_pass.set_viewport(vp.x, vp.y, vp.width, vp.height, 0.0, 1.0);
            state.batcher.render(&mut render_pass, &state.gpu);
        }
    }

    state.gpu.queue.submit(std::iter::once(encoder.finish()));
    state.gpu.queue.present(frame);
}

fn main() -> Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new();
    event_loop.run_app(&mut app)?;
    Ok(())
}
