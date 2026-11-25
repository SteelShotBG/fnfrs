use std::fmt::Write as _;
use std::fs;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use fnfrs_audio::{decode_file, AudioEngine, SongTracks, TrackStem};
use fnfrs_core::constants::{
    DEFAULT_DIFFICULTY, STRUMLINE_SIZE, STRUMLINE_Y_OFFSET, VIRTUAL_HEIGHT, VIRTUAL_WIDTH,
};
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::song::{SongChartData, SongMetadata, SongNoteData};
use fnfrs_game::{
    GameplaySfxSlots, InputQueue, PlayConfig, PlaySession, StrumlineVisualStyle,
};
use fnfrs_render::{
    BitmapFont, BlendMode, FilterMode, GpuContext, GpuError, MeshBatcher, SpriteBatcher,
    SpriteInstance, StackTextBuffer, TextAlign, TextStyle, TextureHandle,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

struct LoadedChart {
    metadata: Option<SongMetadata>,
    chart: Option<SongChartData>,
    label: StackTextBuffer<64>,
}

struct RuntimeState {
    window: Arc<Window>,
    gpu: GpuContext,
    sprite_batcher: SpriteBatcher,
    mesh_batcher: MeshBatcher,
    hud_font: BitmapFont,
    hud_font_tex: TextureHandle,
    visual_style: StrumlineVisualStyle,

    session: PlaySession,
    inputs: InputQueue,
    loaded_chart: LoadedChart,

    has_song_audio: bool,
    start_time: Instant,
    last_frame_time: Instant,
    frame_count: u64,

    header_buf: StackTextBuffer<128>,
    fps_buf: StackTextBuffer<64>,
}

struct App {
    assets: AssetLocator,
    audio: Option<AudioEngine>,
    loaded_chart: LoadedChart,
    sfx_slots: GameplaySfxSlots,
    has_song_audio: bool,
    header_buf: StackTextBuffer<128>,
    state: Option<RuntimeState>,
}

impl App {
    fn new() -> Self {
        let assets = AssetLocator::discover_or_default();
        let mut header_buf = StackTextBuffer::<128>::new();

        let mut audio = match AudioEngine::try_new() {
            Ok(engine) => Some(engine),
            Err(err) => {
                eprintln!("audio init failed: {err}");
                None
            }
        };

        let loaded_chart = load_bopeebo_chart(&assets);
        let mut sfx_slots = GameplaySfxSlots::default();
        let mut has_song_audio = false;

        if let Some(engine) = audio.as_mut() {
            let sample_rate = engine.sample_rate();
            sfx_slots = load_miss_sfx(&assets, engine, sample_rate);

            if loaded_chart.chart.is_some() {
                has_song_audio = try_start_bopeebo_audio(
                    &assets,
                    engine,
                    sample_rate,
                    loaded_chart.metadata.as_ref(),
                );
            }

            let _ = write!(
                header_buf,
                "{} | cpal {} Hz | B: Botplay | Tab: Scroll | R: Restart",
                loaded_chart.label.as_str(),
                sample_rate,
            );
        } else {
            let _ = write!(
                header_buf,
                "{} | No audio | B: Botplay | Tab: Scroll | R: Restart",
                loaded_chart.label.as_str(),
            );
        }

        println!(
            "fnfrs-app initialized ({}x{}, assets: {}, {})",
            VIRTUAL_WIDTH,
            VIRTUAL_HEIGHT,
            assets.root().display(),
            header_buf.as_str(),
        );

        Self {
            assets,
            audio,
            loaded_chart,
            sfx_slots,
            has_song_audio,
            header_buf,
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

        let sprite_batcher = SpriteBatcher::new(&gpu, 2048);
        let mesh_batcher = MeshBatcher::new(&gpu, 2048, 4096);
        let (hud_font, hud_font_tex) = match BitmapFont::create_embedded_on_gpu(&mut gpu) {
            Ok(pair) => pair,
            Err(err) => {
                eprintln!("failed to upload embedded HUD font: {err}");
                event_loop.exit();
                return;
            }
        };

        let mut visual_style = StrumlineVisualStyle::fallback(gpu.white_texture());
        let hold_path = self.assets.image("NOTE_hold_assets");
        if let Ok(bytes) = fs::read(&hold_path) {
            if let Ok(hold_tex) = gpu.from_png_bytes(Some("NOTE_hold_assets"), &bytes) {
                if let Some((w, h)) = gpu.texture_size(hold_tex) {
                    visual_style.hold_texture = Some(hold_tex);
                    visual_style.hold_tex_size = [w as f32, h as f32];
                }
            }
        }

        let mut session = create_session(&self.loaded_chart, PlayConfig::default());
        session.sfx_slots = self.sfx_slots;

        let now = Instant::now();
        window.request_redraw();

        self.state = Some(RuntimeState {
            window,
            gpu,
            sprite_batcher,
            mesh_batcher,
            hud_font,
            hud_font_tex,
            visual_style,
            session,
            inputs: InputQueue::default(),
            loaded_chart: LoadedChart {
                metadata: self.loaded_chart.metadata.clone(),
                chart: self.loaded_chart.chart.clone(),
                label: self.loaded_chart.label.clone(),
            },
            has_song_audio: self.has_song_audio,
            start_time: now,
            last_frame_time: now,
            frame_count: 0,
            header_buf: self.header_buf.clone(),
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
            WindowEvent::KeyboardInput { event, .. } => {
                let now = Instant::now();
                let pressed = event.state == ElementState::Pressed;
                if let PhysicalKey::Code(code) = event.physical_key {
                    if pressed && !event.repeat {
                        match code {
                            KeyCode::Escape => {
                                event_loop.exit();
                                return;
                            }
                            KeyCode::KeyB => {
                                state.session.config.is_botplay = !state.session.config.is_botplay;
                                return;
                            }
                            KeyCode::Tab => {
                                toggle_downscroll(&mut state.session);
                                return;
                            }
                            KeyCode::KeyR => {
                                let cfg = state.session.config.clone();
                                let sfx = state.session.sfx_slots;
                                state.session = create_session(&state.loaded_chart, cfg);
                                state.session.sfx_slots = sfx;
                                state.inputs.clear();
                                state.start_time = now;
                                if let Some(engine) = self.audio.as_mut() {
                                    let _ = engine.seek_ms(0.0);
                                    let _ = engine.play();
                                }
                                return;
                            }
                            _ => {}
                        }
                    }

                    state.inputs.record_key(code, pressed, event.repeat, now);
                }
            }
            WindowEvent::Resized(new_size) => {
                state.gpu.resize(new_size);
            }
            WindowEvent::RedrawRequested => {
                render_game_frame(state, self.audio.as_mut(), event_loop);
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

fn render_game_frame(
    state: &mut RuntimeState,
    mut audio: Option<&mut AudioEngine>,
    event_loop: &ActiveEventLoop,
) {
    let now = Instant::now();
    let elapsed_s = now
        .saturating_duration_since(state.last_frame_time)
        .as_secs_f32()
        .clamp(0.0, 0.1);
    state.last_frame_time = now;
    state.frame_count += 1;

    let song_pos_ms = if state.has_song_audio {
        if let Some(engine) = audio.as_deref_mut() {
            engine.clock_mut().position_ms_at(now)
        } else {
            now.saturating_duration_since(state.start_time).as_secs_f64() * 1000.0
        }
    } else {
        let t = now.saturating_duration_since(state.start_time).as_secs_f64() * 1000.0;
        if state.loaded_chart.chart.is_none() && t >= 10_000.0 {
            let cfg = state.session.config.clone();
            let sfx = state.session.sfx_slots;
            state.session = create_session(&state.loaded_chart, cfg);
            state.session.sfx_slots = sfx;
            state.start_time = now;
            0.0
        } else {
            t
        }
    };

    let _events = state.session.update(
        song_pos_ms,
        elapsed_s,
        now,
        &mut state.inputs,
        audio,
        &state.visual_style,
    );

    state.sprite_batcher.begin_frame();
    state.sprite_batcher.set_virtual_projection(&state.gpu.queue);
    state.mesh_batcher.begin_frame();
    state.mesh_batcher.set_virtual_projection(&state.gpu.queue);

    let white = state.gpu.white_texture();
    state.sprite_batcher.push(
        white,
        BlendMode::Normal,
        FilterMode::Nearest,
        SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
            .with_color([0.06, 0.05, 0.11, 1.0]),
    );

    state.session.draw(
        &mut state.sprite_batcher,
        &mut state.mesh_batcher,
        &state.visual_style,
        &state.hud_font,
        state.hud_font_tex,
    );

    let info_style = TextStyle {
        scale: 1.5,
        color: [0.75, 0.85, 0.95, 0.95],
        ..Default::default()
    };
    state.hud_font.draw_bytes(
        &mut state.sprite_batcher,
        state.hud_font_tex,
        state.header_buf.as_bytes(),
        18.0,
        6.0,
        &info_style,
    );

    state.fps_buf.clear();
    let _ = write!(
        state.fps_buf,
        "{:.1}s | Bot:{}",
        song_pos_ms / 1000.0,
        if state.session.config.is_botplay {
            "ON"
        } else {
            "OFF"
        }
    );
    state.hud_font.draw_bytes(
        &mut state.sprite_batcher,
        state.hud_font_tex,
        state.fps_buf.as_bytes(),
        VIRTUAL_WIDTH - 18.0,
        6.0,
        &TextStyle {
            scale: 1.5,
            color: [0.85, 0.85, 0.95, 0.95],
            align: TextAlign::Right,
            ..Default::default()
        },
    );

    state
        .sprite_batcher
        .prepare(&state.gpu.device, &state.gpu.queue);
    state
        .mesh_batcher
        .prepare(&state.gpu.device, &state.gpu.queue);

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
            state.mesh_batcher.render(&mut render_pass, &state.gpu);
            state.sprite_batcher.render(&mut render_pass, &state.gpu);
        }
    }

    state.gpu.queue.submit(std::iter::once(encoder.finish()));
    state.gpu.queue.present(frame);
}

fn toggle_downscroll(session: &mut PlaySession) {
    session.config.is_downscroll = !session.config.is_downscroll;
    let new_y = if session.config.is_downscroll {
        VIRTUAL_HEIGHT - STRUMLINE_SIZE - STRUMLINE_Y_OFFSET
    } else {
        STRUMLINE_Y_OFFSET
    };

    session.player_strumline.is_downscroll = session.config.is_downscroll;
    session.player_strumline.y = new_y;
    session.player_strumline.layout_receptors();

    session.opponent_strumline.is_downscroll = session.config.is_downscroll;
    session.opponent_strumline.y = new_y;
    session.opponent_strumline.layout_receptors();
}

fn load_bopeebo_chart(assets: &AssetLocator) -> LoadedChart {
    let mut label = StackTextBuffer::<64>::new();
    let meta_path = assets.song_metadata("bopeebo", None);
    let chart_path = assets.song_chart("bopeebo", None);

    if let (Ok(meta_raw), Ok(chart_raw)) = (fs::read_to_string(&meta_path), fs::read_to_string(&chart_path)) {
        if let (Ok(meta), Ok(chart)) = (
            serde_json::from_str::<SongMetadata>(&meta_raw),
            serde_json::from_str::<SongChartData>(&chart_raw),
        ) {
            let _ = write!(label, "{} [{}]", meta.song_name, DEFAULT_DIFFICULTY);
            return LoadedChart {
                metadata: Some(meta),
                chart: Some(chart),
                label,
            };
        }
    }

    let _ = write!(label, "Demo Pattern (120 BPM)");
    LoadedChart {
        metadata: None,
        chart: None,
        label,
    }
}

fn load_miss_sfx(assets: &AssetLocator, engine: &mut AudioEngine, sample_rate: u32) -> GameplaySfxSlots {
    let mut bank = Vec::with_capacity(3);
    let mut slots = GameplaySfxSlots::default();
    let names = ["missnote1", "missnote2", "missnote3"];
    for (idx, name) in names.iter().enumerate() {
        let path = assets.sound(name);
        if let Ok(decoded) = decode_file(&path, sample_rate) {
            slots.miss_notes[idx] = Some(bank.len());
            bank.push(decoded);
        }
    }
    if !bank.is_empty() {
        let _ = engine.register_sfx_bank(bank);
    }
    slots
}

fn try_start_bopeebo_audio(
    assets: &AssetLocator,
    engine: &mut AudioEngine,
    sample_rate: u32,
    metadata: Option<&SongMetadata>,
) -> bool {
    let inst_path = assets.song_inst("bopeebo", "");
    let Ok(inst_audio) = decode_file(&inst_path, sample_rate) else {
        return false;
    };

    let inst_offset = metadata.map_or(0.0, |m| m.offsets.get_instrumental_offset(None));
    let pv_offset = metadata.map_or(0.0, |m| m.offsets.get_vocal_offset("bf", None));
    let ov_offset = metadata.map_or(0.0, |m| m.offsets.get_vocal_offset("dad", None));

    let player_vocals = decode_file(&assets.song_voices("bopeebo", "-bf"), sample_rate)
        .ok()
        .map(|a| TrackStem::new(a, pv_offset));
    let opponent_vocals = decode_file(&assets.song_voices("bopeebo", "-dad"), sample_rate)
        .ok()
        .map(|a| TrackStem::new(a, ov_offset));

    let tracks = SongTracks::new(
        Some(TrackStem::new(inst_audio, inst_offset)),
        player_vocals,
        opponent_vocals,
    );

    engine.load_song(tracks, 0.0).is_ok() && engine.play().is_ok()
}

fn create_session(loaded: &LoadedChart, config: PlayConfig) -> PlaySession {
    if let (Some(meta), Some(chart)) = (&loaded.metadata, &loaded.chart) {
        return PlaySession::new(meta, chart, DEFAULT_DIFFICULTY, config);
    }

    let mut player_notes = Vec::new();
    let mut opponent_notes = Vec::new();
    for step in 0..16 {
        let time = 1200.0 + (step as f64) * 500.0;
        let dir = (step % 4) as i32;
        opponent_notes.push(SongNoteData {
            time,
            data: dir + 4,
            length: if step % 4 == 3 { 400.0 } else { 0.0 },
            kind: None,
            params: Vec::new(),
        });
        player_notes.push(SongNoteData {
            time: time + 500.0,
            data: dir,
            length: if step % 4 == 3 { 400.0 } else { 0.0 },
            kind: None,
            params: Vec::new(),
        });
    }

    PlaySession::from_raw_notes(120.0, &player_notes, &opponent_notes, config)
}

fn main() -> Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new();
    event_loop.run_app(&mut app)?;
    Ok(())
}
