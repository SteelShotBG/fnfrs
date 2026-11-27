use std::fmt::Write as _;
use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use fnfrs_audio::{decode_file, AudioCommand, AudioEngine, DecodedAudio, SongTracks, TrackStem};
use fnfrs_core::constants::{
    DEFAULT_CHARACTER, DEFAULT_DIFFICULTY, DEFAULT_NOTE_STYLE, DEFAULT_STAGE, DEFAULT_VARIATION,
    VIRTUAL_HEIGHT, VIRTUAL_WIDTH,
};
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::save::SaveData;
use fnfrs_core::schema::song::{SongChartData, SongMetadata, SongNoteData};
use fnfrs_game::cutscene::{CutsceneAudioRequest, CutsceneController};
use fnfrs_game::debug_display::{DebugSnapshot, FunkinDebugDisplay};
use fnfrs_game::menus::{
    CharSelectScreenState, CreditsScreenState, FreeplayScreenState, MainMenuScreenState,
    MenuCommand, MenuInputState, OptionsScreenState, StoryMenuScreenState, TitleScreenState,
};
use fnfrs_game::sound_tray::SoundTray;
use fnfrs_game::{
    BaseCharacter, CharacterKind, CountdownAssets, CountdownSfx, GameEventAction,
    GameOverAction, GameOverAudioCue, GameOverPhase, GameplaySfxSlots, HealthIcon, InputQueue,
    KeyBinds, NativeScriptHost, NoteStyleAssets, PauseMenuAction, PauseMenuInput, PlayConfig,
    PlayScene, PlaySceneConfig, PlaySession, ResultsAction, ResultsRank, ScriptSfxSlots,
    SongEventQueue, Stage, StrumlineVisualStyle,
};
use fnfrs_render::{
    BitmapFont, GpuContext, GpuError, MeshBatcher, RainPostPass, SpriteBatcher, StackTextBuffer,
    TextureHandle,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveScreen {
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
enum ReturnTarget {
    Exit,
    MainMenu,
    StoryMenu,
    Freeplay,
}

#[derive(Debug, Clone, Copy, Default)]
struct MenuSfxSlots {
    scroll: Option<usize>,
    confirm: Option<usize>,
    cancel: Option<usize>,
    vol_up: Option<usize>,
    vol_down: Option<usize>,
    vol_max: Option<usize>,
}

struct LoadedChart {
    song_id: String,
    difficulty: String,
    variation: Option<String>,
    metadata: Option<SongMetadata>,
    chart: Option<SongChartData>,
    label: StackTextBuffer<96>,
}

#[derive(Debug, Clone, Copy, Default)]
struct CutsceneSfxSlots {
    pixel_text: Option<usize>,
    click_text: Option<usize>,
    angry_box: Option<usize>,
    senpai_dies: Option<usize>,
    lights_turn_on: Option<usize>,
    santa_emotion: Option<usize>,
    santa_shot: Option<usize>,
}

struct GameplayLaunchOpts {
    downscroll: bool,
    botplay: bool,
    is_story_mode: bool,
    naughtyness: bool,
    master_volume: f32,
    keybinds: KeyBinds,
}

struct ActiveGameplay {
    scene: PlayScene,
    inputs: InputQueue,
    note_assets: NoteStyleAssets,
    visual_style: StrumlineVisualStyle,
    loaded_chart: LoadedChart,
    has_song_audio: bool,
    song_audio_replaced: bool,
    song_duration_ms: Option<f64>,
    game_over_loss_sfx_slot: Option<usize>,
    cutscene: CutsceneController,
    cutscene_sfx: CutsceneSfxSlots,
    waiting_post_cutscene: bool,
    pending_seek_gen: Option<u64>,
    pause_started_at: Option<Instant>,
    paused_duration: Duration,
    pending_pause_input: PauseMenuInput,
    pending_overlay_accept: bool,
    pending_overlay_back: bool,
    start_time: Instant,
}

struct RuntimeState {
    window: Arc<Window>,
    gpu: GpuContext,
    stage_sprite_batcher: SpriteBatcher,
    bg_sprite_batcher: SpriteBatcher,
    fg_sprite_batcher: SpriteBatcher,
    mesh_batcher: MeshBatcher,
    overlay_sprite_batcher: SpriteBatcher,
    rain_pass: RainPostPass,
    hud_font: BitmapFont,
    hud_font_tex: TextureHandle,
    assets: AssetLocator,

    screen: ActiveScreen,
    return_target: ReturnTarget,
    menu_input: MenuInputState,
    menu_sfx: MenuSfxSlots,
    menu_music_playing: bool,

    title_screen: Option<TitleScreenState>,
    main_menu_screen: Option<MainMenuScreenState>,
    story_menu_screen: Option<StoryMenuScreenState>,
    freeplay_screen: Option<FreeplayScreenState>,
    options_screen: Option<OptionsScreenState>,
    credits_screen: Option<CreditsScreenState>,
    char_select_screen: Option<CharSelectScreenState>,
    main_menu_selected: usize,
    save: SaveData,
    sound_tray: SoundTray,
    debug_display: FunkinDebugDisplay,
    downscroll_pref: bool,
    botplay_pref: bool,

    story_playlist: Vec<String>,
    story_difficulty: String,
    story_level_id: String,
    story_playlist_idx: usize,
    story_total_score: i64,

    gameplay: Option<ActiveGameplay>,

    minimized: bool,
    occluded: bool,
    last_frame_time: Instant,
    frame_count: u64,
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

        let save = SaveData::load_or_default();
        let downscroll_pref = save.options.downscroll;
        let botplay_pref = save.options.botplay;
        let mut sound_tray = SoundTray::load(
            &self.assets,
            &mut gpu,
            save.options.master_volume,
            save.options.muted,
        );
        let debug_display = FunkinDebugDisplay::new(save.options.debug_display);
        if let Some(engine) = self.audio.as_mut() {
            let _ = engine.send(AudioCommand::SetMasterGain(sound_tray.effective_volume()));
        }
        let _ = &mut sound_tray;

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
                }
                if state.minimized || state.occluded {
                    event_loop.set_control_flow(ControlFlow::Wait);
                } else {
                    state.last_frame_time = Instant::now();
                    event_loop.set_control_flow(ControlFlow::Poll);
                    state.window.request_redraw();
                }
            }
            WindowEvent::Occluded(occluded) => {
                state.occluded = occluded;
                if occluded || state.minimized {
                    event_loop.set_control_flow(ControlFlow::Wait);
                } else {
                    state.last_frame_time = Instant::now();
                    event_loop.set_control_flow(ControlFlow::Poll);
                    state.window.request_redraw();
                }
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
            if state.minimized || state.occluded {
                event_loop.set_control_flow(ControlFlow::Wait);
            } else {
                event_loop.set_control_flow(ControlFlow::Poll);
                state.window.request_redraw();
            }
        }
    }
}

