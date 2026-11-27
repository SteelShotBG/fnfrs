use std::time::{Duration, Instant};

use fnfrs_audio::AudioEngine;
use fnfrs_core::constants::{DEFAULT_CHARACTER, DEFAULT_NOTE_STYLE, DEFAULT_STAGE, DEFAULT_VARIATION};
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::save::SaveData;
use fnfrs_core::scoring::ScoreTallies;
use fnfrs_core::schema::song::SongNoteData;
use fnfrs_game::cutscene::{CutsceneAudioQueue, CutsceneController};
use fnfrs_game::debug_display::DebugSnapshot;
use fnfrs_game::{
    BaseCharacter, CharacterKind, CountdownAssets, GameOverAction, GameOverPhase,
    GameOverState, GameplaySfxSlots, HealthIcon, InputQueue, KeyBinds, NativeScriptHost,
    NoteStyleAssets, PauseMenuAction, PauseMenuInput, PlayConfig, PlayScene, PlaySceneConfig,
    PlaySession, ResultsAction, ResultsRank, ScriptSfxSlots, SongEventQueue, Stage,
    StrumlineVisualStyle,
};
use fnfrs_render::GpuContext;
use winit::event_loop::ActiveEventLoop;

use crate::audio_loader::{
    dispatch_cutscene_audio, handle_game_over_audio, load_song_chart, play_music_by_key,
    preload_event_icons, register_all_sfx, try_load_song_audio, CutsceneSfxSlots, LoadedChart,
    MenuSfxSlots,
};
use crate::menu_router::{purge_inactive_resources, return_to_menu};
use crate::{present_single_batch, ActiveScreen, ReturnTarget, RuntimeState};

pub(crate) struct GameplayLaunchOpts {
    pub downscroll: bool,
    pub botplay: bool,
    pub is_story_mode: bool,
    pub naughtyness: bool,
    pub master_volume: f32,
    pub keybinds: KeyBinds,
}

pub(crate) struct ActiveGameplay {
    pub scene: PlayScene,
    pub level_assets: AssetLocator,
    pub inputs: InputQueue,
    pub note_assets: NoteStyleAssets,
    pub visual_style: StrumlineVisualStyle,
    pub loaded_chart: LoadedChart,
    pub has_song_audio: bool,
    pub song_audio_replaced: bool,
    pub song_duration_ms: Option<f64>,
    pub game_over_loss_sfx_slot: Option<usize>,
    pub cutscene: CutsceneController,
    pub cutscene_sfx: CutsceneSfxSlots,
    pub waiting_post_cutscene: bool,
    pub pending_seek_gen: Option<u64>,
    pub pause_started_at: Option<Instant>,
    pub paused_duration: Duration,
    pub pending_pause_input: PauseMenuInput,
    pub pause_up_held: bool,
    pub pause_down_held: bool,
    pub pause_shift_held: bool,
    pub pending_overlay_accept: bool,
    pub pending_overlay_back: bool,
    pub start_time: Instant,
}

