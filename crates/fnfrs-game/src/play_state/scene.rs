use std::time::Instant;

use fnfrs_audio::{AudioCommand, AudioEngine};
use fnfrs_core::constants::{COUNTDOWN_VOLUME, MS_PER_SEC, VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_render::{BitmapFont, BlendMode, FilterMode, MeshBatcher, SpriteBatcher, SpriteInstance, TextureHandle};

use super::session::{PlaySession, PlayStepEvents};
use crate::camera::{CameraRig, FlxEase, DEFAULT_STAGE_ZOOM};
use crate::character::BaseCharacter;
use crate::countdown::{CountdownAssets, CountdownController, CountdownSfx, CountdownStep};
use crate::events::{
    AnimationTarget, CameraFocusTarget, FocusCameraEase, GameEventAction, QueuedSongEvent,
    ScrollSpeedTarget, SongEventQueue, ZoomMode,
};
use crate::hud::{
    GameOverEvents, GameOverPhase, GameOverState, HealthBarLayout, HealthIcon, PauseMenuState,
    ResultsSummaryState,
};
use crate::input::InputQueue;
use crate::note_style::NoteStyleAssets;
use crate::scripts::NativeScriptHost;
use crate::stage::Stage;
use crate::strumline::{Strumline, StrumlineVisualStyle};
/// Initialization parameters for [`PlayScene`].
#[derive(Debug, Clone)]
pub struct PlaySceneConfig {
    pub song_name: String,
    pub difficulty: String,
    pub start_countdown: bool,
    pub song_duration_ms: Option<f64>,
    pub death_count: u32,
}

impl Default for PlaySceneConfig {
    fn default() -> Self {
        Self {
            song_name: "Unknown".to_owned(),
            difficulty: "normal".to_owned(),
            start_countdown: true,
            song_duration_ms: None,
            death_count: 0,
        }
    }
}

/// Per-frame events emitted by [`PlayScene::update`].
#[derive(Debug, Clone, Copy, Default)]
pub struct PlaySceneStepEvents {
    pub gameplay: PlayStepEvents,
    pub countdown_step: Option<CountdownStep>,
    pub countdown_sfx: Option<CountdownSfx>,
    pub countdown_finished_now: bool,
    pub game_over_events: GameOverEvents,
    pub song_finished_now: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct ScrollSpeedTween {
    active: bool,
    start: f64,
    target: f64,
    elapsed_s: f32,
    duration_s: f32,
    ease: FlxEase,
}

impl ScrollSpeedTween {
    fn start(&mut self, current: f64, target: f64, duration_s: f32, ease: FlxEase) -> Option<f64> {
        if duration_s <= 0.0 || ease == FlxEase::Instant {
            self.active = false;
            return Some(target);
        }
        self.active = true;
        self.start = current;
        self.target = target;
        self.elapsed_s = 0.0;
        self.duration_s = duration_s;
        self.ease = ease;
        None
    }

    fn update(&mut self, elapsed_s: f32) -> Option<f64> {
        if !self.active {
            return None;
        }
        self.elapsed_s += elapsed_s.max(0.0);
        let raw_t = if self.duration_s > 0.0 {
            (self.elapsed_s / self.duration_s).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let eased = f64::from(self.ease.eval(raw_t));
        let value = self.start + (self.target - self.start) * eased;
        if raw_t >= 1.0 {
            self.active = false;
            return Some(self.target);
        }
        Some(value)
    }
}

#[derive(Debug, Clone)]
struct AlternateHealthIcon {
    char_id: i32,
    icon: HealthIcon,
}

/// Complete V-Slice gameplay scene coordinating [`PlaySession`], [`Stage`],
/// [`BaseCharacter`]s, [`CameraRig`], [`SongEventQueue`], [`HealthIcon`]s,
/// [`CountdownController`], [`PauseMenuState`], [`GameOverState`], and
/// [`ResultsSummaryState`] with **zero per-frame heap allocations**.
#[derive(Debug)]
pub struct PlayScene {
    pub session: PlaySession,
    pub stage: Option<Stage>,
    pub bf: Option<BaseCharacter>,
    pub dad: Option<BaseCharacter>,
    pub gf: Option<BaseCharacter>,
    pub camera_rig: CameraRig,
    pub event_queue: SongEventQueue,
    pub icon_p1: HealthIcon,
    pub icon_p2: HealthIcon,
    default_p1_id: String,
    default_p2_id: String,
    alt_icons: Vec<AlternateHealthIcon>,
    pub countdown: CountdownController,
    pub countdown_assets: Option<CountdownAssets>,
    pub pause_menu: PauseMenuState,
    pub game_over: GameOverState,
    pub results: ResultsSummaryState,
    pub scene_config: PlaySceneConfig,
    pub scripts: NativeScriptHost,

    base_scroll_speed: f64,
    health_bar_size: [f32; 2],
    player_scroll_tween: ScrollSpeedTween,
    opponent_scroll_tween: ScrollSpeedTween,
}

impl PlayScene {
    /// Builds a [`PlayScene`], places characters on `stage`, initializes cameras,
    /// and optionally starts the 4-beat pre-roll countdown.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        session: PlaySession,
        mut stage: Option<Stage>,
        mut bf: Option<BaseCharacter>,
        mut dad: Option<BaseCharacter>,
        mut gf: Option<BaseCharacter>,
        event_queue: SongEventQueue,
        icon_p1: HealthIcon,
        icon_p2: HealthIcon,
        countdown_assets: Option<CountdownAssets>,
        scene_config: PlaySceneConfig,
    ) -> Self {
        if let Some(st) = &mut stage {
            st.reset_stage(bf.as_mut(), dad.as_mut(), gf.as_mut());
        }
        if let Some(player) = &mut bf {
            player.is_dead = false;
        }

        let stage_zoom = stage.as_ref().map_or(DEFAULT_STAGE_ZOOM, |s| s.camera_zoom);
        let mut camera_rig = CameraRig::new(stage_zoom);
        let initial_focus = initial_camera_focus(bf.as_ref(), dad.as_ref(), gf.as_ref());
        camera_rig.cam_game.snap_to(initial_focus[0], initial_focus[1]);

        let pause_menu = PauseMenuState::new(&scene_config.song_name, &scene_config.difficulty);
        let mut game_over = GameOverState::new();
        game_over.death_count = scene_config.death_count;
        let bf_is_pixel = bf.as_ref().is_some_and(|c| c.is_pixel);
        game_over.set_suffix(if bf_is_pixel { "-pixel" } else { "" }, bf_is_pixel);

        let beat_ms = session.conductor.beat_length_ms();
        let mut countdown = CountdownController::new(beat_ms);
        if scene_config.start_countdown {
            countdown.start_at(beat_ms, session.config.start_timestamp_ms);
        } else {
            countdown.skip();
        }

        let base_scroll_speed = session.config.scroll_speed.max(0.1);

        let default_p1_id = icon_p1.character_id.clone();
        let default_p2_id = icon_p2.character_id.clone();

        Self {
            session,
            stage,
            bf,
            dad,
            gf,
            camera_rig,
            event_queue,
            icon_p1,
            icon_p2,
            default_p1_id,
            default_p2_id,
            alt_icons: Vec::new(),
            countdown,
            countdown_assets,
            pause_menu,
            game_over,
            results: ResultsSummaryState::new(),
            scene_config,
            scripts: NativeScriptHost::empty(),
            base_scroll_speed,
            health_bar_size: [601.0, 19.0],
            player_scroll_tween: ScrollSpeedTween::default(),
            opponent_scroll_tween: ScrollSpeedTween::default(),
        }
    }

    /// Attaches a populated [`NativeScriptHost`] and runs initial stage setup hooks.
    pub fn attach_scripts(&mut self, mut scripts: NativeScriptHost) {
        if let Some(dur) = self.scene_config.song_duration_ms {
            scripts.set_song_duration_ms(dur);
        }
        self.session.hide_opponent_strumline = scripts.hides_opponent_strumline();
        self.session.hide_popups = scripts.hides_popups();
        if scripts.centers_player_strumline() {
            self.session.player_strumline.center_horizontally();
        }
        scripts.on_stage_ready(
            self.stage.as_mut(),
            self.bf.as_mut(),
            self.dad.as_mut(),
            self.gf.as_mut(),
        );
        self.scripts = scripts;
    }

    /// Registers a preloaded alternate health icon at song load time for
    /// zero-allocation runtime swaps triggered by `SetHealthIcon` events.
    pub fn register_alternate_icon(&mut self, char_id: i32, icon: HealthIcon) {
        self.alt_icons.push(AlternateHealthIcon { char_id, icon });
    }

    /// Updates cached health bar dimensions used to anchor [`HealthIcon`]s.
    pub fn set_health_bar_size(&mut self, width: f32, height: f32) {
        self.health_bar_size = [width.max(10.0), height.max(10.0)];
    }

    /// Resets scene state for song restart or difficulty reload without reallocating pools.
    pub fn restart(
        &mut self,
        new_session: PlaySession,
        song_name: &str,
        difficulty: &str,
        song_duration_ms: Option<f64>,
        reset_death_count: bool,
    ) {
        swap_health_icon(
            &mut self.icon_p1,
            &mut self.alt_icons,
            0,
            &self.default_p1_id,
            None,
            None,
            None,
            None,
            None,
        );
        swap_health_icon(
            &mut self.icon_p2,
            &mut self.alt_icons,
            1,
            &self.default_p2_id,
            None,
            None,
            None,
            None,
            None,
        );
        self.session = new_session;
        self.scene_config.song_name.clear();
        self.scene_config.song_name.push_str(song_name);
        self.scene_config.difficulty.clear();
        self.scene_config.difficulty.push_str(difficulty);
        self.scene_config.song_duration_ms = song_duration_ms;

        self.base_scroll_speed = self.session.config.scroll_speed.max(0.1);
        self.player_scroll_tween = ScrollSpeedTween::default();
        self.opponent_scroll_tween = ScrollSpeedTween::default();
        self.event_queue.reset_to_time_ms(0.0);

        if let Some(st) = &mut self.stage {
            st.reset_stage(self.bf.as_mut(), self.dad.as_mut(), self.gf.as_mut());
        } else {
            if let Some(c) = &mut self.bf {
                c.reset_character(true);
            }
            if let Some(c) = &mut self.dad {
                c.reset_character(true);
            }
            if let Some(c) = &mut self.gf {
                c.reset_character(true);
            }
        }
        self.scripts.reset(self.stage.as_mut());
        if let Some(player) = &mut self.bf {
            player.is_dead = false;
        }

        self.camera_rig.reset();
        let focus = initial_camera_focus(self.bf.as_ref(), self.dad.as_ref(), self.gf.as_ref());
        self.camera_rig.cam_game.snap_to(focus[0], focus[1]);

        self.pause_menu.set_song_info(song_name, difficulty);
        self.pause_menu.close();
        self.game_over.reset();
        if reset_death_count {
            self.game_over.death_count = 0;
        }
        self.results.hide();

        let beat_ms = self.session.conductor.beat_length_ms();
        if self.scene_config.start_countdown {
            self.countdown
                .start_at(beat_ms, self.session.config.start_timestamp_ms);
        } else {
            self.countdown.skip();
        }
    }

    /// Advances gameplay, countdown, song events, characters, stage, camera rig,
    /// and health icons for one frame with **zero heap allocations**.
    pub fn update(
        &mut self,
        raw_song_pos_ms: f64,
        elapsed_s: f32,
        frame_timestamp: Instant,
        inputs: &mut InputQueue,
        mut audio: Option<&mut AudioEngine>,
        style: &StrumlineVisualStyle,
    ) -> PlaySceneStepEvents {
        let mut out = PlaySceneStepEvents::default();

        if self.game_over.phase != GameOverPhase::Inactive {
            inputs.clear();
            self.camera_rig.update(elapsed_s);
            if let Some(bf) = &mut self.bf {
                let target_anim = match self.game_over.phase {
                    GameOverPhase::FirstDeath => "firstDeath",
                    GameOverPhase::DeathLoop => "deathLoop",
                    GameOverPhase::ConfirmRetry | GameOverPhase::Exiting => "deathConfirm",
                    GameOverPhase::Inactive => "firstDeath",
                };
                if bf.current_animation() != target_anim && bf.has_animation(target_anim) {
                    bf.play_animation(target_anim, true, true);
                }
                bf.update(elapsed_s, self.session.conductor.beat_length_ms(), false);
            }
            return out;
        }

        if self.pause_menu.is_open || self.results.is_active || self.session.is_paused {
            inputs.clear();
            return out;
        }

        let effective_song_pos_ms = if self.countdown.active {
            let is_pixel = style.is_pixel
                || self
                    .countdown_assets
                    .as_ref()
                    .is_some_and(|a| a.default_pixel);
            let tick = self.countdown.update(elapsed_s, is_pixel);
            out.countdown_step = tick.entered_step;
            out.countdown_sfx = tick.sfx;
            out.countdown_finished_now = tick.finished_now;

            if let Some(sfx) = tick.sfx {
                if let Some(slot) = self
                    .countdown_assets
                    .as_ref()
                    .and_then(|a| a.sfx_slots[sfx.as_index()])
                {
                    if let Some(ref mut engine) = audio {
                        let _ = engine.send(AudioCommand::PlaySfx {
                            slot,
                            gain: COUNTDOWN_VOLUME,
                        });
                    }
                }
            }

            if let Some(step) = tick.entered_step {
                let is_countdown_beat = matches!(
                    step,
                    CountdownStep::Three
                        | CountdownStep::Two
                        | CountdownStep::One
                        | CountdownStep::Go
                );
                if is_countdown_beat {
                    self.icon_p1.bop();
                    self.icon_p2.bop();
                }
            }

            if tick.finished_now {
                if let Some(ref mut engine) = audio {
                    let _ = engine.play();
                }
                self.session.config.start_timestamp_ms
            } else {
                self.countdown.song_time_ms
            }
        } else {
            raw_song_pos_ms
        };

        let gameplay = self.session.update(
            effective_song_pos_ms,
            elapsed_s,
            frame_timestamp,
            inputs,
            audio.as_deref_mut(),
            style,
        );
        out.gameplay = gameplay;

        if gameplay.died {
            self.session.is_paused = true;
            if let Some(bf) = &mut self.bf {
                bf.is_dead = true;
                bf.play_animation("firstDeath", true, true);
                let focus = bf.get_camera_focus_point();
                self.camera_rig.cam_game.set_target(focus[0], focus[1]);
                self.camera_rig.cam_game.default_zoom = 1.05;
            }
            out.game_over_events = self.game_over.trigger_death();
            return out;
        }

        let song_pos = self.session.conductor.song_position_ms;
        if !self.countdown.active || song_pos >= 0.0 {
            let step_len_ms = self.session.conductor.step_length_ms();
            let base_scroll = self.base_scroll_speed;
            let camera_rig = &mut self.camera_rig;
            let bf = &mut self.bf;
            let dad = &mut self.dad;
            let gf = &mut self.gf;
            let stage = &mut self.stage;
            let player_strumline = &mut self.session.player_strumline;
            let opponent_strumline = &mut self.session.opponent_strumline;
            let player_tween = &mut self.player_scroll_tween;
            let opponent_tween = &mut self.opponent_scroll_tween;
            let icon_p1 = &mut self.icon_p1;
            let icon_p2 = &mut self.icon_p2;
            let alt_icons = &mut self.alt_icons;
            let scripts = &mut self.scripts;

            let mut pending_flash_dur: Option<f32> = None;
            let mut event_cues = crate::scripts::ScriptStepOutput::default();
            self.event_queue.update(song_pos, |ev| {
                if let GameEventAction::FocusCamera { target, .. } = &ev.action {
                    scripts.on_focus_character(
                        *target == CameraFocusTarget::Player,
                        *target == CameraFocusTarget::Opponent,
                    );
                } else if let GameEventAction::Custom { event_kind, value } = &ev.action {
                    let c = scripts.on_song_event(
                        event_kind,
                        value,
                        stage.as_mut(),
                        bf.as_mut(),
                        dad.as_mut(),
                        &mut pending_flash_dur,
                    );
                    for cue in c.cues.iter().take(c.count).flatten() {
                        event_cues.push(cue.slot, cue.gain);
                    }
                }
                apply_song_event(
                    ev,
                    step_len_ms,
                    base_scroll,
                    camera_rig,
                    bf.as_mut(),
                    dad.as_mut(),
                    gf.as_mut(),
                    stage.as_mut(),
                    player_strumline,
                    opponent_strumline,
                    player_tween,
                    opponent_tween,
                    icon_p1,
                    icon_p2,
                    alt_icons,
                );
            });
            if let Some(dur) = pending_flash_dur {
                self.camera_rig.cam_game.flash([1.0, 1.0, 1.0, 1.0], dur);
            }
            if let Some(ref mut engine) = audio {
                for cue in event_cues.cues.iter().take(event_cues.count).flatten() {
                    let _ = engine.send(AudioCommand::PlaySfx {
                        slot: cue.slot,
                        gain: cue.gain,
                    });
                }
            }
        }

        if let Some(speed) = self.player_scroll_tween.update(elapsed_s) {
            self.session.player_strumline.scroll_speed = speed.max(0.1);
        }
        if let Some(speed) = self.opponent_scroll_tween.update(elapsed_s) {
            self.session.opponent_strumline.scroll_speed = speed.max(0.1);
        }

        if let Some(dir) = gameplay.player_hit_dir {
            self.scripts.on_note_hit(
                true,
                dir,
                gameplay.player_hit_kind,
                self.bf.as_mut(),
                self.stage.as_ref(),
            );
        } else if gameplay.missed_note || gameplay.ghost_missed {
            if let Some(dir) = gameplay.missed_dir {
                if let Some(bf) = &mut self.bf {
                    bf.play_sing_animation(dir, true, "");
                }
            }
        }

        if let Some(dir) = gameplay.opponent_hit_dir {
            self.scripts.on_note_hit(
                false,
                dir,
                gameplay.opponent_hit_kind,
                self.dad.as_mut(),
                self.stage.as_ref(),
            );
        }

        let player_holding_sustain = self.session.is_player_holding_sustain();
        let is_holding_player = player_holding_sustain || self.session.is_player_holding_key();
        if player_holding_sustain {
            if let Some(bf) = &mut self.bf {
                bf.reset_hold_timer();
            }
        }
        if self.session.is_opponent_holding_sustain() {
            if let Some(dad) = &mut self.dad {
                dad.reset_hold_timer();
            }
        }

        {
            let cur_step = self.session.conductor.current_step;
            let cur_beat = self.session.conductor.current_beat;

            if gameplay.conductor.step_hit {
                self.camera_rig.on_step_hit(cur_step);
                if let Some(st) = &mut self.stage {
                    st.on_step_hit(cur_step);
                }
                if let Some(bf) = &mut self.bf {
                    bf.on_step_hit(cur_step, is_holding_player);
                }
                if let Some(dad) = &mut self.dad {
                    dad.on_step_hit(cur_step, false);
                }
                if let Some(gf) = &mut self.gf {
                    gf.on_step_hit(cur_step, false);
                }
                self.icon_p1.on_step_hit(cur_step);
                self.icon_p2.on_step_hit(cur_step);
            }

            if gameplay.conductor.beat_hit {
                let cues = self.scripts.on_beat_hit(
                    cur_beat,
                    self.stage.as_mut(),
                    self.bf.as_mut(),
                    self.dad.as_mut(),
                    self.gf.as_mut(),
                );
                if let Some(ref mut engine) = audio {
                    for cue in cues.cues.iter().take(cues.count).flatten() {
                        let _ = engine.send(AudioCommand::PlaySfx {
                            slot: cue.slot,
                            gain: cue.gain,
                        });
                    }
                }
                if let Some(st) = &mut self.stage {
                    st.on_beat_hit(cur_beat);
                }
                if let Some(bf) = &mut self.bf {
                    bf.on_beat_hit(cur_beat, is_holding_player);
                }
                if let Some(dad) = &mut self.dad {
                    dad.on_beat_hit(cur_beat, false);
                }
                if !self.scripts.owns_gf_dance() {
                    if let Some(gf) = &mut self.gf {
                        gf.on_beat_hit(cur_beat, false);
                    }
                }
            }
        }

        let beat_len_ms = self.session.conductor.beat_length_ms();
        let update_cues = self.scripts.update(
            elapsed_s,
            song_pos,
            beat_len_ms,
            self.session.health,
            self.stage.as_mut(),
            self.bf.as_mut(),
            self.dad.as_mut(),
            self.gf.as_mut(),
        );
        if let Some(ref mut engine) = audio {
            for cue in update_cues.cues.iter().take(update_cues.count).flatten() {
                let _ = engine.send(AudioCommand::PlaySfx {
                    slot: cue.slot,
                    gain: cue.gain,
                });
            }
        }
        if let Some(st) = &mut self.stage {
            st.update(elapsed_s);
        }
        if let Some(bf) = &mut self.bf {
            bf.update(elapsed_s, beat_len_ms, is_holding_player);
        }
        if let Some(dad) = &mut self.dad {
            dad.update(elapsed_s, beat_len_ms, false);
        }
        if let Some(gf) = &mut self.gf {
            gf.update(elapsed_s, beat_len_ms, false);
        }
        self.camera_rig.update(elapsed_s);

        let bar_layout = HealthBarLayout::compute(
            self.health_bar_size[0],
            self.health_bar_size[1],
            self.session.config.is_downscroll,
            self.session.health_lerp,
        );
        self.icon_p1
            .update(elapsed_s, self.session.health_lerp, &bar_layout);
        self.icon_p2
            .update(elapsed_s, self.session.health_lerp, &bar_layout);

        if !self.countdown.active && !self.session.is_player_dying && !self.results.is_active {
            if let Some(end_ms) = self.scene_config.song_duration_ms {
                if song_pos >= end_ms {
                    self.results.show(
                        &self.scene_config.song_name,
                        &self.scene_config.difficulty,
                        self.session.score(),
                        self.session.tallies,
                        false,
                    );
                    out.song_finished_now = true;
                }
            }
        }

        out
    }

    /// Queues world (`stage_sprites`), HUD (`bg_sprites`, `mesh_batcher`, `fg_sprites`),
    /// and overlay (`overlay_sprites`) geometry with **zero heap allocations**.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        stage_sprites: &mut SpriteBatcher,
        bg_sprites: &mut SpriteBatcher,
        fg_sprites: &mut SpriteBatcher,
        mesh_batcher: &mut MeshBatcher,
        overlay_sprites: &mut SpriteBatcher,
        style: &StrumlineVisualStyle,
        note_assets: Option<&NoteStyleAssets>,
        font: &BitmapFont,
        font_tex: TextureHandle,
    ) {
        let cam_pos = [
            self.camera_rig.cam_game.scroll[0] - self.camera_rig.cam_game.shake_offset[0],
            self.camera_rig.cam_game.scroll[1] - self.camera_rig.cam_game.shake_offset[1],
        ];
        let cam_zoom = self.camera_rig.cam_game.effective_zoom();
        let hud_zoom = self.camera_rig.cam_hud.effective_zoom();

        if self.game_over.phase != GameOverPhase::Inactive {
            if let Some(bf) = &self.bf {
                bf.draw(stage_sprites, cam_pos, cam_zoom);
            }
            self.game_over
                .draw(overlay_sprites, style.white_texture, font, font_tex);
            return;
        }

        if self.results.is_active {
            self.results
                .draw(overlay_sprites, style.white_texture, font, font_tex);
            return;
        }

        self.draw_world_layer(stage_sprites, cam_pos, cam_zoom, hud_zoom);

        self.session.draw(
            bg_sprites,
            fg_sprites,
            mesh_batcher,
            style,
            note_assets,
            font,
            font_tex,
        );
        self.icon_p2.draw(fg_sprites, style.white_texture);
        self.icon_p1.draw(fg_sprites, style.white_texture);
        self.countdown
            .draw(fg_sprites, self.countdown_assets.as_ref());

        if let Some(flash) = self.camera_rig.cam_game.flash_overlay() {
            overlay_sprites.push(
                style.white_texture,
                BlendMode::Normal,
                FilterMode::Nearest,
                SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                    .with_color(flash),
            );
        }
        if let Some(flash) = self.camera_rig.cam_hud.flash_overlay() {
            overlay_sprites.push(
                style.white_texture,
                BlendMode::Normal,
                FilterMode::Nearest,
                SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                    .with_color(flash),
            );
        }

        self.pause_menu
            .draw(overlay_sprites, style.white_texture, font, font_tex);
    }

    fn draw_world_layer(
        &self,
        stage_sprites: &mut SpriteBatcher,
        cam_pos: [f32; 2],
        cam_zoom: f32,
        hud_zoom: f32,
    ) {
        match (self.stage.as_ref(), self.gf.as_ref(), self.dad.as_ref(), self.bf.as_ref()) {
            (Some(st), Some(g), Some(d), Some(b)) => {
                st.draw_with_characters_and_scripts(
                    stage_sprites,
                    cam_pos,
                    cam_zoom,
                    hud_zoom,
                    &[g, d, b],
                    &self.scripts,
                    Some(d),
                    Some(g),
                );
            }
            (Some(st), Some(g), Some(d), None) => {
                st.draw_with_characters_and_scripts(
                    stage_sprites,
                    cam_pos,
                    cam_zoom,
                    hud_zoom,
                    &[g, d],
                    &self.scripts,
                    Some(d),
                    Some(g),
                );
            }
            (Some(st), Some(g), None, Some(b)) => {
                st.draw_with_characters_and_scripts(
                    stage_sprites,
                    cam_pos,
                    cam_zoom,
                    hud_zoom,
                    &[g, b],
                    &self.scripts,
                    None,
                    Some(g),
                );
            }
            (Some(st), None, Some(d), Some(b)) => {
                st.draw_with_characters_and_scripts(
                    stage_sprites,
                    cam_pos,
                    cam_zoom,
                    hud_zoom,
                    &[d, b],
                    &self.scripts,
                    Some(d),
                    None,
                );
            }
            (Some(st), Some(g), None, None) => {
                st.draw_with_characters_and_scripts(
                    stage_sprites,
                    cam_pos,
                    cam_zoom,
                    hud_zoom,
                    &[g],
                    &self.scripts,
                    None,
                    Some(g),
                );
            }
            (Some(st), None, Some(d), None) => {
                st.draw_with_characters_and_scripts(
                    stage_sprites,
                    cam_pos,
                    cam_zoom,
                    hud_zoom,
                    &[d],
                    &self.scripts,
                    Some(d),
                    None,
                );
            }
            (Some(st), None, None, Some(b)) => {
                st.draw_with_characters_and_scripts(
                    stage_sprites,
                    cam_pos,
                    cam_zoom,
                    hud_zoom,
                    &[b],
                    &self.scripts,
                    None,
                    None,
                );
            }
            (Some(st), None, None, None) => {
                st.draw_with_characters_and_scripts(
                    stage_sprites,
                    cam_pos,
                    cam_zoom,
                    hud_zoom,
                    &[],
                    &self.scripts,
                    None,
                    None,
                );
            }
            (None, g, d, b) => {
                draw_optional_chars_sorted(stage_sprites, cam_pos, cam_zoom, [g, d, b]);
            }
        }
    }
}

fn draw_optional_chars_sorted(
    stage_sprites: &mut SpriteBatcher,
    cam_pos: [f32; 2],
    cam_zoom: f32,
    chars: [Option<&BaseCharacter>; 3],
) {
    let mut entries: [(i32, usize); 3] = [(i32::MAX, 0), (i32::MAX, 1), (i32::MAX, 2)];
    let mut count = 0usize;
    for (idx, ch_opt) in chars.iter().enumerate() {
        if let Some(ch) = ch_opt {
            entries[count] = (ch.z_index, idx);
            count += 1;
        }
    }
    for i in 1..count {
        let key = entries[i];
        let mut j = i;
        while j > 0 && entries[j - 1].0 > key.0 {
            entries[j] = entries[j - 1];
            j -= 1;
        }
        entries[j] = key;
    }
    for &(_, idx) in entries.iter().take(count) {
        if let Some(ch) = chars[idx] {
            ch.draw(stage_sprites, cam_pos, cam_zoom);
        }
    }
}

fn initial_camera_focus(
    bf: Option<&BaseCharacter>,
    dad: Option<&BaseCharacter>,
    gf: Option<&BaseCharacter>,
) -> [f32; 2] {
    dad.or(gf)
        .or(bf)
        .map_or([VIRTUAL_WIDTH * 0.5, VIRTUAL_HEIGHT * 0.5], BaseCharacter::get_camera_focus_point)
}

#[allow(clippy::too_many_arguments)]
fn swap_health_icon(
    active: &mut HealthIcon,
    alt_icons: &mut [AlternateHealthIcon],
    char_id: i32,
    icon_id: &str,
    scale: Option<f32>,
    flip_x: Option<bool>,
    is_pixel: Option<bool>,
    should_bop: Option<bool>,
    offsets: Option<[f32; 2]>,
) {
    if !active.character_id.eq_ignore_ascii_case(icon_id) {
        if let Some(entry) = alt_icons
            .iter_mut()
            .find(|a| a.char_id == char_id && a.icon.character_id.eq_ignore_ascii_case(icon_id))
        {
            let prev_x = active.x;
            let prev_y = active.y;
            let prev_bop = active.bop_scale_mult;
            let prev_angle = active.angle_deg;
            std::mem::swap(active, &mut entry.icon);
            active.x = prev_x;
            active.y = prev_y;
            active.bop_scale_mult = prev_bop;
            active.angle_deg = prev_angle;
        }
    }
    if let Some(s) = scale {
        active.base_scale = s.max(0.05);
    }
    if let Some(fx) = flip_x {
        active.flip_x = if active.is_player { !fx } else { fx };
    }
    if let Some(px) = is_pixel {
        active.is_pixel = px;
    }
    if let Some(bop) = should_bop {
        active.should_bop = bop;
    }
    if let Some(off) = offsets {
        active.offsets = off;
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_song_event(
    ev: &QueuedSongEvent,
    step_len_ms: f64,
    base_scroll_speed: f64,
    camera_rig: &mut CameraRig,
    bf: Option<&mut BaseCharacter>,
    dad: Option<&mut BaseCharacter>,
    gf: Option<&mut BaseCharacter>,
    stage: Option<&mut Stage>,
    player_strumline: &mut Strumline,
    opponent_strumline: &mut Strumline,
    player_tween: &mut ScrollSpeedTween,
    opponent_tween: &mut ScrollSpeedTween,
    icon_p1: &mut HealthIcon,
    icon_p2: &mut HealthIcon,
    alt_icons: &mut [AlternateHealthIcon],
) {
    match &ev.action {
        GameEventAction::FocusCamera {
            target,
            offset,
            duration_steps,
            ease,
        } => {
            let base = match target {
                CameraFocusTarget::Position => [0.0, 0.0],
                CameraFocusTarget::Player => bf.map_or(
                    [VIRTUAL_WIDTH * 0.5, VIRTUAL_HEIGHT * 0.5],
                    |c| c.get_camera_focus_point(),
                ),
                CameraFocusTarget::Opponent => dad.map_or(
                    [VIRTUAL_WIDTH * 0.5, VIRTUAL_HEIGHT * 0.5],
                    |c| c.get_camera_focus_point(),
                ),
                CameraFocusTarget::Girlfriend => gf.map_or(
                    [VIRTUAL_WIDTH * 0.5, VIRTUAL_HEIGHT * 0.5],
                    |c| c.get_camera_focus_point(),
                ),
                CameraFocusTarget::Unknown(_) => camera_rig.cam_game.target_follow,
            };
            let tx = base[0] + offset[0];
            let ty = base[1] + offset[1];
            match *ease {
                FocusCameraEase::Classic => camera_rig.cam_game.set_target(tx, ty),
                FocusCameraEase::Tween(flx) => camera_rig.cam_game.tween_to_steps(
                    tx,
                    ty,
                    *duration_steps,
                    step_len_ms,
                    flx,
                ),
            }
        }
        GameEventAction::ZoomCamera {
            zoom,
            duration_steps,
            mode,
            ease,
        } => {
            let direct = matches!(mode, ZoomMode::Direct);
            camera_rig.cam_game.tween_zoom_steps(
                *zoom,
                direct,
                *duration_steps,
                step_len_ms,
                *ease,
            );
        }
        GameEventAction::SetCameraBop {
            rate_beats,
            offset_beats,
            intensity,
        } => {
            camera_rig.set_bop(*rate_beats, *offset_beats, *intensity);
        }
        GameEventAction::PlayAnimation {
            target,
            anim,
            force,
        } => match GameEventAction::resolve_animation_target(target) {
            AnimationTarget::Boyfriend => {
                if let Some(c) = bf {
                    c.play_animation(anim, *force, false);
                }
            }
            AnimationTarget::Dad => {
                if let Some(c) = dad {
                    c.play_animation(anim, *force, false);
                }
            }
            AnimationTarget::Girlfriend => {
                if let Some(c) = gf {
                    c.play_animation(anim, *force, false);
                }
            }
            AnimationTarget::Prop(prop_name) => {
                if let Some(st) = stage {
                    if let Some(prop) = st.get_named_prop_mut(prop_name) {
                        if let Some(bop) = &mut prop.bopper {
                            bop.play_animation(anim, *force);
                        }
                    }
                }
            }
        },
        GameEventAction::ScrollSpeed {
            scroll,
            duration_steps,
            ease,
            strumline,
            absolute,
        } => {
            let target = if *absolute {
                f64::from(*scroll)
            } else {
                base_scroll_speed * f64::from(*scroll)
            }
            .max(0.1);
            let duration_s = ((f64::from(*duration_steps) * step_len_ms) / MS_PER_SEC) as f32;

            if matches!(
                strumline,
                ScrollSpeedTarget::Both | ScrollSpeedTarget::Player
            ) {
                if let Some(immediate) =
                    player_tween.start(player_strumline.scroll_speed, target, duration_s, *ease)
                {
                    player_strumline.scroll_speed = immediate;
                }
            }
            if matches!(
                strumline,
                ScrollSpeedTarget::Both | ScrollSpeedTarget::Opponent
            ) {
                if let Some(immediate) = opponent_tween.start(
                    opponent_strumline.scroll_speed,
                    target,
                    duration_s,
                    *ease,
                ) {
                    opponent_strumline.scroll_speed = immediate;
                }
            }
        }
        GameEventAction::SetHealthIcon {
            char_id,
            icon_id,
            scale,
            flip_x,
            is_pixel,
            should_bop,
            offsets,
        } => match *char_id {
            0 => swap_health_icon(
                icon_p1,
                alt_icons,
                0,
                icon_id,
                *scale,
                *flip_x,
                *is_pixel,
                *should_bop,
                *offsets,
            ),
            1 => swap_health_icon(
                icon_p2,
                alt_icons,
                1,
                icon_id,
                *scale,
                *flip_x,
                *is_pixel,
                *should_bop,
                *offsets,
            ),
            _ => {}
        },
        GameEventAction::Custom { .. } => {}
    }
}

