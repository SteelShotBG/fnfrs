mod audio_loader;
mod gameplay_host;
mod menu_router;

use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use fnfrs_audio::{decode_file, AudioCommand, AudioEngine};
use fnfrs_core::constants::{
    DEFAULT_DIFFICULTY, DEFAULT_VARIATION, VIRTUAL_HEIGHT, VIRTUAL_WIDTH,
};
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::save::SaveData;
use fnfrs_game::debug_display::FunkinDebugDisplay;
use fnfrs_game::menus::{
    CharSelectScreenState, CreditsScreenState, FreeplayScreenState, MainMenuScreenState,
    MenuInputState, OptionsScreenState, StoryMenuScreenState, TitleScreenState,
};
use fnfrs_game::sound_tray::SoundTray;
use fnfrs_game::{GameOverPhase, KeyBinds, PauseMenuInput};
use fnfrs_render::{
    BitmapFont, GpuContext, GpuError, MeshBatcher, RainPostPass, SpriteBatcher, TextureHandle,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use audio_loader::{load_song_chart, play_music_by_key, register_menu_sfx, MenuSfxSlots};
use gameplay_host::{
    build_active_gameplay, restart_runtime_song, update_and_render_gameplay, ActiveGameplay,
    GameplayLaunchOpts,
};
use menu_router::update_and_draw_menus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActiveScreen {
    Title,
    MainMenu,
    StoryMenu,
    Freeplay,
    Options,
    Credits,
    CharSelect,
    Gameplay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReturnTarget {
    Exit,
    MainMenu,
    StoryMenu,
    Freeplay,
}

pub(crate) struct RuntimeState {
    pub window: Arc<Window>,
    pub gpu: GpuContext,
    pub stage_sprite_batcher: SpriteBatcher,
    pub bg_sprite_batcher: SpriteBatcher,
    pub fg_sprite_batcher: SpriteBatcher,
    pub mesh_batcher: MeshBatcher,
    pub overlay_sprite_batcher: SpriteBatcher,
    pub rain_pass: RainPostPass,
    pub hud_font: BitmapFont,
    pub hud_font_tex: TextureHandle,
    pub assets: AssetLocator,

    pub screen: ActiveScreen,
    pub return_target: ReturnTarget,
    pub menu_input: MenuInputState,
    pub menu_sfx: MenuSfxSlots,
    pub menu_music_playing: bool,

    pub title_screen: Option<TitleScreenState>,
    pub main_menu_screen: Option<MainMenuScreenState>,
    pub story_menu_screen: Option<StoryMenuScreenState>,
    pub freeplay_screen: Option<FreeplayScreenState>,
    pub options_screen: Option<OptionsScreenState>,
    pub credits_screen: Option<CreditsScreenState>,
    pub char_select_screen: Option<CharSelectScreenState>,
    pub main_menu_selected: usize,
    pub save: SaveData,
    pub sound_tray: SoundTray,
    pub debug_display: FunkinDebugDisplay,
    pub downscroll_pref: bool,
    pub botplay_pref: bool,

    pub story_playlist: Vec<String>,
    pub story_difficulty: String,
    pub story_level_id: String,
    pub story_playlist_idx: usize,
    pub story_total_score: i64,

    pub gameplay: Option<ActiveGameplay>,

    pub minimized: bool,
    pub occluded: bool,
    pub last_frame_time: Instant,
    pub next_frame_deadline: Instant,
    pub frame_count: u64,
}

struct App {
    assets: AssetLocator,
    audio: Option<AudioEngine>,
    cli_launch: bool,
    initial_song_id: String,
    initial_difficulty: String,
    initial_variation: Option<String>,
    state: Option<RuntimeState>,
}

impl App {
    fn new() -> Self {
        let mut args = std::env::args().skip(1);
        let first_arg = args.next().filter(|s| !s.is_empty());
        let cli_launch = first_arg.is_some();
        let initial_song_id = first_arg
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_else(|| "bopeebo".to_owned());
        let initial_difficulty = args
            .next()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_else(|| DEFAULT_DIFFICULTY.to_owned());
        let initial_variation = args
            .next()
            .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case(DEFAULT_VARIATION))
            .map(|s| s.to_ascii_lowercase());

        let assets = AssetLocator::discover_or_default();
        let audio = match AudioEngine::try_new() {
            Ok(engine) => Some(engine),
            Err(err) => {
                eprintln!("audio init failed: {err}");
                None
            }
        };

        println!(
            "fnfrs-app initialized ({}x{}, assets: {}, mode: {})",
            VIRTUAL_WIDTH,
            VIRTUAL_HEIGHT,
            assets.root().display(),
            if cli_launch { "cli-direct" } else { "menus" },
        );

        Self {
            assets,
            audio,
            cli_launch,
            initial_song_id,
            initial_difficulty,
            initial_variation,
            state: None,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let title = if self.cli_launch {
            format!("fnfrs — {} [{}]", self.initial_song_id, self.initial_difficulty)
        } else {
            "fnfrs — Friday Night Funkin'".to_owned()
        };
        let attrs = Window::default_attributes()
            .with_title(title)
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

        let stage_sprite_batcher = SpriteBatcher::new(&gpu, 1024);
        let bg_sprite_batcher = SpriteBatcher::new(&gpu, 512);
        let fg_sprite_batcher = SpriteBatcher::new(&gpu, 2048);
        let mesh_batcher = MeshBatcher::new(&gpu, 2048, 4096);
        let overlay_sprite_batcher = SpriteBatcher::new(&gpu, 1024);
        let rain_pass = RainPostPass::new(&gpu);
        let (hud_font, hud_font_tex) = load_hud_font(&self.assets, &mut gpu, event_loop);
        gpu.pin_texture(hud_font_tex);

        let save = SaveData::load_or_default();
        let downscroll_pref = save.options.downscroll;
        let botplay_pref = save.options.botplay;
        let sound_tray = SoundTray::load(
            &self.assets,
            &mut gpu,
            save.options.master_volume,
            save.options.muted,
        );
        let debug_display = FunkinDebugDisplay::new(save.options.debug_display);
        if let Some(engine) = self.audio.as_mut() {
            let _ = engine.send(AudioCommand::SetMasterGain(sound_tray.effective_volume()));
        }

        let mut title_screen = None;
        let main_menu_screen = None;
        let story_menu_screen = None;
        let freeplay_screen = None;
        let options_screen = None;
        let credits_screen = None;
        let char_select_screen = None;

        let mut menu_sfx = MenuSfxSlots::default();
        let mut menu_music_playing = false;
        let mut screen = ActiveScreen::Title;
        let mut return_target = ReturnTarget::MainMenu;
        let mut gameplay = None;

        if self.cli_launch {
            screen = ActiveScreen::Gameplay;
            return_target = ReturnTarget::Exit;
            let loaded = load_song_chart(
                &self.assets,
                &self.initial_song_id,
                &self.initial_difficulty,
                self.initial_variation.as_deref(),
            );
            gameplay = Some(build_active_gameplay(
                &self.assets,
                &mut gpu,
                self.audio.as_mut(),
                loaded,
                GameplayLaunchOpts {
                    downscroll: downscroll_pref,
                    botplay: botplay_pref,
                    is_story_mode: false,
                    naughtyness: save.options.naughtyness,
                    master_volume: sound_tray.effective_volume(),
                    keybinds: KeyBinds::from_names(&save.options.keybinds),
                },
                &mut menu_sfx,
                Instant::now(),
            ));
        } else {
            title_screen = Some(TitleScreenState::load(&self.assets, &mut gpu, false));
            if let Some(engine) = self.audio.as_mut() {
                menu_sfx = register_menu_sfx(&self.assets, engine);
                menu_music_playing = play_music_by_key(&self.assets, engine, "freakyMenu");
            }
        }

        let now = Instant::now();
        window.request_redraw();

        self.state = Some(RuntimeState {
            window,
            gpu,
            stage_sprite_batcher,
            bg_sprite_batcher,
            fg_sprite_batcher,
            mesh_batcher,
            overlay_sprite_batcher,
            rain_pass,
            hud_font,
            hud_font_tex,
            assets: self.assets.clone(),
            screen,
            return_target,
            menu_input: MenuInputState::default(),
            menu_sfx,
            menu_music_playing,
            title_screen,
            main_menu_screen,
            story_menu_screen,
            freeplay_screen,
            options_screen,
            credits_screen,
            char_select_screen,
            main_menu_selected: 0,
            save,
            sound_tray,
            debug_display,
            downscroll_pref,
            botplay_pref,
            story_playlist: Vec::new(),
            story_difficulty: DEFAULT_DIFFICULTY.to_owned(),
            story_level_id: String::new(),
            story_playlist_idx: 0,
            story_total_score: 0,
            gameplay,
            minimized: false,
            occluded: false,
            last_frame_time: now,
            next_frame_deadline: now,
            frame_count: 0,
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
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };

                if pressed
                    && !event.repeat
                    && state.screen == ActiveScreen::Options
                    && state
                        .options_screen
                        .as_ref()
                        .is_some_and(OptionsScreenState::is_awaiting_rebind)
                {
                    if let Some(key_name) = KeyBinds::key_to_name(code) {
                        if let Some(opt) = state.options_screen.as_mut() {
                            if opt.feed_captured_key(key_name) {
                                state.save.options = opt.options.clone();
                                let _ = state.save.save_to_disk();
                            }
                        }
                    }
                    return;
                }

                if pressed && !event.repeat {
                    match code {
                        KeyCode::F3 => {
                            state.save.options.debug_display = state.debug_display.toggle();
                            let _ = state.save.save_to_disk();
                            return;
                        }
                        KeyCode::Minus | KeyCode::Equal | KeyCode::Digit0 => {
                            let sfx_key = match code {
                                KeyCode::Minus => state.sound_tray.adjust(-1, false),
                                KeyCode::Equal => state.sound_tray.adjust(1, false),
                                _ => state.sound_tray.adjust(0, true),
                            };
                            state.save.options.master_volume = state.sound_tray.raw_volume();
                            state.save.options.muted = state.sound_tray.is_muted();
                            let _ = state.save.save_to_disk();
                            if let Some(engine) = self.audio.as_mut() {
                                let _ = engine.send(AudioCommand::SetMasterGain(
                                    state.sound_tray.effective_volume(),
                                ));
                                let slot = match sfx_key {
                                    "soundtray/VolMAX" => state.menu_sfx.vol_max,
                                    "soundtray/Voldown" => state.menu_sfx.vol_down,
                                    _ => state.menu_sfx.vol_up,
                                };
                                if let Some(s) = slot {
                                    let _ = engine.play_sfx(s, 0.9);
                                }
                            }
                            return;
                        }
                        _ => {}
                    }
                }

                if state.screen != ActiveScreen::Gameplay {
                    match code {
                        KeyCode::ArrowLeft | KeyCode::KeyA => state.menu_input.left_held = pressed,
                        KeyCode::ArrowRight | KeyCode::KeyD => {
                            state.menu_input.right_held = pressed;
                        }
                        KeyCode::Enter | KeyCode::Space => {
                            state.menu_input.accept_held = pressed;
                        }
                        KeyCode::ShiftLeft | KeyCode::ShiftRight => {
                            state.menu_input.shift_held = pressed;
                        }
                        _ => {}
                    }
                    if pressed && !event.repeat {
                        match code {
                            KeyCode::ArrowUp | KeyCode::KeyW => state.menu_input.up_pressed = true,
                            KeyCode::ArrowDown | KeyCode::KeyS => {
                                state.menu_input.down_pressed = true;
                            }
                            KeyCode::ArrowLeft | KeyCode::KeyA => {
                                state.menu_input.left_pressed = true;
                            }
                            KeyCode::ArrowRight | KeyCode::KeyD => {
                                state.menu_input.right_pressed = true;
                            }
                            KeyCode::Enter | KeyCode::Space => {
                                state.menu_input.accept_pressed = true;
                            }
                            KeyCode::Escape | KeyCode::Backspace => {
                                state.menu_input.back_pressed = true;
                            }
                            KeyCode::Tab => state.menu_input.tab_pressed = true,
                            KeyCode::KeyF => state.menu_input.fav_pressed = true,
                            KeyCode::KeyC => state.menu_input.char_select_pressed = true,
                            _ => {}
                        }
                    }
                    return;
                }

                let Some(gp) = state.gameplay.as_mut() else {
                    return;
                };

                if gp.cutscene.is_active() {
                    if pressed && !event.repeat {
                        match code {
                            KeyCode::Enter | KeyCode::Space => {
                                gp.pending_overlay_accept = true;
                            }
                            KeyCode::Escape | KeyCode::Backspace => {
                                gp.pending_overlay_back = true;
                            }
                            _ => {}
                        }
                    }
                    return;
                }

                if gp.scene.results.is_active {
                    if pressed && !event.repeat {
                        match code {
                            KeyCode::Enter | KeyCode::Space => {
                                gp.pending_overlay_accept = true;
                            }
                            KeyCode::Escape | KeyCode::Backspace => {
                                gp.pending_overlay_back = true;
                            }
                            _ => {}
                        }
                    }
                    return;
                }

                if gp.scene.game_over.phase != GameOverPhase::Inactive {
                    if pressed && !event.repeat {
                        match code {
                            KeyCode::Enter | KeyCode::Space | KeyCode::KeyR => {
                                gp.pending_overlay_accept = true;
                            }
                            KeyCode::Escape | KeyCode::Backspace => {
                                gp.pending_overlay_back = true;
                            }
                            _ => {}
                        }
                    }
                    return;
                }

                if gp.scene.pause_menu.is_open {
                    if pressed && !event.repeat {
                        let mapped = if code == KeyCode::KeyP {
                            PauseMenuInput {
                                back_pressed: true,
                                ..Default::default()
                            }
                        } else {
                            PauseMenuInput::from_key_code(code)
                        };
                        gp.pending_pause_input.up_pressed |= mapped.up_pressed;
                        gp.pending_pause_input.down_pressed |= mapped.down_pressed;
                        gp.pending_pause_input.accept_pressed |= mapped.accept_pressed;
                        gp.pending_pause_input.back_pressed |= mapped.back_pressed;
                    }
                    return;
                }

                if pressed && !event.repeat {
                    match code {
                        KeyCode::Escape | KeyCode::KeyP => {
                            gp.scene.pause_menu.open(
                                gp.scene.session.config.is_botplay,
                                gp.scene.session.config.is_downscroll,
                                gp.scene.game_over.death_count,
                            );
                            gp.scene.pause_menu.is_practice =
                                gp.scene.session.config.practice_mode;
                            gp.scene.session.is_paused = true;
                            gp.scene.countdown.set_paused(true);
                            gp.pause_started_at = Some(now);
                            gp.inputs.clear();
                            if let Some(engine) = self.audio.as_mut() {
                                let _ = engine.pause();
                                let pause_path = state.assets.music("breakfast");
                                if let Ok(decoded) = decode_file(&pause_path, engine.sample_rate())
                                {
                                    let _ = engine.start_bg_loop(decoded);
                                }
                            }
                            return;
                        }
                        KeyCode::KeyB => {
                            gp.scene.session.config.is_botplay =
                                !gp.scene.session.config.is_botplay;
                            gp.scene.session.refresh_hud();
                            state.downscroll_pref = gp.scene.session.config.is_downscroll;
                            state.botplay_pref = gp.scene.session.config.is_botplay;
                            return;
                        }
                        KeyCode::Tab => {
                            let next = !gp.scene.session.config.is_downscroll;
                            gp.scene.session.set_downscroll(next);
                            state.downscroll_pref = next;
                            state.botplay_pref = gp.scene.session.config.is_botplay;
                            return;
                        }
                        KeyCode::KeyR => {
                            restart_runtime_song(&state.assets, gp, self.audio.as_mut(), now, false);
                            return;
                        }
                        _ => {}
                    }
                }

                gp.inputs.record_key(code, pressed, event.repeat, now);
            }
            WindowEvent::Resized(new_size) => {
                state.minimized = new_size.width == 0 || new_size.height == 0;
                state.gpu.resize(new_size);
                if !state.minimized {
                    state.rain_pass.resize_if_needed(&state.gpu);
                    let now = Instant::now();
                    state.last_frame_time = now;
                    state.next_frame_deadline = now;
                }
                schedule_next_step(state, event_loop);
            }
            WindowEvent::Occluded(occluded) => {
                state.occluded = occluded;
                if !occluded {
                    let now = Instant::now();
                    state.last_frame_time = now;
                    state.next_frame_deadline = now;
                }
                schedule_next_step(state, event_loop);
            }
            WindowEvent::RedrawRequested
                if !state.minimized
                    && !state.occluded
                    && state.gpu.size.width > 0
                    && state.gpu.size.height > 0 =>
            {
                render_app_frame(state, self.audio.as_mut(), event_loop);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            schedule_next_step(state, event_loop);
        }
    }
}

fn schedule_next_step(state: &RuntimeState, event_loop: &ActiveEventLoop) {
    if state.minimized || state.occluded {
        event_loop.set_control_flow(ControlFlow::Wait);
        return;
    }
    if state.save.options.fps_cap > 0 {
        let now = Instant::now();
        if now < state.next_frame_deadline {
            event_loop.set_control_flow(ControlFlow::WaitUntil(state.next_frame_deadline));
            return;
        }
    }
    event_loop.set_control_flow(ControlFlow::Poll);
    state.window.request_redraw();
}

fn render_app_frame(
    state: &mut RuntimeState,
    audio: Option<&mut AudioEngine>,
    event_loop: &ActiveEventLoop,
) {
    if state.minimized
        || state.occluded
        || state.gpu.size.width == 0
        || state.gpu.size.height == 0
    {
        return;
    }

    let now = Instant::now();
    let elapsed_s = now
        .saturating_duration_since(state.last_frame_time)
        .as_secs_f32()
        .clamp(0.0, 0.1);
    state.last_frame_time = now;
    if state.save.options.fps_cap > 0 {
        let period = Duration::from_secs_f64(1.0 / f64::from(state.save.options.fps_cap));
        let next = state.next_frame_deadline + period;
        state.next_frame_deadline = if next > now { next } else { now + period };
    }
    state.frame_count += 1;
    state.debug_display.update(elapsed_s);
    state.sound_tray.update(elapsed_s);
    let audio_rate_hz = audio.as_deref().map_or(0, AudioEngine::sample_rate);

    if state.screen != ActiveScreen::Gameplay {
        update_and_draw_menus(state, audio, event_loop, elapsed_s, now, audio_rate_hz);
    } else {
        update_and_render_gameplay(state, audio, event_loop, elapsed_s, now, audio_rate_hz);
    }
}

pub(crate) fn present_single_batch(
    state: &mut RuntimeState,
    event_loop: &ActiveEventLoop,
    clear_color: wgpu::Color,
    render_gameplay_layers: bool,
) {
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

    let vp = state.gpu.letterbox_viewport();
    let rain_active = if render_gameplay_layers {
        state.gameplay.as_ref().and_then(|gp| {
            gp.scene
                .scripts
                .rain_params()
                .filter(|_| {
                    !gp.scene.pause_menu.is_open
                        && gp.scene.game_over.phase == GameOverPhase::Inactive
                })
                .map(|r| {
                    (
                        r,
                        gp.scene.camera_rig.cam_game.scroll,
                        gp.scene.camera_rig.cam_game.effective_zoom(),
                    )
                })
        })
    } else {
        None
    };

    if let Some((rain, cam_pos, cam_zoom)) = rain_active {
        state.rain_pass.resize_if_needed(&state.gpu);
        state.rain_pass.update_uniforms(
            &state.gpu.queue,
            rain.time,
            rain.scale,
            rain.intensity,
            rain.color,
            [VIRTUAL_WIDTH, VIRTUAL_HEIGHT],
            cam_pos,
            cam_zoom,
        );
        {
            let mut world_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fnfrs-rain-world-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: state.rain_pass.target_view(),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if vp.width > 0.0 && vp.height > 0.0 {
                world_pass.set_viewport(vp.x, vp.y, vp.width, vp.height, 0.0, 1.0);
                state
                    .stage_sprite_batcher
                    .render(&mut world_pass, &state.gpu);
            }
        }
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
            state.rain_pass.draw(&mut render_pass);
            if vp.width > 0.0 && vp.height > 0.0 {
                render_pass.set_viewport(vp.x, vp.y, vp.width, vp.height, 0.0, 1.0);
                state.bg_sprite_batcher.render(&mut render_pass, &state.gpu);
                state.mesh_batcher.render(&mut render_pass, &state.gpu);
                state.fg_sprite_batcher.render(&mut render_pass, &state.gpu);
                state
                    .overlay_sprite_batcher
                    .render(&mut render_pass, &state.gpu);
            }
        }
    } else {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("fnfrs-main-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear_color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        if vp.width > 0.0 && vp.height > 0.0 {
            render_pass.set_viewport(vp.x, vp.y, vp.width, vp.height, 0.0, 1.0);
            if render_gameplay_layers {
                state
                    .stage_sprite_batcher
                    .render(&mut render_pass, &state.gpu);
                state.bg_sprite_batcher.render(&mut render_pass, &state.gpu);
                state.mesh_batcher.render(&mut render_pass, &state.gpu);
                state.fg_sprite_batcher.render(&mut render_pass, &state.gpu);
            }
            state
                .overlay_sprite_batcher
                .render(&mut render_pass, &state.gpu);
        }
    }

    state.gpu.queue.submit(std::iter::once(encoder.finish()));
    state.gpu.queue.present(frame);
}

fn load_hud_font(
    assets: &AssetLocator,
    gpu: &mut GpuContext,
    event_loop: &ActiveEventLoop,
) -> (BitmapFont, TextureHandle) {
    let fnt_path = assets.font("vcr-bmp.fnt");
    let png_path = assets.font("vcr-bmp.png");
    if let (Ok(fnt_str), Ok(png_bytes)) = (fs::read_to_string(&fnt_path), fs::read(&png_path)) {
        if let Ok(pair) =
            BitmapFont::create_bmfont_on_gpu(gpu, Some("vcr-bmp"), &fnt_str, &png_bytes, false)
        {
            return pair;
        }
    }

    match BitmapFont::create_embedded_on_gpu(gpu) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("failed to upload embedded HUD font: {err}");
            event_loop.exit();
            (BitmapFont::embedded_fallback().0, gpu.white_texture())
        }
    }
}

fn main() -> Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new();
    event_loop.run_app(&mut app)?;
    Ok(())
}