pub(crate) fn build_active_gameplay(
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
    let gf_id = match metadata {
        Some(m) => m.play_data.characters.girlfriend.trim(),
        None => "gf",
    };

    let bf = BaseCharacter::load_by_id(bf_id, CharacterKind::Bf, &level_assets, gpu);
    let dad = BaseCharacter::load_by_id(dad_id, CharacterKind::Dad, &level_assets, gpu);
    let gf = if gf_id.is_empty() || gf_id.eq_ignore_ascii_case("none") {
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

    let mut has_song_audio = false;
    let mut song_duration_ms = None;
    let mut sfx_slots = GameplaySfxSlots::default();
    let mut script_sfx_slots = ScriptSfxSlots::default();
    let mut game_over_loss_sfx_slot = None;
    let mut cutscene_sfx = CutsceneSfxSlots::default();

    if let Some(engine) = audio.as_deref_mut() {
        let sample_rate = engine.sample_rate();
        if loaded_chart.chart.is_some() {
            if let Some(opt_dur) = try_load_song_audio(assets, engine, sample_rate, &loaded_chart)
            {
                has_song_audio = true;
                song_duration_ms = opt_dur;
            }
        }
        let mut go_profile = GameOverState::new();
        go_profile.set_character_profile(bf_id, dad_id, bf_is_pixel);
        let (miss_slots, loss_slot, script_slots, tray_slots, cs_slots) = register_all_sfx(
            &level_assets,
            engine,
            sample_rate,
            &mut countdown_assets,
            go_profile.blue_ball_suffix.as_str(),
            loaded_chart.song_id.as_str(),
            stage_id,
            go_profile.quote_kind,
            opts.naughtyness,
        );
        sfx_slots = miss_slots;
        game_over_loss_sfx_slot = loss_slot;
        script_sfx_slots = script_slots;
        cutscene_sfx = cs_slots;
        menu_sfx.vol_up = tray_slots.0;
        menu_sfx.vol_down = tray_slots.1;
        menu_sfx.vol_max = tray_slots.2;
    }

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
    if let Some(engine) = audio.as_deref() {
        scripts.set_visualizer_history(Some(engine.viz_history()), engine.sample_rate());
    }
    scripts.set_visualizer_audible(opts.master_volume > 0.0);
    scene.attach_scripts(scripts);
    scene.pause_menu.load_atlas_font(&level_assets, gpu);
    if let Some(meta) = metadata {
        scene.pause_menu.set_credits(
            meta.artist.as_str(),
            meta.charter.as_deref().unwrap_or("Unknown"),
        );
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
        stage_id,
        bf_id,
        opts.is_story_mode,
        opts.naughtyness,
        &level_assets,
        gpu,
        opts.master_volume,
        &mut scene,
    );
    if cutscene.is_active() {
        scene.session.is_paused = true;
        scene.countdown.set_paused(true);
        scene.hide_hud_for_cutscene = cutscene.hides_hud();
    }
    dispatch_cutscene_audio(&level_assets, audio.as_deref_mut(), cutscene_sfx, pre_req);

    if !cutscene.is_active() && !start_countdown && has_song_audio {
        if let Some(engine) = audio {
            let _ = engine.play();
        }
    }

    ActiveGameplay {
        scene,
        level_assets,
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
        pause_up_held: false,
        pause_down_held: false,
        pause_shift_held: false,
        pending_overlay_accept: false,
        pending_overlay_back: false,
        start_time: now,
    }
}

pub(crate) fn restart_runtime_song(
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
            if let Some(opt_dur) = try_load_song_audio(assets, engine, sr, &gp.loaded_chart) {
                gp.has_song_audio = true;
                if let Some(dur) = opt_dur {
                    gp.song_duration_ms = Some(dur);
                }
                gp.song_audio_replaced = false;
                gp.scene
                    .scripts
                    .set_visualizer_history(Some(engine.viz_history()), sr);
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

pub(crate) fn reload_difficulty(
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

pub(crate) fn update_and_render_gameplay(
    state: &mut RuntimeState,
    mut audio: Option<&mut AudioEngine>,
    event_loop: &ActiveEventLoop,
    elapsed_s: f32,
    now: Instant,
    audio_rate_hz: u32,
) {
    let Some(gp) = state.gameplay.as_mut() else {
        return;
    };

    let overlay_accept = std::mem::take(&mut gp.pending_overlay_accept);
    let overlay_back = std::mem::take(&mut gp.pending_overlay_back);
    let mut pause_input = std::mem::take(&mut gp.pending_pause_input);
    pause_input.up_held = gp.pause_up_held;
    pause_input.down_held = gp.pause_down_held;
    pause_input.shift_held = gp.pause_shift_held;

    if gp.scene.pause_menu.is_open {
        let action = gp.scene.pause_menu.update(elapsed_s, pause_input);
        if gp.scene.pause_menu.scroll_sfx_queued {
            gp.scene.pause_menu.scroll_sfx_queued = false;
            if let (Some(slot), Some(engine)) = (state.menu_sfx.scroll, audio.as_deref_mut()) {
                let _ = engine.play_sfx(slot, 0.4);
            }
        }
        if gp.scene.pause_menu.offset_dirty {
            gp.scene.pause_menu.offset_dirty = false;
            state.save.options.audio_offset_ms = gp.scene.pause_menu.global_offset_ms;
            let _ = state.save.save_to_disk();
        }
        if gp.scene.pause_menu.is_open {
            if let Some(engine) = audio.as_deref_mut() {
                let _ = engine.set_pause_loop_gain(gp.scene.pause_menu.music_gain);
            }
        }
        match action {
            PauseMenuAction::None => {}
            PauseMenuAction::Resume => {
                if let Some(engine) = audio.as_deref_mut() {
                    let _ = engine.stop_pause_loop();
                }
                if gp.cutscene.is_active() {
                    gp.cutscene.set_paused(false);
                    if let Some(engine) = audio.as_deref_mut() {
                        let _ = engine.set_bg_loop_paused(false);
                    }
                } else {
                    gp.scene.session.is_paused = false;
                    gp.scene.countdown.set_paused(false);
                    if let Some(started) = gp.pause_started_at.take() {
                        gp.paused_duration += now.saturating_duration_since(started);
                    }
                    if let Some(engine) = audio.as_deref_mut() {
                        if !gp.scene.countdown.active && gp.has_song_audio {
                            let _ = engine.play();
                        }
                    }
                }
            }
            PauseMenuAction::RestartSong => {
                if let Some(engine) = audio.as_deref_mut() {
                    let _ = engine.stop_pause_loop();
                    let _ = engine.stop_bg_loop();
                }
                restart_runtime_song(&state.assets, gp, audio.as_deref_mut(), now, false);
            }
            PauseMenuAction::ChangeDifficulty(idx) => {
                if let Some(engine) = audio.as_deref_mut() {
                    let _ = engine.stop_pause_loop();
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
                gp.scene.session.config.practice_mode = true;
                gp.scene.session.refresh_hud();
            }
            PauseMenuAction::SkipCutscene | PauseMenuAction::SkipDialogue => {
                if let Some(engine) = audio.as_deref_mut() {
                    let _ = engine.stop_pause_loop();
                }
                let mut q = CutsceneAudioQueue::default();
                if gp.cutscene.skip_active(&mut q) {
                    dispatch_cutscene_audio(&gp.level_assets, audio.as_deref_mut(), gp.cutscene_sfx, q);
                    if gp.waiting_post_cutscene {
                        gp.waiting_post_cutscene = false;
                        if complete_song_or_advance_story(state, audio, now) {
                            return;
                        }
                    } else {
                        gp.scene.session.is_paused = false;
                        gp.scene.countdown.set_paused(false);
                        gp.start_time = now;
                        gp.paused_duration = Duration::ZERO;
                    }
                }
            }
            PauseMenuAction::RestartCutscene | PauseMenuAction::RestartDialogue => {
                if let Some(engine) = audio.as_deref_mut() {
                    let _ = engine.stop_pause_loop();
                    let _ = engine.set_bg_loop_paused(false);
                }
                let mut q = CutsceneAudioQueue::default();
                gp.cutscene.restart_active(&mut q);
                dispatch_cutscene_audio(&gp.level_assets, audio.as_deref_mut(), gp.cutscene_sfx, q);
            }
            PauseMenuAction::Exit => {
                if let Some(engine) = audio.as_deref_mut() {
                    let _ = engine.stop_pause_loop();
                    let _ = engine.stop_bg_loop();
                }
                gp.cutscene.set_paused(false);
                return_to_menu(state, audio, event_loop);
                return;
            }
        }
    } else if gp.cutscene.is_active() {
        let step = gp.cutscene.update_with_scene(
            elapsed_s,
            &state.gpu,
            state.sound_tray.effective_volume(),
            overlay_accept,
            &mut gp.scene,
        );
        dispatch_cutscene_audio(
            &gp.level_assets,
            audio.as_deref_mut(),
            gp.cutscene_sfx,
            step.audio,
        );
        if step.mute_opponent_vocals {
            if let Some(engine) = audio.as_deref_mut() {
                let _ = engine.set_track_gain(fnfrs_audio::TrackKind::OpponentVocals, 0.0);
            }
        }
        if step.finished {
            if gp.waiting_post_cutscene {
                gp.waiting_post_cutscene = false;
                if complete_song_or_advance_story(state, audio, now) {
                    return;
                }
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
            state.story_total_score = 0;
            state.story_tallies = ScoreTallies::default();
            return_to_menu(state, audio, event_loop);
            return;
        }
    } else if gp.scene.game_over.phase != GameOverPhase::Inactive {
        let go_events = gp
            .scene
            .game_over
            .update(elapsed_s, overlay_accept, overlay_back);
        if let Some(cue) = go_events.audio_cue {
            handle_game_over_audio(&gp.level_assets, gp, audio.as_deref_mut(), cue);
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
            combine_story_tallies(&mut state.story_tallies, &gp.scene.results.tallies);
        }
        gp.scene.results.is_new_highscore = is_new;

        if let Some(engine) = audio.as_deref_mut() {
            let _ = engine.pause();
        }
        let post_req = gp.cutscene.start_post_song(
            &gp.loaded_chart.song_id,
            gp.loaded_chart.variation.as_deref(),
            state.return_target == ReturnTarget::StoryMenu,
            &gp.level_assets,
            &mut state.gpu,
            state.sound_tray.effective_volume(),
            &mut gp.scene,
        );
        if gp.cutscene.is_active() {
            gp.scene.session.is_paused = true;
            gp.scene.results.is_active = false;
            gp.scene.hide_hud_for_cutscene = gp.cutscene.hides_hud();
            gp.waiting_post_cutscene = true;
            dispatch_cutscene_audio(&gp.level_assets, audio.as_deref_mut(), gp.cutscene_sfx, post_req);
        } else if complete_song_or_advance_story(state, audio, now) {
            return;
        }
    } else if let Some(cue) = step_events.game_over_events.audio_cue {
        handle_game_over_audio(&gp.level_assets, gp, audio, cue);
    }

    let Some(gp) = state.gameplay.as_mut() else {
        return;
    };
    gp.scene.hide_hud_for_cutscene = gp.cutscene.is_active() && gp.cutscene.hides_hud();

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

    let cam_pos = [
        gp.scene.camera_rig.cam_game.scroll[0] - gp.scene.camera_rig.cam_game.shake_offset[0],
        gp.scene.camera_rig.cam_game.scroll[1] - gp.scene.camera_rig.cam_game.shake_offset[1],
    ];
    let cam_zoom = gp.scene.camera_rig.cam_game.effective_zoom();
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
        gp.cutscene
            .draw_world(&mut state.stage_sprite_batcher, cam_pos, cam_zoom);
        gp.cutscene.draw(
            &mut state.overlay_sprite_batcher,
            &state.hud_font,
            state.hud_font_tex,
        );
    }
    gp.scene.draw_pause_overlay(
        &mut state.overlay_sprite_batcher,
        gp.visual_style.white_texture,
        &state.hud_font,
        state.hud_font_tex,
    );

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

fn combine_story_tallies(total: &mut ScoreTallies, song: &ScoreTallies) {
    total.sick = total.sick.saturating_add(song.sick);
    total.good = total.good.saturating_add(song.good);
    total.bad = total.bad.saturating_add(song.bad);
    total.shit = total.shit.saturating_add(song.shit);
    total.missed = total.missed.saturating_add(song.missed);
    total.max_combo = total.max_combo.max(song.max_combo);
    total.total_notes_hit = total.total_notes_hit.saturating_add(song.total_notes_hit);
    total.total_notes = total.total_notes.saturating_add(song.total_notes);
}

/// Advances to the next Story Mode track without showing Results, or displays
/// the final song / combined Story Week Results screen when the playlist ends.
fn complete_song_or_advance_story(
    state: &mut RuntimeState,
    mut audio: Option<&mut AudioEngine>,
    now: Instant,
) -> bool {
    if state.return_target == ReturnTarget::StoryMenu
        && state.story_playlist_idx + 1 < state.story_playlist.len()
    {
        state.story_playlist_idx += 1;
        let next_song = state.story_playlist[state.story_playlist_idx].clone();
        let diff = state.story_difficulty.clone();
        let prev_var = state
            .gameplay
            .as_ref()
            .and_then(|g| g.loaded_chart.variation.clone())
            .unwrap_or_else(|| DEFAULT_VARIATION.to_owned());
        let eff_var = state
            .assets
            .resolve_song_variation(&next_song, &diff, Some(&prev_var));
        purge_inactive_resources(state, audio.as_deref_mut(), ActiveScreen::Title);
        state.screen = ActiveScreen::Gameplay;
        let loaded = load_song_chart(&state.assets, &next_song, &diff, eff_var.as_deref());
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
        return true;
    }

    let Some(gp) = state.gameplay.as_mut() else {
        return false;
    };
    if state.return_target == ReturnTarget::StoryMenu {
        let valid_score =
            !gp.scene.session.config.practice_mode && !gp.scene.session.config.is_botplay;
        let mut is_new_level = false;
        if valid_score && !state.story_level_id.is_empty() {
            let key = SaveData::level_key(&state.story_level_id, &state.story_difficulty);
            let prev = state.save.scores.levels.get(&key).map(|s| s.score);
            is_new_level = prev.is_none_or(|p| state.story_total_score > p);
            state.save.record_level_score(
                &state.story_level_id,
                &state.story_difficulty,
                state.story_total_score,
            );
        }
        let title = if state.story_level_title.is_empty() {
            state.story_level_id.clone()
        } else {
            state.story_level_title.clone()
        };
        gp.scene.results.show(
            &title,
            &state.story_difficulty,
            state.story_total_score,
            state.story_tallies,
            is_new_level,
        );
    }
    activate_results_screen(
        &state.assets,
        &mut state.gpu,
        &state.save,
        gp,
        audio.as_deref_mut(),
    );
    false
}

fn wall_clock_ms(start_time: Instant, paused_duration: Duration, now: Instant) -> f64 {
    now.saturating_duration_since(start_time)
        .saturating_sub(paused_duration)
        .as_secs_f64()
        * 1000.0
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
        || gp.scene
            .bf
            .as_ref()
            .is_some_and(|b| b.id.to_ascii_lowercase().starts_with("pico"))
        || save.playable_character.to_ascii_lowercase().starts_with("pico");
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
