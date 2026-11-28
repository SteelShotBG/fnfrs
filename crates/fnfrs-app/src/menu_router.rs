use std::fs;
use std::time::Instant;

use fnfrs_audio::AudioEngine;
use fnfrs_core::constants::{DEFAULT_DIFFICULTY, DEFAULT_VARIATION};
use fnfrs_core::scoring::ScoreTallies;
use fnfrs_core::schema::song::SongMetadata;
use fnfrs_game::debug_display::DebugSnapshot;
use fnfrs_game::menus::{
    CharSelectScreenState, CreditsScreenState, FreeplayScreenState, MainMenuScreenState,
    MenuCommand, MenuInputState, OptionsScreenState, StoryMenuScreenState, TitleScreenState,
};
use fnfrs_game::KeyBinds;
use winit::event_loop::ActiveEventLoop;

use crate::audio_loader::{
    load_song_chart, play_music_by_key, register_menu_sfx, stream_single_track,
};
use crate::gameplay_host::{build_active_gameplay, GameplayLaunchOpts};
use crate::{present_single_batch, ActiveScreen, ReturnTarget, RuntimeState};

pub(crate) fn purge_inactive_resources(
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
    // Pinned textures (hud_font_tex & SoundTray) remain resident automatically.
    state.gpu.clear_user_textures();
    if let Some(engine) = audio {
        engine.reclaim_retired();
    }
}

pub(crate) fn enter_menu_screen(
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
        if let Some(engine) = audio {
            engine.reclaim_retired();
        }
        match target {
            ActiveScreen::Title => {
                state.title_screen = Some(TitleScreenState::load(
                    &state.assets,
                    &mut state.gpu,
                    skip_title_intro,
                ));
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
                state.story_menu_screen = Some(StoryMenuScreenState::load(
                    &state.assets,
                    &mut state.gpu,
                    &state.save,
                    state.story_menu_level_id.as_deref(),
                    &state.story_menu_difficulty,
                ));
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
                    s.reset_for_enter(&state.save);
                }
            }
            ActiveScreen::Freeplay => {
                if let Some(f) = &mut state.freeplay_screen {
                    f.sync_from_save(&state.assets, &mut state.gpu, &state.save);
                    f.reset_for_enter();
                }
            }
            ActiveScreen::Options
            | ActiveScreen::Credits
            | ActiveScreen::CharSelect
            | ActiveScreen::Gameplay => {}
        }
    }
}

pub(crate) fn return_to_menu(
    state: &mut RuntimeState,
    mut audio: Option<&mut AudioEngine>,
    event_loop: &ActiveEventLoop,
) {
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

pub(crate) fn update_and_draw_menus(
    state: &mut RuntimeState,
    mut audio: Option<&mut AudioEngine>,
    event_loop: &ActiveEventLoop,
    elapsed_s: f32,
    now: Instant,
    audio_rate_hz: u32,
) {
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
                if let Some(id) = s.selected_week_id() {
                    state.story_menu_level_id = Some(id.to_owned());
                }
                state.story_menu_difficulty = s.selected_difficulty().to_owned();
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
                if input.up_pressed
                    || input.down_pressed
                    || input.left_pressed
                    || input.right_pressed
                    || input.tab_pressed
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
}

pub(crate) fn handle_menu_command(
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
                state.debug_display.set_visible(state.save.options.debug_display);
                state.next_frame_deadline = now;
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
                if inst_path.exists() && stream_single_track(engine, inst_path) {
                    state.menu_music_playing = true;
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
            state.story_level_title.clear();
            state.story_total_score = 0;
            state.story_tallies = ScoreTallies::default();
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
        MenuCommand::StartStoryWeek {
            level_id,
            level_title,
            songs,
            difficulty,
        } => {
            if let Some(first_song) = songs.first().cloned() {
                state.story_level_id = level_id;
                state.story_level_title = level_title;
                state.story_total_score = 0;
                state.story_tallies = ScoreTallies::default();
                purge_inactive_resources(state, audio.as_deref_mut(), ActiveScreen::Gameplay);
                state.return_target = ReturnTarget::StoryMenu;
                state.screen = ActiveScreen::Gameplay;
                state.story_playlist = songs;
                state.story_difficulty = difficulty.clone();
                state.story_playlist_idx = 0;
                let eff_var =
                    state
                        .assets
                        .resolve_song_variation(&first_song, &difficulty, Some(DEFAULT_VARIATION));
                let loaded =
                    load_song_chart(&state.assets, &first_song, &difficulty, eff_var.as_deref());
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
