//! Pre-song & post-song V-Slice cutscene coordinator (video, Week 6 dialogue, in-game scripted scenes).

pub mod common;
pub mod darnell_intro;
pub mod eggnog_erect;
pub mod philly_doppel;
pub mod sserafim_intro;
pub mod stress_pico;

use fnfrs_audio::AudioEngine;
use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{
    BitmapFont, BlendMode, FilterMode, GpuContext, SpriteBatcher, SpriteInstance, TextureHandle,
};

use crate::character::{build_loaded_anim, draw_atlas_frame, load_sparrow_source, AnimBank, AtlasDrawParams};
use crate::dialogue::{DialogueAction, DialogueState};
use crate::hud::PauseMenuMode;
use crate::play_state::PlayScene;
use crate::video::{VideoCutsceneAction, VideoCutsceneState};

pub use common::{CutsceneAudioQueue, CutsceneAudioRequest};
use darnell_intro::DarnellIntroState;
use eggnog_erect::EggnogErectOutroState;
use philly_doppel::PhillyDoppelState;
use sserafim_intro::SserafimIntroState;
use stress_pico::StressPicoOutroState;

#[derive(Debug)]
enum ActiveCutsceneKind {
    Video {
        vid: Box<VideoCutsceneState>,
        chain_darnell_ingame: bool,
    },
    Dialogue(Box<DialogueState>),
    Week5Horror {
        elapsed_s: f32,
        lights_turned_on: bool,
    },
    Week6SenpaiIntro {
        elapsed_s: f32,
        dialogue: Box<DialogueState>,
    },
    Week6ThornsIntro {
        elapsed_s: f32,
        fade_step_timer_s: f32,
        senpai_alpha: f32,
        started_anim: bool,
        senpai_bank: Option<AnimBank>,
        dialogue: Box<DialogueState>,
    },
    EggnogErectOutro(Box<EggnogErectOutroState>),
    PhillyDoppelganger(Box<PhillyDoppelState>),
    StressPicoOutro(Box<StressPicoOutroState>),
    DarnellIntro(Box<DarnellIntroState>),
    SserafimIntro(Box<SserafimIntroState>),
}

/// Per-frame output from [`CutsceneController::update_with_scene`].
#[derive(Debug, Clone, Copy, Default)]
pub struct CutsceneStepResult {
    pub finished: bool,
    pub audio: CutsceneAudioQueue,
    pub mute_opponent_vocals: bool,
}

/// Top-level cutscene runner for Story Mode, Pico Mix, and Erect song entries/exits.
#[derive(Debug, Default)]
pub struct CutsceneController {
    kind: Option<ActiveCutsceneKind>,
    white_tex: Option<TextureHandle>,
    paused: bool,
}