fn build_active_gameplay(
    assets: &AssetLocator,
    gpu: &mut GpuContext,
    mut audio: Option<&mut AudioEngine>,
    loaded_chart: LoadedChart,
    opts: GameplayLaunchOpts,
    menu_sfx: &mut MenuSfxSlots,
    now: Instant,
) -> ActiveGameplay {
    let metadata = loaded_chart.metadata.as_ref();
    let note_style_id = metadata
        .map(|m| m.play_data.note_style.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_NOTE_STYLE);

    let stage_id = metadata
        .map(|m| m.play_data.stage.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_STAGE);
    let stage = Stage::load_by_id(stage_id, assets, gpu);

    let mut level_assets = assets.clone();
    if let Some(st) = &stage {
        if !st.directory.is_empty() {
            level_assets.set_current_level(Some(&st.directory));
        }
    }

    let note_assets = NoteStyleAssets::load_from_assets(&level_assets, gpu, note_style_id);
    let visual_style = note_assets.to_visual_style(gpu.white_texture());
    let mut countdown_assets = CountdownAssets::load_from_assets(&level_assets, gpu, note_style_id);

    let bf_id = metadata
        .map(|m| m.play_data.characters.player.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_CHARACTER);
    let dad_id = metadata
        .map(|m| m.play_data.characters.opponent.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("dad");
    let gf_id = metadata
        .map(|m| m.play_data.characters.girlfriend.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("gf");

    let bf = BaseCharacter::load_by_id(bf_id, CharacterKind::Bf, &level_assets, gpu);
    let dad = BaseCharacter::load_by_id(dad_id, CharacterKind::Dad, &level_assets, gpu);
    let gf = if gf_id.eq_ignore_ascii_case("none") {
        None
    } else {
        BaseCharacter::load_by_id(gf_id, CharacterKind::Gf, &level_assets, gpu)
    };

    let bf_is_pixel = bf.as_ref().is_some_and(|c| c.is_pixel);
    let dad_is_pixel = dad.as_ref().is_some_and(|c| c.is_pixel);
    let icon_p1 = HealthIcon::load(
        &level_assets,
        gpu,
        bf_id,
        true,
        bf.as_ref().and_then(|c| c.health_icon.as_ref()),
        bf_is_pixel,
    );
    let icon_p2 = HealthIcon::load(
        &level_assets,
        gpu,
        dad_id,
        false,
        dad.as_ref().and_then(|c| c.health_icon.as_ref()),
        dad_is_pixel,
    );

    let mut song_duration_ms = None;
    let mut viz_inst_audio: Option<(DecodedAudio, i64)> = None;
    let mut sfx_slots = GameplaySfxSlots::default();
    let mut script_sfx_slots = ScriptSfxSlots::default();
    let mut game_over_loss_sfx_slot = None;
    let mut cutscene_sfx = CutsceneSfxSlots::default();

    if let Some(engine) = audio.as_deref_mut() {
        let sample_rate = engine.sample_rate();
        if loaded_chart.chart.is_some() {
            if let Some((dur, inst, off_frames)) =
                try_load_song_audio(assets, engine, sample_rate, &loaded_chart)
            {
                song_duration_ms = Some(dur);
                viz_inst_audio = Some((inst, off_frames));
            }
        }
        let (miss_slots, loss_slot, script_slots, tray_slots, cs_slots) = register_all_sfx(
            &level_assets,
            engine,
            sample_rate,
            &mut countdown_assets,
            bf_is_pixel,
            stage_id,
        );
        sfx_slots = miss_slots;
        game_over_loss_sfx_slot = loss_slot;
        script_sfx_slots = script_slots;
        cutscene_sfx = cs_slots;
        menu_sfx.vol_up = tray_slots.0;
        menu_sfx.vol_down = tray_slots.1;
        menu_sfx.vol_max = tray_slots.2;
    }

    let has_song_audio = song_duration_ms.is_some();
    let initial_config = PlayConfig {
        scroll_speed: 0.0,
        is_downscroll: opts.downscroll,
        is_botplay: opts.botplay,
        ..PlayConfig::default()
    };
    let mut session = create_session(&loaded_chart, initial_config);
    session.sfx_slots = sfx_slots;
    session.set_strumline_metrics(note_assets.strum_hitbox[1], note_assets.strum_offsets[1]);

    let event_queue = loaded_chart
        .chart
        .as_ref()
        .map_or_else(SongEventQueue::default, SongEventQueue::from_chart);

    let start_countdown = loaded_chart.chart.is_some();
    let effective_duration_ms = song_duration_ms.or_else(|| {
        loaded_chart
            .chart
            .as_ref()
            .map(|_| compute_chart_end_ms(&session))
    });

    let song_display_name = metadata
        .map(|m| m.song_name.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| loaded_chart.song_id.clone());

    let mut scene = PlayScene::new(
        session,
        stage,
        bf,
        dad,
        gf,
        event_queue,
        icon_p1,
        icon_p2,
        Some(countdown_assets),
        PlaySceneConfig {
            song_name: song_display_name,
            difficulty: loaded_chart.difficulty.clone(),
            start_countdown,
            song_duration_ms: effective_duration_ms,
            death_count: 0,
        },
    );
    if let Some((_, sz)) = note_assets.health_bar_tex {
        scene.set_health_bar_size(sz[0], sz[1]);
    }
    preload_event_icons(&level_assets, gpu, &mut scene, bf_is_pixel, dad_is_pixel);
    let picospeaker_notes = loaded_chart
        .chart
        .as_ref()
        .map(|c| c.get_notes("picospeaker"))
        .unwrap_or(&[]);
    let mut scripts = NativeScriptHost::new(
        loaded_chart.song_id.as_str(),
        stage_id,
        dad_id,
        gf_id,
        scene.gf.as_ref(),
        picospeaker_notes,
        &level_assets,
        gpu,
        script_sfx_slots,
    );
    scripts.set_visualizer_audio(viz_inst_audio);
    scripts.set_visualizer_audible(opts.master_volume > 0.0);
    scene.attach_scripts(scripts);
    if let Some(meta) = metadata {
        if !meta.play_data.difficulties.is_empty() {
            let mut diff_refs: Vec<&str> = Vec::with_capacity(meta.play_data.difficulties.len());
            for d in &meta.play_data.difficulties {
                diff_refs.push(d.as_str());
            }
            scene.pause_menu.set_difficulties(&diff_refs);
        }
    }

    let mut cutscene = CutsceneController::default();
    let pre_req = cutscene.start_pre_song(
        &loaded_chart.song_id,
        loaded_chart.variation.as_deref(),
        opts.is_story_mode,
        opts.naughtyness,
        &level_assets,
        gpu,
        opts.master_volume,
    );
    if cutscene.is_active() {
        scene.session.is_paused = true;
        scene.countdown.set_paused(true);
    }
    dispatch_cutscene_audio(&level_assets, audio.as_deref_mut(), cutscene_sfx, pre_req);

    if !cutscene.is_active() && !start_countdown && has_song_audio {
        if let Some(engine) = audio {
            let _ = engine.play();
        }
    }

    ActiveGameplay {
        scene,
        inputs: InputQueue::new(opts.keybinds),
        note_assets,
        visual_style,
        loaded_chart,
        has_song_audio,
        song_audio_replaced: false,
        song_duration_ms: effective_duration_ms,
        game_over_loss_sfx_slot,
        cutscene,
        cutscene_sfx,
        waiting_post_cutscene: false,
        pending_seek_gen: None,
        pause_started_at: None,
        paused_duration: Duration::ZERO,
        pending_pause_input: PauseMenuInput::default(),
        pending_overlay_accept: false,
        pending_overlay_back: false,
        start_time: now,
    }
}

fn purge_inactive_resources(
    state: &mut RuntimeState,
    audio: Option<&mut AudioEngine>,
    keep_screen: ActiveScreen,
) {
    if keep_screen != ActiveScreen::Title {
        state.title_screen = None;
    }
    if keep_screen != ActiveScreen::MainMenu {
        state.main_menu_screen = None;
    }
    if keep_screen != ActiveScreen::StoryMenu {
        state.story_menu_screen = None;
    }
    if keep_screen != ActiveScreen::Freeplay {
        state.freeplay_screen = None;
    }
    if keep_screen != ActiveScreen::Options {
        state.options_screen = None;
    }
    if keep_screen != ActiveScreen::Credits {
        state.credits_screen = None;
    }
    if keep_screen != ActiveScreen::CharSelect {
        state.char_select_screen = None;
    }
    if keep_screen != ActiveScreen::Gameplay {
        state.gameplay = None;
    }
    state.gpu.clear_user_textures();
    let (hud_font, hud_font_tex) = load_hud_font_optional(&state.assets, &mut state.gpu);
    state.hud_font = hud_font;
    state.hud_font_tex = hud_font_tex;
    state.sound_tray.reload_textures(&state.assets, &mut state.gpu);
    if let Some(engine) = audio {
        engine.reclaim_retired();
    }
}

fn enter_menu_screen(
    state: &mut RuntimeState,
    audio: Option<&mut AudioEngine>,
    target: ActiveScreen,
    skip_title_intro: bool,
) {
    state.screen = target;
    let already_loaded = match target {
        ActiveScreen::Title => state.title_screen.is_some(),
        ActiveScreen::MainMenu => state.main_menu_screen.is_some(),
        ActiveScreen::StoryMenu => state.story_menu_screen.is_some(),
        ActiveScreen::Freeplay => state.freeplay_screen.is_some(),
        ActiveScreen::Options => state.options_screen.is_some(),
        ActiveScreen::Credits => state.credits_screen.is_some(),
        ActiveScreen::CharSelect => state.char_select_screen.is_some(),
        ActiveScreen::Gameplay => state.gameplay.is_some(),
    };
    if !already_loaded {
        state.title_screen = None;
        state.main_menu_screen = None;
        state.story_menu_screen = None;
        state.freeplay_screen = None;
        state.options_screen = None;
        state.credits_screen = None;
        state.char_select_screen = None;
        state.gameplay = None;
        state.gpu.clear_user_textures();
        let (hud_font, hud_font_tex) = load_hud_font_optional(&state.assets, &mut state.gpu);
        state.hud_font = hud_font;
        state.hud_font_tex = hud_font_tex;
        state.sound_tray.reload_textures(&state.assets, &mut state.gpu);
        if let Some(engine) = audio {
            engine.reclaim_retired();
        }
        match target {
            ActiveScreen::Title => {
                state.title_screen =
                    Some(TitleScreenState::load(&state.assets, &mut state.gpu, skip_title_intro));
            }
            ActiveScreen::MainMenu => {
                state.main_menu_screen = Some(MainMenuScreenState::load(
                    &state.assets,
                    &mut state.gpu,
                    state.save.options.flashing_lights,
                    state.main_menu_selected,
                ));
            }
            ActiveScreen::StoryMenu => {
                state.story_menu_screen = Some(StoryMenuScreenState::load(&state.assets, &mut state.gpu));
            }
            ActiveScreen::Freeplay => {
                let mut f = FreeplayScreenState::load(&state.assets, &mut state.gpu);
                f.sync_from_save(&state.assets, &mut state.gpu, &state.save);
                state.freeplay_screen = Some(f);
            }
            ActiveScreen::Options => {
                state.options_screen = Some(OptionsScreenState::load(
                    &state.assets,
                    &mut state.gpu,
                    state.save.options.clone(),
                ));
            }
            ActiveScreen::Credits => {
                state.credits_screen = Some(CreditsScreenState::load(&state.assets, &mut state.gpu));
            }
            ActiveScreen::CharSelect => {
                state.char_select_screen = Some(CharSelectScreenState::load(
                    &state.assets,
                    &mut state.gpu,
                    &state.save.playable_character,
                ));
            }
            ActiveScreen::Gameplay => {}
        }
    } else {
        match target {
            ActiveScreen::Title => {
                if let Some(t) = &mut state.title_screen {
                    t.reset(skip_title_intro);
                }
            }
            ActiveScreen::MainMenu => {
                if let Some(m) = &mut state.main_menu_screen {
                    m.reset_for_enter(state.main_menu_selected, state.save.options.flashing_lights);
                }
            }
            ActiveScreen::StoryMenu => {
                if let Some(s) = &mut state.story_menu_screen {
                    s.reset_for_enter();
                }
            }
            ActiveScreen::Freeplay => {
                if let Some(f) = &mut state.freeplay_screen {
                    f.sync_from_save(&state.assets, &mut state.gpu, &state.save);
                    f.reset_for_enter();
                }
            }
            ActiveScreen::Options | ActiveScreen::Credits | ActiveScreen::CharSelect | ActiveScreen::Gameplay => {}
        }
    }
}

fn return_to_menu(state: &mut RuntimeState, mut audio: Option<&mut AudioEngine>, event_loop: &ActiveEventLoop) {
    if let Some(engine) = audio.as_deref_mut() {
        let _ = engine.stop_bg_loop();
    }
    if state.return_target == ReturnTarget::Exit {
        event_loop.exit();
        return;
    }
    let target = match state.return_target {
        ReturnTarget::Exit | ReturnTarget::MainMenu => ActiveScreen::MainMenu,
        ReturnTarget::StoryMenu => ActiveScreen::StoryMenu,
        ReturnTarget::Freeplay => ActiveScreen::Freeplay,
    };
    enter_menu_screen(state, audio.as_deref_mut(), target, true);
    if let Some(engine) = audio {
        state.menu_sfx = register_menu_sfx(&state.assets, engine);
        state.menu_music_playing = play_music_by_key(&state.assets, engine, "freakyMenu");
    }
}

fn render_app_frame(
    state: &mut RuntimeState,
    mut audio: Option<&mut AudioEngine>,
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
    state.frame_count += 1;
    state.debug_display.update(elapsed_s);
    state.sound_tray.update(elapsed_s);
    let audio_rate_hz = audio.as_deref().map_or(0, AudioEngine::sample_rate);

    if state.screen != ActiveScreen::Gameplay {
        if state.menu_music_playing && state.screen != ActiveScreen::Freeplay {
            if let Some(engine) = audio.as_deref_mut() {
                if !engine.clock().is_playing() {
                    let _ = engine.seek_ms(0.0);
                    let _ = engine.play();
                }
            }
        }

        let input = MenuInputState {
            up_pressed: std::mem::take(&mut state.menu_input.up_pressed),
            down_pressed: std::mem::take(&mut state.menu_input.down_pressed),
            left_pressed: std::mem::take(&mut state.menu_input.left_pressed),
            right_pressed: std::mem::take(&mut state.menu_input.right_pressed),
            accept_pressed: std::mem::take(&mut state.menu_input.accept_pressed),
            accept_held: state.menu_input.accept_held,
            back_pressed: std::mem::take(&mut state.menu_input.back_pressed),
            tab_pressed: std::mem::take(&mut state.menu_input.tab_pressed),
            fav_pressed: std::mem::take(&mut state.menu_input.fav_pressed),
            char_select_pressed: std::mem::take(&mut state.menu_input.char_select_pressed),
            left_held: state.menu_input.left_held,
            right_held: state.menu_input.right_held,
            shift_held: state.menu_input.shift_held,
        };

        let mut enter_char_select = false;
        let cmd = match state.screen {
            ActiveScreen::Title => state
                .title_screen
                .as_mut()
                .map_or(MenuCommand::None, |s| s.update(elapsed_s, input)),
            ActiveScreen::MainMenu => {
                if let Some(s) = &mut state.main_menu_screen {
                    let cmd = s.update(elapsed_s, input);
                    state.main_menu_selected = s.selected();
                    cmd
                } else {
                    MenuCommand::None
                }
            }
            ActiveScreen::StoryMenu => {
                let cmd = state
                    .story_menu_screen
                    .as_mut()
                    .map_or(MenuCommand::None, |s| s.update(elapsed_s, input));
                if let Some(s) = &mut state.story_menu_screen {
                    s.sync_active_week(&state.assets, &mut state.gpu);
                }
                cmd
            }
            ActiveScreen::Freeplay => {
                let cmd = state
                    .freeplay_screen
                    .as_mut()
                    .map_or(MenuCommand::None, |s| s.update(elapsed_s, input));
                if let Some(f) = &mut state.freeplay_screen {
                    let mut sync_needed = false;
                    if let Some((song_id, _var)) = f.favorite_toggle_requested.take() {
                        state.save.toggle_favorite(&song_id);
                        sync_needed = true;
                    }
                    if input.up_pressed || input.down_pressed || input.left_pressed
                        || input.right_pressed || input.tab_pressed
                    {
                        sync_needed = true;
                    }
                    if sync_needed {
                        f.sync_from_save(&state.assets, &mut state.gpu, &state.save);
                    }
                    if f.char_select_requested {
                        f.char_select_requested = false;
                        enter_char_select = true;
                    }
                }
                cmd
            }
            ActiveScreen::Options => state
                .options_screen
                .as_mut()
                .map_or(MenuCommand::None, |s| s.update(elapsed_s, input)),
            ActiveScreen::Credits => state
                .credits_screen
                .as_mut()
                .map_or(MenuCommand::None, |s| s.update(elapsed_s, input)),
            ActiveScreen::CharSelect => state
                .char_select_screen
                .as_mut()
                .map_or(MenuCommand::None, |s| s.update(elapsed_s, input)),
            ActiveScreen::Gameplay => MenuCommand::None,
        };

        handle_menu_command(state, audio.as_deref_mut(), cmd, now);
        if enter_char_select {
            enter_menu_screen(state, audio, ActiveScreen::CharSelect, true);
        }

        state.overlay_sprite_batcher.begin_frame();
        state
            .overlay_sprite_batcher
            .set_virtual_projection(&state.gpu.queue);
        match state.screen {
            ActiveScreen::Title => {
                if let Some(s) = &state.title_screen {
                    s.draw(&mut state.overlay_sprite_batcher, &state.hud_font, state.hud_font_tex);
                }
            }
            ActiveScreen::MainMenu => {
                if let Some(s) = &state.main_menu_screen {
                    s.draw(&mut state.overlay_sprite_batcher, &state.hud_font, state.hud_font_tex);
                }
            }
            ActiveScreen::StoryMenu => {
                if let Some(s) = &state.story_menu_screen {
                    s.draw(
                        &mut state.overlay_sprite_batcher,
                        &state.hud_font,
                        state.hud_font_tex,
                        input,
                    );
                }
            }
            ActiveScreen::Freeplay => {
                if let Some(s) = &state.freeplay_screen {
                    s.draw(&mut state.overlay_sprite_batcher, &state.hud_font, state.hud_font_tex);
                }
            }
            ActiveScreen::Options => {
                if let Some(s) = &state.options_screen {
                    s.draw(&mut state.overlay_sprite_batcher, &state.hud_font, state.hud_font_tex);
                }
            }
            ActiveScreen::Credits => {
                if let Some(s) = &state.credits_screen {
                    s.draw(&mut state.overlay_sprite_batcher, &state.hud_font, state.hud_font_tex);
                }
            }
            ActiveScreen::CharSelect => {
                if let Some(s) = &state.char_select_screen {
                    s.draw(&mut state.overlay_sprite_batcher, &state.hud_font, state.hud_font_tex);
                }
            }
            ActiveScreen::Gameplay => {}
        }
        let tex_count = state.gpu.active_texture_count();
        let spr_count = state.overlay_sprite_batcher.instance_count();
        let white_tex = state.gpu.white_texture();
        let snap = DebugSnapshot {
            screen_label: match state.screen {
                ActiveScreen::Title => "TitleScreen",
                ActiveScreen::MainMenu => "MainMenu",
                ActiveScreen::StoryMenu => "StoryMenu",
                ActiveScreen::Freeplay => "Freeplay",
                ActiveScreen::Options => "Options",
                ActiveScreen::Credits => "Credits",
                ActiveScreen::CharSelect => "CharSelect",
                ActiveScreen::Gameplay => "Gameplay",
            },
            sprite_count: spr_count,
            texture_count: tex_count,
            audio_rate_hz,
            is_botplay: state.botplay_pref,
            is_downscroll: state.downscroll_pref,
            ..Default::default()
        };
        state.debug_display.draw(
            &mut state.overlay_sprite_batcher,
            white_tex,
            &state.hud_font,
            state.hud_font_tex,
            &snap,
        );
        state
            .sound_tray
            .draw(&mut state.overlay_sprite_batcher, &state.hud_font, state.hud_font_tex);
        state
            .overlay_sprite_batcher
            .prepare(&state.gpu.device, &state.gpu.queue);

        present_single_batch(state, event_loop, wgpu::Color::BLACK, false);
        return;
    }

    let Some(gp) = state.gameplay.as_mut() else {
        return;
    };

    let overlay_accept = std::mem::take(&mut gp.pending_overlay_accept);
    let overlay_back = std::mem::take(&mut gp.pending_overlay_back);
    let pause_input = std::mem::take(&mut gp.pending_pause_input);

    if gp.cutscene.is_active() {
        let (done, audio_req) = gp.cutscene.update(
            elapsed_s,
            &state.gpu,
            state.sound_tray.effective_volume(),
            overlay_accept,
            overlay_back,
        );
        dispatch_cutscene_audio(
            &state.assets,
            audio.as_deref_mut(),
            gp.cutscene_sfx,
            audio_req,
        );
        if done {
            if gp.waiting_post_cutscene {
                gp.waiting_post_cutscene = false;
                activate_results_screen(&state.assets, &mut state.gpu, &state.save, gp, audio.as_deref_mut());
            } else {
                gp.scene.session.is_paused = false;
                gp.scene.countdown.set_paused(false);
                gp.start_time = now;
                gp.paused_duration = Duration::ZERO;
                if let Some(engine) = audio.as_deref_mut() {
                    let _ = engine.stop_bg_loop();
                }
            }
        }
    } else if gp.scene.results.is_active {
        if gp
            .scene
            .results
            .update(elapsed_s, overlay_accept, overlay_back)
            == ResultsAction::Continue
        {
            if state.return_target == ReturnTarget::StoryMenu
                && state.story_playlist_idx + 1 < state.story_playlist.len()
            {
                state.story_playlist_idx += 1;
                let next_song = state.story_playlist[state.story_playlist_idx].clone();
                let diff = state.story_difficulty.clone();
                purge_inactive_resources(state, audio.as_deref_mut(), ActiveScreen::Title);
                state.screen = ActiveScreen::Gameplay;
                let loaded = load_song_chart(&state.assets, &next_song, &diff, None);
                state.gameplay = Some(build_active_gameplay(
                    &state.assets,
                    &mut state.gpu,
                    audio,
                    loaded,
                    GameplayLaunchOpts {
                        downscroll: state.downscroll_pref,
                        botplay: state.botplay_pref,
                        is_story_mode: true,
                        naughtyness: state.save.options.naughtyness,
                        master_volume: state.sound_tray.effective_volume(),
                        keybinds: KeyBinds::from_names(&state.save.options.keybinds),
                    },
                    &mut state.menu_sfx,
                    now,
                ));
                return;
            }
            if state.return_target == ReturnTarget::StoryMenu && !state.story_level_id.is_empty() {
                state.save.record_level_score(
                    &state.story_level_id,
                    &state.story_difficulty,
                    state.story_total_score,
                );
                state.story_total_score = 0;
            }
            return_to_menu(state, audio, event_loop);
            return;
        }
    } else if gp.scene.game_over.phase != GameOverPhase::Inactive {
        let go_events = gp
            .scene
            .game_over
            .update(elapsed_s, overlay_accept, overlay_back);
        if let Some(cue) = go_events.audio_cue {
            handle_game_over_audio(&state.assets, gp, audio.as_deref_mut(), cue);
        }
        if gp.scene.game_over.phase == GameOverPhase::DeathLoop && gp.song_audio_replaced {
            if let Some(engine) = audio.as_deref_mut() {
                if !engine.clock().is_playing() {
                    let _ = engine.seek_ms(0.0);
                    let _ = engine.play();
                }
            }
        }
        match go_events.action {
            GameOverAction::None => {}
            GameOverAction::RetrySong => {
                restart_runtime_song(&state.assets, gp, audio.as_deref_mut(), now, false);
            }
            GameOverAction::ExitToMenu => {
                return_to_menu(state, audio, event_loop);
                return;
            }
        }
    } else if gp.scene.pause_menu.is_open {
        let action = gp.scene.pause_menu.update(elapsed_s, pause_input);
        match action {
            PauseMenuAction::None => {}
            PauseMenuAction::Resume => {
                gp.scene.session.is_paused = false;
                gp.scene.countdown.set_paused(false);
                if let Some(started) = gp.pause_started_at.take() {
                    gp.paused_duration += now.saturating_duration_since(started);
                }
                if let Some(engine) = audio.as_deref_mut() {
                    let _ = engine.stop_bg_loop();
                    if !gp.scene.countdown.active && gp.has_song_audio {
                        let _ = engine.play();
                    }
                }
            }
            PauseMenuAction::RestartSong => {
                if let Some(engine) = audio.as_deref_mut() {
                    let _ = engine.stop_bg_loop();
                }
                restart_runtime_song(&state.assets, gp, audio.as_deref_mut(), now, false);
            }
            PauseMenuAction::ChangeDifficulty(idx) => {
                if let Some(engine) = audio.as_deref_mut() {
                    let _ = engine.stop_bg_loop();
                }
                if let Some(diff) = gp
                    .scene
                    .pause_menu
                    .difficulty_label(idx)
                    .map(str::to_owned)
                {
                    reload_difficulty(&state.assets, gp, audio.as_deref_mut(), &diff, now);
                } else {
                    restart_runtime_song(&state.assets, gp, audio.as_deref_mut(), now, false);
                }
            }
            PauseMenuAction::TogglePractice => {
                gp.scene.session.config.practice_mode = !gp.scene.session.config.practice_mode;
                gp.scene.session.refresh_hud();
            }
            PauseMenuAction::ToggleBotplay => {
                gp.scene.session.config.is_botplay = !gp.scene.session.config.is_botplay;
                gp.scene.session.refresh_hud();
                state.downscroll_pref = gp.scene.session.config.is_downscroll;
                state.botplay_pref = gp.scene.session.config.is_botplay;
            }
            PauseMenuAction::ToggleDownscroll => {
                let next = !gp.scene.session.config.is_downscroll;
                gp.scene.session.set_downscroll(next);
                state.downscroll_pref = next;
                state.botplay_pref = gp.scene.session.config.is_botplay;
            }
            PauseMenuAction::Exit => {
                return_to_menu(state, audio, event_loop);
                return;
            }
        }
    }

    let raw_song_pos_ms = if gp.scene.countdown.active {
        gp.scene.countdown.song_time_ms
    } else if gp.has_song_audio {
        if let Some(engine) = audio.as_deref_mut() {
            if let Some(old_gen) = gp.pending_seek_gen {
                if engine.clock().generation() == old_gen {
                    0.0
                } else {
                    gp.pending_seek_gen = None;
                    engine.clock_mut().position_ms_at(now)
                }
            } else {
                engine.clock_mut().position_ms_at(now)
            }
        } else {
            wall_clock_ms(gp.start_time, gp.paused_duration, now)
        }
    } else {
        let t = wall_clock_ms(gp.start_time, gp.paused_duration, now);
        if gp.loaded_chart.chart.is_none() && t >= 10_000.0 {
            restart_runtime_song(&state.assets, gp, audio.as_deref_mut(), now, false);
            0.0
        } else {
            t
        }
    };
    let effective_song_pos_ms = if gp.scene.countdown.active {
        raw_song_pos_ms
    } else {
        raw_song_pos_ms + f64::from(state.save.options.audio_offset_ms)
    };
    let viz_audible = !state.sound_tray.is_muted() && state.sound_tray.effective_volume() > 0.0;
    gp.scene.scripts.set_visualizer_audible(viz_audible);

    let step_events = gp.scene.update(
        effective_song_pos_ms,
        elapsed_s,
        now,
        &mut gp.inputs,
        audio.as_deref_mut(),
        &gp.visual_style,
    );

    if step_events.countdown_finished_now {
        gp.start_time = now;
        gp.paused_duration = Duration::ZERO;
    }

    if step_events.song_finished_now {
        let is_new = if !gp.scene.session.config.practice_mode && !gp.scene.session.config.is_botplay {
            state.save.record_song_score(
                &gp.loaded_chart.song_id,
                gp.loaded_chart.variation.as_deref(),
                &gp.loaded_chart.difficulty,
                gp.scene.results.score,
                gp.scene.results.tallies,
                gp.scene.results.clear_percent as f32,
                gp.scene.results.rank.label(),
            )
        } else {
            false
        };
        if state.return_target == ReturnTarget::StoryMenu {
            state.story_total_score += gp.scene.results.score;
        }
        gp.scene.results.is_new_highscore = is_new;

        if let Some(engine) = audio.as_deref_mut() {
            let _ = engine.pause();
        }
        let post_req = gp.cutscene.start_post_song(
            &gp.loaded_chart.song_id,
            gp.loaded_chart.variation.as_deref(),
            state.return_target == ReturnTarget::StoryMenu,
            &state.assets,
            &mut state.gpu,
            state.sound_tray.effective_volume(),
        );
        if gp.cutscene.is_active() {
            gp.scene.results.is_active = false;
            gp.waiting_post_cutscene = true;
            dispatch_cutscene_audio(&state.assets, audio.as_deref_mut(), gp.cutscene_sfx, post_req);
        } else {
            activate_results_screen(&state.assets, &mut state.gpu, &state.save, gp, audio.as_deref_mut());
        }
    }

    if let Some(cue) = step_events.game_over_events.audio_cue {
        handle_game_over_audio(&state.assets, gp, audio, cue);
    }

    let hud_zoom = gp.scene.camera_rig.cam_hud.effective_zoom();
    state.stage_sprite_batcher.begin_frame();
    state
        .stage_sprite_batcher
        .set_virtual_projection(&state.gpu.queue);
    state.bg_sprite_batcher.begin_frame();
    state
        .bg_sprite_batcher
        .set_zoomed_virtual_projection(&state.gpu.queue, hud_zoom);
    state.mesh_batcher.begin_frame();
    state
        .mesh_batcher
        .set_zoomed_virtual_projection(&state.gpu.queue, hud_zoom);
    state.fg_sprite_batcher.begin_frame();
    state
        .fg_sprite_batcher
        .set_zoomed_virtual_projection(&state.gpu.queue, hud_zoom);
    state.overlay_sprite_batcher.begin_frame();
    state
        .overlay_sprite_batcher
        .set_virtual_projection(&state.gpu.queue);

    gp.scene.draw(
        &mut state.stage_sprite_batcher,
        &mut state.bg_sprite_batcher,
        &mut state.fg_sprite_batcher,
        &mut state.mesh_batcher,
        &mut state.overlay_sprite_batcher,
        &gp.visual_style,
        Some(&gp.note_assets),
        &state.hud_font,
        state.hud_font_tex,
    );
    if gp.cutscene.is_active() {
        gp.cutscene.draw(
            &mut state.overlay_sprite_batcher,
            &state.hud_font,
            state.hud_font_tex,
        );
    }

    let snap_pos = gp.scene.session.conductor.song_position_ms;
    let snap_dur = gp.song_duration_ms.unwrap_or(0.0);
    let snap_beat = gp.scene.session.conductor.current_beat;
    let snap_step = gp.scene.session.conductor.current_step;
    let snap_bpm = gp.scene.session.conductor.bpm();
    let snap_cam = gp.scene.camera_rig.cam_game.scroll;
    let snap_zoom = gp.scene.camera_rig.cam_game.effective_zoom();
    let snap_bot = gp.scene.session.config.is_botplay;
    let snap_down = gp.scene.session.config.is_downscroll;
    let snap = DebugSnapshot {
        screen_label: gp.loaded_chart.label.as_str(),
        song_pos_ms: snap_pos,
        song_duration_ms: snap_dur,
        beat: snap_beat,
        step: snap_step,
        bpm: snap_bpm,
        cam_pos: snap_cam,
        cam_zoom: snap_zoom,
        sprite_count: state.stage_sprite_batcher.instance_count()
            + state.bg_sprite_batcher.instance_count()
            + state.fg_sprite_batcher.instance_count()
            + state.overlay_sprite_batcher.instance_count(),
        texture_count: state.gpu.active_texture_count(),
        audio_rate_hz,
        is_botplay: snap_bot,
        is_downscroll: snap_down,
    };
    let white_tex = state.gpu.white_texture();
    state.debug_display.draw(
        &mut state.overlay_sprite_batcher,
        white_tex,
        &state.hud_font,
        state.hud_font_tex,
        &snap,
    );
    state
        .sound_tray
        .draw(&mut state.overlay_sprite_batcher, &state.hud_font, state.hud_font_tex);

    state
        .stage_sprite_batcher
        .prepare(&state.gpu.device, &state.gpu.queue);
    state
        .bg_sprite_batcher
        .prepare(&state.gpu.device, &state.gpu.queue);
    state
        .mesh_batcher
        .prepare(&state.gpu.device, &state.gpu.queue);
    state
        .fg_sprite_batcher
        .prepare(&state.gpu.device, &state.gpu.queue);
    state
        .overlay_sprite_batcher
        .prepare(&state.gpu.device, &state.gpu.queue);

    let clear_color = if gp.scene.stage.is_some()
        || gp.scene.game_over.phase != GameOverPhase::Inactive
    {
        wgpu::Color::BLACK
    } else {
        wgpu::Color {
            r: 0.145,
            g: 0.130,
            b: 0.215,
            a: 1.0,
        }
    };
    present_single_batch(state, event_loop, clear_color, true);
}

fn open_external_url(url: &str) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

fn handle_menu_command(
    state: &mut RuntimeState,
    mut audio: Option<&mut AudioEngine>,
    cmd: MenuCommand,
    now: Instant,
) {
    match cmd {
        MenuCommand::None => {}
        MenuCommand::PlaySfx(key) => {
            if let Some(engine) = audio {
                let slot = match key {
                    "scrollMenu" => state.menu_sfx.scroll,
                    "confirmMenu" => state.menu_sfx.confirm,
                    "cancelMenu" => state.menu_sfx.cancel,
                    _ => None,
                };
                if let Some(s) = slot {
                    let _ = engine.play_sfx(s, 0.8);
                }
            }
        }
        MenuCommand::PlaySfxVol(key, vol) => {
            if let Some(engine) = audio {
                let slot = match key {
                    "scrollMenu" => state.menu_sfx.scroll,
                    "confirmMenu" => state.menu_sfx.confirm,
                    "cancelMenu" => state.menu_sfx.cancel,
                    _ => None,
                };
                if let Some(s) = slot {
                    let _ = engine.play_sfx(s, vol);
                }
            }
        }
        MenuCommand::EnterTitle => {
            enter_menu_screen(state, audio, ActiveScreen::Title, true);
        }
        MenuCommand::CancelToTitle => {
            if let Some(engine) = audio.as_deref_mut() {
                if let Some(s) = state.menu_sfx.cancel {
                    let _ = engine.play_sfx(s, 0.8);
                }
            }
            enter_menu_screen(state, audio, ActiveScreen::Title, true);
        }
        MenuCommand::EnterMainMenu => {
            enter_menu_screen(state, audio, ActiveScreen::MainMenu, true);
        }
        MenuCommand::EnterStoryMenu => {
            enter_menu_screen(state, audio, ActiveScreen::StoryMenu, true);
        }
        MenuCommand::EnterFreeplay => {
            enter_menu_screen(state, audio, ActiveScreen::Freeplay, true);
        }
        MenuCommand::EnterOptions => {
            enter_menu_screen(state, audio, ActiveScreen::Options, true);
        }
        MenuCommand::EnterCredits => {
            enter_menu_screen(state, audio.as_deref_mut(), ActiveScreen::Credits, true);
            if let Some(engine) = audio {
                state.menu_music_playing = play_music_by_key(&state.assets, engine, "freeplayRandom")
                    || state.menu_music_playing;
            }
        }
        MenuCommand::ExitCreditsToMainMenu => {
            enter_menu_screen(state, audio.as_deref_mut(), ActiveScreen::MainMenu, true);
            if let Some(engine) = audio {
                if let Some(s) = state.menu_sfx.cancel {
                    let _ = engine.play_sfx(s, 0.8);
                }
                state.menu_music_playing = play_music_by_key(&state.assets, engine, "freakyMenu");
                engine.reclaim_retired();
            }
        }
        MenuCommand::OpenUrl(url) => {
            open_external_url(url);
        }
        MenuCommand::EnterCharSelect => {
            enter_menu_screen(state, audio, ActiveScreen::CharSelect, true);
        }
        MenuCommand::SavePreferencesChanged => {
            if let Some(opt) = &state.options_screen {
                state.save.options = opt.options.clone();
                state.downscroll_pref = state.save.options.downscroll;
                state.botplay_pref = state.save.options.botplay;
                state.debug_display.visible = state.save.options.debug_display;
                let _ = state.save.save_to_disk();
                if let Some(engine) = audio {
                    if let Some(s) = state.menu_sfx.scroll {
                        let _ = engine.play_sfx(s, 0.7);
                    }
                }
            } else if let Some(cs) = &state.char_select_screen {
                state.save.playable_character = cs.selected_character().to_owned();
                let _ = state.save.save_to_disk();
                if let Some(engine) = audio.as_deref_mut() {
                    if let Some(s) = state.menu_sfx.confirm {
                        let _ = engine.play_sfx(s, 0.8);
                    }
                }
                enter_menu_screen(state, audio, ActiveScreen::Freeplay, true);
            }
        }
        MenuCommand::PreviewSong { song_id, variation } => {
            if let Some(engine) = audio {
                let eff_var = state
                    .assets
                    .resolve_song_variation(&song_id, DEFAULT_DIFFICULTY, variation.as_deref());
                let meta_path = state.assets.song_metadata(&song_id, eff_var.as_deref());
                let meta = fs::read_to_string(meta_path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<SongMetadata>(&raw).ok());
                let inst_path =
                    state
                        .assets
                        .resolve_song_inst(&song_id, meta.as_ref(), eff_var.as_deref());
                if let Ok(decoded) = decode_file(&inst_path, engine.sample_rate()) {
                    let tracks = SongTracks::new(Some(TrackStem::new(decoded, 0.0)), None, None);
                    if engine.load_song(tracks, 0.0).is_ok() {
                        let _ = engine.play();
                        state.menu_music_playing = true;
                    }
                }
            }
        }
        MenuCommand::StopPreviewRestoreMenuMusic => {
            enter_menu_screen(state, None, ActiveScreen::MainMenu, true);
            if let Some(engine) = audio {
                if let Some(s) = state.menu_sfx.cancel {
                    let _ = engine.play_sfx(s, 0.8);
                }
                state.menu_music_playing = play_music_by_key(&state.assets, engine, "freakyMenu");
                engine.reclaim_retired();
            }
        }
        MenuCommand::RequestRestartMenuMusic => {
            if let Some(engine) = audio {
                state.menu_music_playing = play_music_by_key(&state.assets, engine, "freakyMenu");
            }
        }
        MenuCommand::StartFreeplaySong {
            song_id,
            difficulty,
            variation,
        } => {
            purge_inactive_resources(state, audio.as_deref_mut(), ActiveScreen::Gameplay);
            state.return_target = ReturnTarget::Freeplay;
            state.screen = ActiveScreen::Gameplay;
            state.story_playlist.clear();
            state.story_level_id.clear();
            state.story_total_score = 0;
            let loaded = load_song_chart(
                &state.assets,
                &song_id,
                &difficulty,
                variation.as_deref(),
            );
            state.gameplay = Some(build_active_gameplay(
                &state.assets,
                &mut state.gpu,
                audio,
                loaded,
                GameplayLaunchOpts {
                    downscroll: state.downscroll_pref,
                    botplay: state.botplay_pref,
                    is_story_mode: false,
                    naughtyness: state.save.options.naughtyness,
                    master_volume: state.sound_tray.effective_volume(),
                    keybinds: KeyBinds::from_names(&state.save.options.keybinds),
                },
                &mut state.menu_sfx,
                now,
            ));
        }
        MenuCommand::StartStoryWeek { songs, difficulty } => {
            if let Some(first_song) = songs.first().cloned() {
                state.story_level_id = state
                    .story_menu_screen
                    .as_ref()
                    .and_then(StoryMenuScreenState::selected_week_id)
                    .unwrap_or("")
                    .to_owned();
                state.story_total_score = 0;
                purge_inactive_resources(state, audio.as_deref_mut(), ActiveScreen::Gameplay);
                state.return_target = ReturnTarget::StoryMenu;
                state.screen = ActiveScreen::Gameplay;
                state.story_playlist = songs;
                state.story_difficulty = difficulty.clone();
                state.story_playlist_idx = 0;
                let loaded = load_song_chart(&state.assets, &first_song, &difficulty, None);
                state.gameplay = Some(build_active_gameplay(
                    &state.assets,
                    &mut state.gpu,
                    audio,
                    loaded,
                    GameplayLaunchOpts {
                        downscroll: state.downscroll_pref,
                        botplay: state.botplay_pref,
                        is_story_mode: true,
                        naughtyness: state.save.options.naughtyness,
                        master_volume: state.sound_tray.effective_volume(),
                        keybinds: KeyBinds::from_names(&state.save.options.keybinds),
                    },
                    &mut state.menu_sfx,
                    now,
                ));
            }
        }
    }
}

fn present_single_batch(
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

fn register_menu_sfx(assets: &AssetLocator, engine: &mut AudioEngine) -> MenuSfxSlots {
    let sample_rate = engine.sample_rate();
    let mut bank = Vec::with_capacity(6);
    let mut slots = MenuSfxSlots::default();
    for (slot_ref, key) in [
        (&mut slots.scroll, "scrollMenu"),
        (&mut slots.confirm, "confirmMenu"),
        (&mut slots.cancel, "cancelMenu"),
        (&mut slots.vol_up, "soundtray/Volup"),
        (&mut slots.vol_down, "soundtray/Voldown"),
        (&mut slots.vol_max, "soundtray/VolMAX"),
    ] {
        let path = assets.sound(key);
        if let Ok(decoded) = decode_file(&path, sample_rate) {
            *slot_ref = Some(bank.len());
            bank.push(decoded);
        }
    }
    if !bank.is_empty() {
        let _ = engine.register_sfx_bank(bank);
    }
    slots
}

fn wall_clock_ms(start_time: Instant, paused_duration: Duration, now: Instant) -> f64 {
    now.saturating_duration_since(start_time)
        .saturating_sub(paused_duration)
        .as_secs_f64()
        * 1000.0
}

fn handle_game_over_audio(
    assets: &AssetLocator,
    gp: &mut ActiveGameplay,
    audio: Option<&mut AudioEngine>,
    cue: GameOverAudioCue,
) {
    let Some(engine) = audio else {
        return;
    };
    match cue {
        GameOverAudioCue::PlayLossSfx => {
            let _ = engine.pause();
            if let Some(slot) = gp.game_over_loss_sfx_slot {
                let _ = engine.play_sfx(slot, 1.0);
            }
        }
        GameOverAudioCue::StartGameOverLoop => {
            let mut buf = StackTextBuffer::<64>::new();
            let primary = gp.scene.game_over.loop_music_key(&mut buf);
            let fallback = "gameplay/gameover/gameOver";
            if play_music_by_key(assets, engine, primary)
                || play_music_by_key(assets, engine, fallback)
                || play_music_by_key(assets, engine, "gameOver")
            {
                gp.song_audio_replaced = true;
            }
        }
        GameOverAudioCue::PlayGameOverEnd => {
            let mut buf = StackTextBuffer::<64>::new();
            let primary = gp.scene.game_over.end_music_key(&mut buf);
            let fallback = "gameplay/gameover/gameOverEnd";
            if play_music_by_key(assets, engine, primary)
                || play_music_by_key(assets, engine, fallback)
                || play_music_by_key(assets, engine, "gameOverEnd")
            {
                gp.song_audio_replaced = true;
            }
        }
        GameOverAudioCue::StopMusic => {
            let _ = engine.pause();
        }
    }
}

fn play_music_by_key(assets: &AssetLocator, engine: &mut AudioEngine, key: &str) -> bool {
    let path = assets.music(key);
    let Ok(decoded) = decode_file(&path, engine.sample_rate()) else {
        return false;
    };
    let tracks = SongTracks::new(Some(TrackStem::new(decoded, 0.0)), None, None);
    engine.load_song(tracks, 0.0).is_ok() && engine.play().is_ok()
}

fn restart_runtime_song(
    assets: &AssetLocator,
    gp: &mut ActiveGameplay,
    mut audio: Option<&mut AudioEngine>,
    now: Instant,
    reset_death_count: bool,
) {
    let mut cfg = gp.scene.session.config.clone();
    cfg.scroll_speed = 0.0;
    let sfx = gp.scene.session.sfx_slots;
    let mut new_session = create_session(&gp.loaded_chart, cfg);
    new_session.sfx_slots = sfx;
    new_session.set_strumline_metrics(
        gp.note_assets.strum_hitbox[1],
        gp.note_assets.strum_offsets[1],
    );

    let song_name = gp.scene.scene_config.song_name.clone();
    let difficulty = gp.loaded_chart.difficulty.clone();

    if gp.song_audio_replaced {
        if let Some(engine) = audio.as_deref_mut() {
            let sr = engine.sample_rate();
            if let Some((dur, inst, off_frames)) =
                try_load_song_audio(assets, engine, sr, &gp.loaded_chart)
            {
                gp.has_song_audio = true;
                gp.song_duration_ms = Some(dur);
                gp.song_audio_replaced = false;
                gp.scene.scripts.set_visualizer_audio(Some((inst, off_frames)));
            }
        }
    }

    let duration = gp.song_duration_ms.or_else(|| {
        gp.loaded_chart
            .chart
            .as_ref()
            .map(|_| compute_chart_end_ms(&new_session))
    });

    gp.scene.restart(
        new_session,
        &song_name,
        &difficulty,
        duration,
        reset_death_count,
    );
    gp.inputs.clear();
    gp.pause_started_at = None;
    gp.paused_duration = Duration::ZERO;
    gp.start_time = now;

    if let Some(engine) = audio {
        if gp.has_song_audio {
            gp.pending_seek_gen = Some(engine.clock().generation());
            let _ = engine.seek_ms(0.0);
            if gp.scene.countdown.active {
                let _ = engine.pause();
            } else {
                let _ = engine.play();
            }
        }
    }
}

fn reload_difficulty(
    assets: &AssetLocator,
    gp: &mut ActiveGameplay,
    audio: Option<&mut AudioEngine>,
    difficulty: &str,
    now: Instant,
) {
    let song_id = gp.loaded_chart.song_id.clone();
    let variation = gp.loaded_chart.variation.clone();
    let reloaded = load_song_chart(assets, &song_id, difficulty, variation.as_deref());
    gp.loaded_chart = reloaded;
    gp.scene.event_queue = gp
        .loaded_chart
        .chart
        .as_ref()
        .map_or_else(SongEventQueue::default, SongEventQueue::from_chart);
    gp.song_audio_replaced = true;
    restart_runtime_song(assets, gp, audio, now, false);
}

fn compute_chart_end_ms(session: &PlaySession) -> f64 {
    let p_end = session
        .player_strumline
        .note_data
        .iter()
        .map(|n| n.time + n.length.max(0.0))
        .fold(0.0_f64, f64::max);
    let o_end = session
        .opponent_strumline
        .note_data
        .iter()
        .map(|n| n.time + n.length.max(0.0))
        .fold(0.0_f64, f64::max);
    p_end.max(o_end) + 1500.0
}

fn load_song_chart(
    assets: &AssetLocator,
    song_id: &str,
    difficulty: &str,
    requested_variation: Option<&str>,
) -> LoadedChart {
    let mut label = StackTextBuffer::<96>::new();
    let effective_var = assets.resolve_song_variation(song_id, difficulty, requested_variation);
    let meta_path = assets.song_metadata(song_id, effective_var.as_deref());
    let chart_path = assets.song_chart(song_id, effective_var.as_deref());

    if let (Ok(meta_raw), Ok(chart_raw)) =
        (fs::read_to_string(&meta_path), fs::read_to_string(&chart_path))
    {
        if let (Ok(meta), Ok(chart)) = (
            serde_json::from_str::<SongMetadata>(&meta_raw),
            serde_json::from_str::<SongChartData>(&chart_raw),
        ) {
            if let Some(var) = effective_var.as_deref() {
                let _ = write!(label, "{song_id} [{difficulty}] ({var})");
            } else {
                let _ = write!(label, "{song_id} [{difficulty}]");
            }
            return LoadedChart {
                song_id: song_id.to_owned(),
                difficulty: difficulty.to_owned(),
                variation: effective_var,
                metadata: Some(meta),
                chart: Some(chart),
                label,
            };
        }
    }

    let _ = write!(label, "Demo Pattern (120 BPM)");
    LoadedChart {
        song_id: song_id.to_owned(),
        difficulty: difficulty.to_owned(),
        variation: effective_var,
        metadata: None,
        chart: None,
        label,
    }
}

#[allow(clippy::type_complexity)]
fn register_all_sfx(
    assets: &AssetLocator,
    engine: &mut AudioEngine,
    sample_rate: u32,
    countdown_assets: &mut CountdownAssets,
    bf_is_pixel: bool,
    stage_id: &str,
) -> (
    GameplaySfxSlots,
    Option<usize>,
    ScriptSfxSlots,
    (Option<usize>, Option<usize>, Option<usize>),
    CutsceneSfxSlots,
) {
    let mut bank = Vec::with_capacity(24);
    let mut slots = GameplaySfxSlots::default();
    let mut script_slots = ScriptSfxSlots::default();

    let miss_names = ["missnote1", "missnote2", "missnote3"];
    for (idx, name) in miss_names.iter().enumerate() {
        let path = assets.sound(name);
        if let Ok(decoded) = decode_file(&path, sample_rate) {
            slots.miss_notes[idx] = Some(bank.len());
            bank.push(decoded);
        }
    }

    let cues = [
        CountdownSfx::Intro3,
        CountdownSfx::Intro2,
        CountdownSfx::Intro1,
        CountdownSfx::IntroGo,
    ];
    for cue in cues {
        let idx = cue.as_index();
        let primary = countdown_assets.audio_key(cue).map(|k| assets.sound(k));
        let candidates = [
            primary,
            Some(assets.sound(cue.as_key())),
            Some(assets.sound(cue.vslice_name())),
        ];
        for cand in candidates.into_iter().flatten() {
            if let Ok(decoded) = decode_file(&cand, sample_rate) {
                countdown_assets.sfx_slots[idx] = Some(bank.len());
                bank.push(decoded);
                break;
            }
        }
    }

    let loss_candidates: [&str; 3] = if bf_is_pixel {
        [
            "gameplay/gameover/fnf_loss_sfx-pixel",
            "gameplay/gameover/fnf_loss_sfx",
            "fnf_loss_sfx-pixel",
        ]
    } else {
        [
            "gameplay/gameover/fnf_loss_sfx",
            "fnf_loss_sfx",
            "fnf_loss_sfx",
        ]
    };
    let mut loss_slot = None;
    for key in loss_candidates {
        let path = assets.sound(key);
        if let Ok(decoded) = decode_file(&path, sample_rate) {
            loss_slot = Some(bank.len());
            bank.push(decoded);
            break;
        }
    }

    let stage_lower = stage_id.to_ascii_lowercase();
    if stage_lower == "spookymansion" || stage_lower == "spookymansionerect" {
        for (idx, key) in ["thunder_1", "thunder_2"].iter().enumerate() {
            if let Ok(decoded) = decode_file(&assets.sound(key), sample_rate) {
                script_slots.thunder[idx] = Some(bank.len());
                bank.push(decoded);
            }
        }
    } else if stage_lower == "phillytrain" || stage_lower == "phillytrainerect" {
        if let Ok(decoded) = decode_file(&assets.sound("train_passes"), sample_rate) {
            script_slots.train_passes = Some(bank.len());
            bank.push(decoded);
        }
    } else if stage_lower == "phillyblazin" {
        for (idx, key) in ["Lightning1", "Lightning2", "Lightning3"].iter().enumerate() {
            if let Ok(decoded) = decode_file(&assets.sound(key), sample_rate) {
                script_slots.lightning[idx] = Some(bank.len());
                bank.push(decoded);
            }
        }
    } else if stage_lower == "sserafim" {
        for (idx, key) in ["doorKick1", "doorKick2"].iter().enumerate() {
            if let Ok(decoded) = decode_file(&assets.sound(key), sample_rate) {
                script_slots.door_kick[idx] = Some(bank.len());
                bank.push(decoded);
            }
        }
    } else if stage_lower == "limoride"
        || stage_lower == "phillystreets"
        || stage_lower == "phillystreetserect"
    {
        let mut week4_assets = assets.clone();
        week4_assets.set_current_level(Some("week4"));
        for (idx, key) in ["carPass0", "carPass1"].iter().enumerate() {
            let decoded = decode_file(&assets.sound(key), sample_rate)
                .or_else(|_| decode_file(&week4_assets.sound(key), sample_rate));
            if let Ok(audio) = decoded {
                script_slots.car_pass[idx] = Some(bank.len());
                bank.push(audio);
            }
        }
    }

    let mut tray_slots = (None, None, None);
    for (slot_ref, key) in [
        (&mut tray_slots.0, "soundtray/Volup"),
        (&mut tray_slots.1, "soundtray/Voldown"),
        (&mut tray_slots.2, "soundtray/VolMAX"),
    ] {
        let path = assets.sound(key);
        if let Ok(decoded) = decode_file(&path, sample_rate) {
            *slot_ref = Some(bank.len());
            bank.push(decoded);
        }
    }

    let mut cs_slots = CutsceneSfxSlots::default();
    for (slot_ref, key) in [
        (&mut cs_slots.pixel_text, "pixelText"),
        (&mut cs_slots.click_text, "clickText"),
        (&mut cs_slots.angry_box, "ANGRY_TEXT_BOX"),
        (&mut cs_slots.senpai_dies, "Senpai_Dies"),
        (&mut cs_slots.lights_turn_on, "Lights_Turn_On"),
        (&mut cs_slots.santa_emotion, "santa_emotion"),
        (&mut cs_slots.santa_shot, "santa_shot_n_falls"),
    ] {
        let path = assets.sound(key);
        if let Ok(decoded) = decode_file(&path, sample_rate) {
            *slot_ref = Some(bank.len());
            bank.push(decoded);
        }
    }

    if !bank.is_empty() {
        let _ = engine.register_sfx_bank(bank);
    }
    (slots, loss_slot, script_slots, tray_slots, cs_slots)
}

fn activate_results_screen(
    assets: &AssetLocator,
    gpu: &mut GpuContext,
    save: &SaveData,
    gp: &mut ActiveGameplay,
    audio: Option<&mut AudioEngine>,
) {
    gp.scene.results.is_active = true;
    let is_pico = gp
        .loaded_chart
        .variation
        .as_deref()
        .is_some_and(|v| v.eq_ignore_ascii_case("pico"))
        || save.playable_character.eq_ignore_ascii_case("pico");
    gp.scene.results.load_visuals(assets, gpu, is_pico);
    if let Some(engine) = audio {
        let _ = engine.pause();
        let _ = engine.stop_bg_loop();
        let results_key = match gp.scene.results.rank {
            ResultsRank::PerfectGold | ResultsRank::Perfect => "results/resultsEXCELLENT",
            ResultsRank::Excellent | ResultsRank::Great | ResultsRank::Good => {
                "results/resultsNORMAL"
            }
            ResultsRank::Loss => "results/resultsSHIT",
        };
        if play_music_by_key(assets, engine, results_key)
            || play_music_by_key(assets, engine, "results/resultsNORMAL")
        {
            gp.song_audio_replaced = true;
        }
    }
}

fn dispatch_cutscene_audio(
    assets: &AssetLocator,
    audio: Option<&mut AudioEngine>,
    sfx: CutsceneSfxSlots,
    req: CutsceneAudioRequest,
) {
    let Some(engine) = audio else {
        return;
    };
    match req {
        CutsceneAudioRequest::None => {}
        CutsceneAudioRequest::StopMusic => {
            let _ = engine.stop_bg_loop();
        }
        CutsceneAudioRequest::PlayMusic { key, .. } => {
            let path = assets.music(key);
            if let Ok(decoded) = decode_file(&path, engine.sample_rate()) {
                let _ = engine.start_bg_loop(decoded);
            }
        }
        CutsceneAudioRequest::PlaySound { key } => {
            let slot = match key {
                "pixelText" => sfx.pixel_text,
                "clickText" => sfx.click_text,
                "ANGRY_TEXT_BOX" => sfx.angry_box,
                "Senpai_Dies" => sfx.senpai_dies,
                "Lights_Turn_On" => sfx.lights_turn_on,
                "santa_emotion" => sfx.santa_emotion,
                "santa_shot_n_falls" => sfx.santa_shot,
                _ => None,
            };
            if let Some(s) = slot {
                let _ = engine.play_sfx(s, 0.9);
            }
        }
    }
}

fn preload_event_icons(
    assets: &AssetLocator,
    gpu: &mut GpuContext,
    scene: &mut PlayScene,
    bf_is_pixel: bool,
    dad_is_pixel: bool,
) {
    struct PreloadIconSpec {
        char_id: i32,
        icon_id: String,
        scale: Option<f32>,
        flip_x: Option<bool>,
        is_pixel: Option<bool>,
        should_bop: Option<bool>,
        offsets: Option<[f32; 2]>,
    }

    let mut to_load: Vec<PreloadIconSpec> = Vec::new();
    for ev in scene.event_queue.events() {
        if let GameEventAction::SetHealthIcon {
            char_id,
            icon_id,
            scale,
            flip_x,
            is_pixel,
            should_bop,
            offsets,
        } = &ev.action
        {
            if (*char_id == 0 || *char_id == 1)
                && !to_load
                    .iter()
                    .any(|spec| spec.char_id == *char_id && spec.icon_id.eq_ignore_ascii_case(icon_id))
            {
                to_load.push(PreloadIconSpec {
                    char_id: *char_id,
                    icon_id: icon_id.clone(),
                    scale: *scale,
                    flip_x: *flip_x,
                    is_pixel: *is_pixel,
                    should_bop: *should_bop,
                    offsets: *offsets,
                });
            }
        }
    }
    for spec in to_load {
        let is_player = spec.char_id == 0;
        let char_pix = if is_player { bf_is_pixel } else { dad_is_pixel };
        let mut icon = HealthIcon::load(assets, gpu, &spec.icon_id, is_player, None, char_pix);
        if let Some(s) = spec.scale {
            icon.base_scale = s.max(0.05);
        }
        if let Some(fx) = spec.flip_x {
            icon.flip_x = if is_player { !fx } else { fx };
        }
        if let Some(px) = spec.is_pixel {
            icon.is_pixel = px;
        }
        if let Some(bop) = spec.should_bop {
            icon.should_bop = bop;
        }
        if let Some(off) = spec.offsets {
            icon.offsets = off;
        }
        scene.register_alternate_icon(spec.char_id, icon);
    }
}

fn try_load_song_audio(
    assets: &AssetLocator,
    engine: &mut AudioEngine,
    sample_rate: u32,
    loaded: &LoadedChart,
) -> Option<(f64, DecodedAudio, i64)> {
    let metadata = loaded.metadata.as_ref();
    let song_id = loaded.song_id.as_str();
    let variation = loaded.variation.as_deref();

    let inst_path = assets.resolve_song_inst(song_id, metadata, variation);
    let inst_audio = decode_file(&inst_path, sample_rate).ok()?;

    let inst_override = metadata
        .map(|m| m.play_data.characters.instrumental.trim())
        .filter(|s| !s.is_empty());

    let player_char = metadata
        .map(|m| m.play_data.characters.player.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_CHARACTER);
    let opponent_char = metadata
        .map(|m| m.play_data.characters.opponent.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("dad");

    let inst_offset = metadata.map_or(0.0, |m| m.offsets.get_instrumental_offset(inst_override));
    let pv_offset = metadata.map_or(0.0, |m| {
        m.offsets.get_vocal_offset(player_char, inst_override)
    });
    let ov_offset = metadata.map_or(0.0, |m| {
        m.offsets.get_vocal_offset(opponent_char, inst_override)
    });

    let player_vocals_list = metadata.and_then(|m| m.play_data.characters.player_vocals.as_deref());
    let opponent_vocals_list =
        metadata.and_then(|m| m.play_data.characters.opponent_vocals.as_deref());

    let mut player_vocals = assets
        .resolve_song_voices_for(song_id, player_vocals_list, player_char, variation)
        .and_then(|p| decode_file(&p, sample_rate).ok())
        .map(|a| TrackStem::new(a, pv_offset));
    let opponent_vocals = assets
        .resolve_song_voices_for(song_id, opponent_vocals_list, opponent_char, variation)
        .and_then(|p| decode_file(&p, sample_rate).ok())
        .map(|a| TrackStem::new(a, ov_offset));

    if player_vocals.is_none() && opponent_vocals.is_none() {
        player_vocals = assets
            .resolve_song_voices_fallback(song_id, variation)
            .and_then(|p| decode_file(&p, sample_rate).ok())
            .map(|a| TrackStem::new(a, pv_offset));
    }

    let inst_stem = TrackStem::new(inst_audio, inst_offset);
    let inst_for_viz = inst_stem.audio.clone();
    let inst_offset_frames = inst_stem.offset_frames;
    let tracks = SongTracks::new(
        Some(inst_stem),
        player_vocals,
        opponent_vocals,
    );
    let duration_ms = (tracks.duration_frames as f64 / f64::from(sample_rate.max(1))) * 1000.0;

    engine.load_song(tracks, 0.0).ok()?;
    Some((duration_ms, inst_for_viz, inst_offset_frames))
}

fn create_session(loaded: &LoadedChart, config: PlayConfig) -> PlaySession {
    if let (Some(meta), Some(chart)) = (&loaded.metadata, &loaded.chart) {
        return PlaySession::new(meta, chart, &loaded.difficulty, config);
    }

    let mut player_notes = Vec::new();
    let mut opponent_notes = Vec::new();
    for step in 0..16 {
        let time = 1200.0 + f64::from(step) * 500.0;
        let dir = step % 4;
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

fn load_hud_font_optional(assets: &AssetLocator, gpu: &mut GpuContext) -> (BitmapFont, TextureHandle) {
    let fnt_path = assets.font("vcr-bmp.fnt");
    let png_path = assets.font("vcr-bmp.png");
    if let (Ok(fnt_str), Ok(png_bytes)) = (fs::read_to_string(&fnt_path), fs::read(&png_path)) {
        if let Ok(pair) =
            BitmapFont::create_bmfont_on_gpu(gpu, Some("vcr-bmp"), &fnt_str, &png_bytes, false)
        {
            return pair;
        }
    }
    BitmapFont::create_embedded_on_gpu(gpu)
        .unwrap_or_else(|_| (BitmapFont::embedded_fallback().0, gpu.white_texture()))
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