impl CutsceneController {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.kind.is_some()
    }

    #[must_use]
    pub fn hides_hud(&self) -> bool {
        self.kind.is_some()
    }

    /// Returns which [`PauseMenuMode`] should open when Escape/P is pressed during a cutscene.
    #[must_use]
    pub fn pause_mode(&self) -> Option<PauseMenuMode> {
        match &self.kind {
            Some(ActiveCutsceneKind::Video { .. }) => Some(PauseMenuMode::Cutscene),
            Some(
                ActiveCutsceneKind::Dialogue(_)
                | ActiveCutsceneKind::Week6SenpaiIntro { .. }
                | ActiveCutsceneKind::Week6ThornsIntro { .. },
            ) => Some(PauseMenuMode::Conversation),
            _ => None,
        }
    }

    /// Pauses or resumes active video/dialogue playback without discarding progress.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if let Some(ActiveCutsceneKind::Video { vid, .. }) = &mut self.kind {
            vid.set_paused(paused);
        }
    }

    /// Restarts active paused video or dialogue from the beginning.
    pub fn restart_active(&mut self, audio: &mut CutsceneAudioQueue) {
        self.paused = false;
        match &mut self.kind {
            Some(ActiveCutsceneKind::Video { vid, .. }) => vid.restart(),
            Some(ActiveCutsceneKind::Dialogue(dlg)) => dlg.restart(),
            Some(ActiveCutsceneKind::Week6SenpaiIntro { elapsed_s, dialogue }) => {
                *elapsed_s = 0.0;
                dialogue.restart();
                audio.push(CutsceneAudioRequest::PlayMusic {
                    key: "Lunchbox",
                    looped: true,
                });
            }
            Some(ActiveCutsceneKind::Week6ThornsIntro { dialogue, .. }) => {
                dialogue.restart();
            }
            _ => {}
        }
    }

    /// Immediately finishes/skips active paused video or dialogue cleanly.
    pub fn skip_active(&mut self, audio: &mut CutsceneAudioQueue) -> bool {
        self.paused = false;
        match &mut self.kind {
            Some(ActiveCutsceneKind::Video { vid, .. }) => {
                vid.finish();
                self.kind = None;
                true
            }
            Some(
                ActiveCutsceneKind::Dialogue(_)
                | ActiveCutsceneKind::Week6SenpaiIntro { .. }
                | ActiveCutsceneKind::Week6ThornsIntro { .. },
            ) => {
                audio.push(CutsceneAudioRequest::StopMusic);
                self.kind = None;
                true
            }
            _ => false,
        }
    }

    /// Starts any matching pre-song cutscene (Story Mode, Pico Mix, or Stage intro).
    #[allow(clippy::too_many_arguments)]
    pub fn start_pre_song(
        &mut self,
        song_id: &str,
        variation: Option<&str>,
        stage_id: &str,
        bf_id: &str,
        is_story_mode: bool,
        naughtyness: bool,
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        mut audio: Option<&mut AudioEngine>,
        volume: f32,
        scene: &mut PlayScene,
    ) -> CutsceneAudioQueue {
        self.white_tex = Some(gpu.white_texture());
        self.kind = None;
        self.paused = false;
        let mut queue = CutsceneAudioQueue::default();

        let song = song_id.to_ascii_lowercase();
        let stage = stage_id.to_ascii_lowercase();
        let bf = bf_id.to_ascii_lowercase();
        let is_pico = variation.is_some_and(|v| v.eq_ignore_ascii_case("pico"));

        if song == "senpai" && (is_story_mode || is_pico) {
            let conv_id = if is_pico { "senpai-pico" } else { "senpai" };
            if let Some(dlg) = DialogueState::load_by_id(conv_id, assets, gpu) {
                self.kind = Some(ActiveCutsceneKind::Week6SenpaiIntro {
                    elapsed_s: 0.0,
                    dialogue: Box::new(dlg),
                });
                queue.push(CutsceneAudioRequest::PlayMusic {
                    key: "Lunchbox",
                    looped: true,
                });
                return queue;
            }
        }

        if song == "roses" && (is_story_mode || is_pico) {
            let conv_id = match (is_pico, naughtyness) {
                (true, true) => "roses-pico",
                (true, false) => "roses-pico-censored",
                (false, true) => "roses",
                (false, false) => "roses-censored",
            };
            if let Some(dlg) = DialogueState::load_by_id(conv_id, assets, gpu) {
                self.kind = Some(ActiveCutsceneKind::Dialogue(Box::new(dlg)));
                queue.push(CutsceneAudioRequest::PlaySound {
                    key: "ANGRY_TEXT_BOX",
                });
                return queue;
            }
        }

        if song == "thorns" && is_story_mode {
            if let Some(dlg) = DialogueState::load_by_id("thorns", assets, gpu) {
                let mut bank = AnimBank::new();
                if let Some(src) = load_sparrow_source(assets, gpu, "weeb/senpaiCrazy") {
                    bank.atlases.push(src);
                    let anim_data = fnfrs_core::schema::character::AnimationData {
                        name: "idle".to_owned(),
                        prefix: Some("Senpai Pre Explosion instance 1".to_owned()),
                        asset_path: None,
                        frame_indices: Vec::new(),
                        offsets: [0.0, 0.0],
                        looped: false,
                        flip_x: false,
                        flip_y: false,
                        frame_rate: 24,
                        anim_type: String::new(),
                        render_type: String::new(),
                        atlas_settings: None,
                    };
                    if let Some(loaded) = build_loaded_anim(&anim_data, &bank.atlases, 0) {
                        bank.anims.push(loaded);
                    }
                    bank.finalize_links();
                }
                self.kind = Some(ActiveCutsceneKind::Week6ThornsIntro {
                    elapsed_s: 0.0,
                    fade_step_timer_s: 0.0,
                    senpai_alpha: 0.0,
                    started_anim: false,
                    senpai_bank: if bank.anims.is_empty() { None } else { Some(bank) },
                    dialogue: Box::new(dlg),
                });
                queue.push(CutsceneAudioRequest::StopMusic);
                return queue;
            }
        }

        if song == "winter-horrorland" && is_story_mode {
            self.kind = Some(ActiveCutsceneKind::Week5Horror {
                elapsed_s: 0.0,
                lights_turned_on: false,
            });
            return queue;
        }

        if stage == "phillytrainerect" && bf == "pico-playable" {
            let state = PhillyDoppelState::start(assets, gpu, scene, &mut queue);
            self.kind = Some(ActiveCutsceneKind::PhillyDoppelganger(Box::new(state)));
            return queue;
        }

        if stage == "sserafim" {
            let state = SserafimIntroState::start(assets, gpu, scene);
            self.kind = Some(ActiveCutsceneKind::SserafimIntro(Box::new(state)));
            return queue;
        }

        if song == "stress" && is_pico {
            let key = if naughtyness {
                "stressPicoCutscene"
            } else {
                "stressPicoCutscene-censored"
            };
            if let Some(vid) = VideoCutsceneState::open(key, assets, gpu, audio.as_deref_mut(), volume)
            {
                self.kind = Some(ActiveCutsceneKind::Video {
                    vid: Box::new(vid),
                    chain_darnell_ingame: false,
                });
                queue.push(CutsceneAudioRequest::StopMusic);
                return queue;
            }
        }

        if is_story_mode {
            let video_key: Option<&str> = match song.as_str() {
                "ugh" => Some("ughCutscene"),
                "guns" => Some("gunsCutscene"),
                "stress" => Some(if naughtyness {
                    "stressCutscene"
                } else {
                    "stressCutscene-censored"
                }),
                "darnell" => Some(if naughtyness {
                    "darnellCutscene"
                } else {
                    "darnellCutscene-censored"
                }),
                _ => None,
            };
            if let Some(key) = video_key {
                let fallback_key = (song == "darnell").then_some("darnellCutscene");
                let vid = match VideoCutsceneState::open(key, assets, gpu, audio.as_deref_mut(), volume)
                {
                    Some(v) => Some(v),
                    None => fallback_key
                        .and_then(|k| VideoCutsceneState::open(k, assets, gpu, audio, volume)),
                };
                if let Some(vid) = vid {
                    self.kind = Some(ActiveCutsceneKind::Video {
                        vid: Box::new(vid),
                        chain_darnell_ingame: song == "darnell",
                    });
                    queue.push(CutsceneAudioRequest::StopMusic);
                    return queue;
                }
            }
            if song == "darnell" {
                let state = DarnellIntroState::start(scene);
                self.kind = Some(ActiveCutsceneKind::DarnellIntro(Box::new(state)));
                return queue;
            }
        }

        queue
    }

    /// Starts any matching post-song cutscene (`eggnog-erect`, `stress-pico`, Story videos).
    #[allow(clippy::too_many_arguments)]
    pub fn start_post_song(
        &mut self,
        song_id: &str,
        variation: Option<&str>,
        is_story_mode: bool,
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        audio: Option<&mut AudioEngine>,
        volume: f32,
        scene: &mut PlayScene,
    ) -> CutsceneAudioQueue {
        self.white_tex = Some(gpu.white_texture());
        self.kind = None;
        self.paused = false;
        let mut queue = CutsceneAudioQueue::default();

        let song = song_id.to_ascii_lowercase();
        let is_erect = variation.is_some_and(|v| v.eq_ignore_ascii_case("erect"));
        let is_pico = variation.is_some_and(|v| v.eq_ignore_ascii_case("pico"));

        if song == "eggnog" && is_erect {
            let state = EggnogErectOutroState::start(assets, gpu, scene);
            self.kind = Some(ActiveCutsceneKind::EggnogErectOutro(Box::new(state)));
            queue.push(CutsceneAudioRequest::PlaySound {
                key: "santa_emotion",
            });
            return queue;
        }

        if song == "stress" && is_pico {
            let state = StressPicoOutroState::start(assets, scene, &mut queue);
            self.kind = Some(ActiveCutsceneKind::StressPicoOutro(Box::new(state)));
            return queue;
        }

        if is_story_mode {
            let video_key = match song.as_str() {
                "2hot" => Some("2hotCutscene"),
                "blazin" => Some("blazinCutscene"),
                _ => None,
            };
            if let Some(key) = video_key {
                if let Some(vid) = VideoCutsceneState::open(key, assets, gpu, audio, volume) {
                    self.kind = Some(ActiveCutsceneKind::Video {
                        vid: Box::new(vid),
                        chain_darnell_ingame: false,
                    });
                    queue.push(CutsceneAudioRequest::StopMusic);
                    return queue;
                }
            }
        }

        queue
    }

    /// Advances the active cutscene alongside `PlayScene` (stage props, characters, camera).
    pub fn update_with_scene(
        &mut self,
        elapsed_s: f32,
        gpu: &GpuContext,
        volume: f32,
        advance_pressed: bool,
        scene: &mut PlayScene,
    ) -> CutsceneStepResult {
        let mut res = CutsceneStepResult::default();
        if self.paused {
            return res;
        }
        let Some(kind) = self.kind.as_mut() else {
            return res;
        };
        let dt = elapsed_s.clamp(0.0, 0.1);
        let mut done = false;
        let mut start_darnell_ingame = false;

        match kind {
            ActiveCutsceneKind::Video {
                vid,
                chain_darnell_ingame,
            } => {
                if vid.update(dt, gpu, volume, advance_pressed, false)
                    == VideoCutsceneAction::Finished
                {
                    if *chain_darnell_ingame {
                        start_darnell_ingame = true;
                    } else {
                        done = true;
                    }
                }
            }
            ActiveCutsceneKind::Dialogue(dlg) => {
                match dlg.update(dt, advance_pressed, false) {
                    DialogueAction::Finished => {
                        done = true;
                        res.audio.push(CutsceneAudioRequest::StopMusic);
                    }
                    DialogueAction::PlayTypeTick => {
                        res.audio
                            .push(CutsceneAudioRequest::PlaySound { key: "pixelText" });
                    }
                    DialogueAction::PlayClick => {
                        res.audio
                            .push(CutsceneAudioRequest::PlaySound { key: "clickText" });
                    }
                    DialogueAction::None => {}
                }
            }
            ActiveCutsceneKind::Week5Horror {
                elapsed_s,
                lights_turned_on,
            } => {
                *elapsed_s += dt;
                if !*lights_turned_on && *elapsed_s >= 0.1 {
                    *lights_turned_on = true;
                    scene.camera_rig.cam_game.snap_to(400.0, -2050.0);
                    scene.camera_rig.cam_game.zoom = 2.5;
                    res.audio.push(CutsceneAudioRequest::PlaySound {
                        key: "Lights_Turn_On",
                    });
                }
                scene.camera_rig.update(dt);
                if *elapsed_s >= 2.1 {
                    done = true;
                }
            }
            ActiveCutsceneKind::Week6SenpaiIntro { elapsed_s, dialogue } => {
                *elapsed_s += dt;
                if *elapsed_s >= 2.0 {
                    match dialogue.update(dt, advance_pressed, false) {
                        DialogueAction::Finished => {
                            done = true;
                            res.audio.push(CutsceneAudioRequest::StopMusic);
                        }
                        DialogueAction::PlayTypeTick => {
                            res.audio
                                .push(CutsceneAudioRequest::PlaySound { key: "pixelText" });
                        }
                        DialogueAction::PlayClick => {
                            res.audio
                                .push(CutsceneAudioRequest::PlaySound { key: "clickText" });
                        }
                        DialogueAction::None => {}
                    }
                }
            }
            ActiveCutsceneKind::Week6ThornsIntro {
                elapsed_s,
                fade_step_timer_s,
                senpai_alpha,
                started_anim,
                senpai_bank,
                dialogue,
            } => {
                *elapsed_s += dt;
                if *senpai_alpha < 1.0 {
                    *fade_step_timer_s += dt;
                    if *fade_step_timer_s >= 0.3 {
                        *fade_step_timer_s -= 0.3;
                        *senpai_alpha = (*senpai_alpha + 0.15).min(1.0);
                        if *senpai_alpha >= 1.0 && !*started_anim {
                            *started_anim = true;
                            res.audio.push(CutsceneAudioRequest::PlaySound {
                                key: "Senpai_Dies",
                            });
                            if let Some(bank) = senpai_bank {
                                if let Some(&idx) = bank.anim_index.get("idle") {
                                    bank.play_index(idx, true);
                                }
                            }
                        }
                    }
                } else {
                    if let Some(bank) = senpai_bank {
                        bank.update(dt);
                    }
                    if *elapsed_s >= 4.2 {
                        match dialogue.update(dt, advance_pressed, false) {
                            DialogueAction::Finished => {
                                done = true;
                                res.audio.push(CutsceneAudioRequest::StopMusic);
                            }
                            DialogueAction::PlayTypeTick => {
                                res.audio
                                    .push(CutsceneAudioRequest::PlaySound { key: "pixelText" });
                            }
                            DialogueAction::PlayClick => {
                                res.audio
                                    .push(CutsceneAudioRequest::PlaySound { key: "clickText" });
                            }
                            DialogueAction::None => {}
                        }
                    }
                }
            }
            ActiveCutsceneKind::EggnogErectOutro(st) => {
                done = st.update(dt, advance_pressed, scene, &mut res.audio);
            }
            ActiveCutsceneKind::PhillyDoppelganger(st) => {
                done = st.update(dt, advance_pressed, scene, &mut res.audio);
                if done && st.explode {
                    res.mute_opponent_vocals = true;
                }
            }
            ActiveCutsceneKind::StressPicoOutro(st) => {
                done = st.update(dt, advance_pressed, scene, &mut res.audio);
            }
            ActiveCutsceneKind::DarnellIntro(st) => {
                done = st.update(dt, advance_pressed, scene, &mut res.audio);
            }
            ActiveCutsceneKind::SserafimIntro(st) => {
                done = st.update(dt, advance_pressed, scene, &mut res.audio);
            }
        }

        if start_darnell_ingame {
            let state = DarnellIntroState::start(scene);
            self.kind = Some(ActiveCutsceneKind::DarnellIntro(Box::new(state)));
        } else if done {
            self.kind = None;
            res.finished = true;
        }
        res
    }

    /// Draws any world-space scripted cutscene sprites into `stage_sprites`.
    pub fn draw_world(&self, batcher: &mut SpriteBatcher, cam_pos: [f32; 2], cam_zoom: f32) {
        let Some(kind) = &self.kind else {
            return;
        };
        match kind {
            ActiveCutsceneKind::EggnogErectOutro(st) => st.draw_world(batcher, cam_pos, cam_zoom),
            ActiveCutsceneKind::PhillyDoppelganger(st) => st.draw_world(batcher, cam_pos, cam_zoom),
            ActiveCutsceneKind::SserafimIntro(st) => st.draw_world(batcher, cam_pos, cam_zoom),
            _ => {}
        }
    }

    /// Draws cutscene overlays (videos, dialogue, fades, subtitles, skip prompt).
    pub fn draw(
        &mut self,
        batcher: &mut SpriteBatcher,
        font: &BitmapFont,
        font_tex: TextureHandle,
    ) {
        let Some(kind) = self.kind.as_mut() else {
            return;
        };
        match kind {
            ActiveCutsceneKind::Video { vid, .. } => {
                if let Some(white_tex) = self.white_tex {
                    vid.draw(batcher, white_tex, font, font_tex);
                }
            }
            ActiveCutsceneKind::Dialogue(dlg) => {
                dlg.draw(batcher, font, font_tex);
            }
            ActiveCutsceneKind::Week5Horror { elapsed_s, .. } => {
                let alpha = if *elapsed_s < 0.1 {
                    1.0
                } else {
                    (1.0 - (*elapsed_s - 0.1) / 1.8).clamp(0.0, 1.0)
                };
                if alpha > 0.01 {
                    if let Some(white_tex) = self.white_tex {
                        batcher.push(
                            white_tex,
                            BlendMode::Normal,
                            FilterMode::Nearest,
                            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                                .with_color([0.0, 0.0, 0.0, alpha]),
                        );
                    }
                }
            }
            ActiveCutsceneKind::Week6SenpaiIntro { elapsed_s, dialogue } => {
                if *elapsed_s < 2.0 {
                    let t = ((*elapsed_s - 0.25) / 1.75).clamp(0.0, 1.0);
                    let snapped = (t * 8.0).floor() / 8.0;
                    let a = 1.0 - snapped;
                    if let Some(white_tex) = self.white_tex {
                        batcher.push(
                            white_tex,
                            BlendMode::Normal,
                            FilterMode::Nearest,
                            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                                .with_color([0.0, 0.0, 0.0, a]),
                        );
                    }
                } else {
                    dialogue.draw(batcher, font, font_tex);
                }
            }
            ActiveCutsceneKind::Week6ThornsIntro {
                elapsed_s,
                senpai_alpha,
                senpai_bank,
                dialogue,
                ..
            } => {
                if *elapsed_s < 4.2 {
                    if let Some(white_tex) = self.white_tex {
                        batcher.push(
                            white_tex,
                            BlendMode::Normal,
                            FilterMode::Nearest,
                            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                                .with_color([1.0, 0.106, 0.192, 1.0]),
                        );
                    }
                    if let Some(bank) = senpai_bank {
                        if let Some(crate::character::LoadedAtlasSource::Sparrow {
                            texture,
                            tex_size,
                            ..
                        }) = bank.atlases.first()
                        {
                            if let Some(crate::character::AnimFrames::Sheet { frames, .. }) =
                                bank.current_anim.and_then(|i| bank.anims.get(i)).map(|a| &a.frames)
                            {
                                if let Some(frame) = frames.get(bank.current_frame) {
                                    let params = AtlasDrawParams {
                                        pos: [430.0, 140.0],
                                        scale: [6.0, 6.0],
                                        anim_offsets: [0.0, 0.0],
                                        global_offsets: [0.0, 0.0],
                                        scroll_factor: [0.0, 0.0],
                                        cam_pos: [0.0, 0.0],
                                        cam_zoom: 1.0,
                                        angle_deg: 0.0,
                                        flip_x: false,
                                        flip_y: false,
                                        color: [1.0, 1.0, 1.0, 1.0],
                                        alpha: *senpai_alpha,
                                        blend: BlendMode::Normal,
                                        is_pixel: true,
                                        adjust_color: None,
                                        wiggle: None,
                                        ref_unscaled_size: None,
                                    };
                                    draw_atlas_frame(
                                        batcher, *texture, *tex_size, frame, &params, false, false,
                                    );
                                }
                            }
                        }
                    }
                } else {
                    dialogue.draw(batcher, font, font_tex);
                }
            }
            ActiveCutsceneKind::EggnogErectOutro(st) => {
                st.draw_overlay(batcher, self.white_tex, font, font_tex);
            }
            ActiveCutsceneKind::PhillyDoppelganger(st) => {
                st.draw_overlay(batcher, self.white_tex, font, font_tex);
            }
            ActiveCutsceneKind::StressPicoOutro(st) => {
                st.draw_overlay(batcher, self.white_tex, font, font_tex);
            }
            ActiveCutsceneKind::DarnellIntro(st) => {
                st.draw_overlay(batcher, self.white_tex, font, font_tex);
            }
            ActiveCutsceneKind::SserafimIntro(st) => {
                st.draw_overlay(batcher, self.white_tex, font, font_tex);
            }
        }
    }
}
