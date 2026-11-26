//! Pre-song & post-song V-Slice cutscene coordinator (video, Week 6 dialogue, in-game).

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{
    BitmapFont, BlendMode, FilterMode, GpuContext, SpriteBatcher, SpriteInstance, TextureHandle,
};

use crate::character::{build_loaded_anim, draw_atlas_frame, load_sparrow_source, AnimBank, AtlasDrawParams};
use crate::dialogue::{DialogueAction, DialogueState};
use crate::video::{VideoCutsceneAction, VideoCutsceneState};

/// External audio cue requested by an active cutscene step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutsceneAudioRequest {
    None,
    PlayMusic { key: &'static str, looped: bool },
    PlaySound { key: &'static str },
    StopMusic,
}

#[derive(Debug)]
enum ActiveCutsceneKind {
    Video(Box<VideoCutsceneState>),
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
    EggnogErectOutro {
        elapsed_s: f32,
        santa_shot_played: bool,
    },
}

/// Top-level cutscene runner for Story Mode and Pico Mix song entries/exits.
#[derive(Debug, Default)]
pub struct CutsceneController {
    kind: Option<ActiveCutsceneKind>,
    white_tex: Option<TextureHandle>,
}

impl CutsceneController {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.kind.is_some()
    }

    #[must_use]
    pub fn hides_hud(&self) -> bool {
        matches!(
            self.kind,
            Some(
                ActiveCutsceneKind::Video(_)
                    | ActiveCutsceneKind::Week5Horror { .. }
                    | ActiveCutsceneKind::Week6SenpaiIntro { .. }
                    | ActiveCutsceneKind::Week6ThornsIntro { .. }
                    | ActiveCutsceneKind::EggnogErectOutro { .. }
            )
        )
    }

    /// Starts any matching pre-song cutscene (Story Mode or Pico mix) cleanly.
    pub fn start_pre_song(
        &mut self,
        song_id: &str,
        variation: Option<&str>,
        is_story_mode: bool,
        naughtyness: bool,
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        volume: f32,
    ) -> CutsceneAudioRequest {
        self.white_tex = Some(gpu.white_texture());
        self.kind = None;
        let song = song_id.to_ascii_lowercase();
        let is_pico = variation.is_some_and(|v| v.eq_ignore_ascii_case("pico"));

        if song == "senpai" && (is_story_mode || is_pico) {
            let conv_id = if is_pico { "senpai-pico" } else { "senpai" };
            if let Some(dlg) = DialogueState::load_by_id(conv_id, assets, gpu) {
                self.kind = Some(ActiveCutsceneKind::Week6SenpaiIntro {
                    elapsed_s: 0.0,
                    dialogue: Box::new(dlg),
                });
                return CutsceneAudioRequest::PlayMusic {
                    key: "Lunchbox",
                    looped: true,
                };
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
                return CutsceneAudioRequest::PlaySound {
                    key: "ANGRY_TEXT_BOX",
                };
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
                return CutsceneAudioRequest::StopMusic;
            }
        }

        if song == "winter-horrorland" && is_story_mode {
            self.kind = Some(ActiveCutsceneKind::Week5Horror {
                elapsed_s: 0.0,
                lights_turned_on: false,
            });
            return CutsceneAudioRequest::None;
        }

        if is_story_mode {
            let video_key: Option<&str> = match song.as_str() {
                "ugh" => Some("ughCutscene"),
                "guns" => Some("gunsCutscene"),
                "stress" => Some(match (is_pico, naughtyness) {
                    (true, true) => "stressPicoCutscene",
                    (true, false) => "stressPicoCutscene-censored",
                    (false, true) => "stressCutscene",
                    (false, false) => "stressCutscene-censored",
                }),
                "darnell" => Some("darnellCutscene"),
                _ => None,
            };
            if let Some(key) = video_key {
                if let Some(vid) = VideoCutsceneState::open(key, assets, gpu, volume) {
                    self.kind = Some(ActiveCutsceneKind::Video(Box::new(vid)));
                    return CutsceneAudioRequest::StopMusic;
                }
            }
        }

        CutsceneAudioRequest::None
    }

    /// Starts any matching post-song cutscene (Story Mode video or `eggnog-erect` Santa death).
    pub fn start_post_song(
        &mut self,
        song_id: &str,
        variation: Option<&str>,
        is_story_mode: bool,
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        volume: f32,
    ) -> CutsceneAudioRequest {
        self.white_tex = Some(gpu.white_texture());
        self.kind = None;
        let song = song_id.to_ascii_lowercase();
        let is_erect = variation.is_some_and(|v| v.eq_ignore_ascii_case("erect"));

        if song == "eggnog" && is_erect {
            self.kind = Some(ActiveCutsceneKind::EggnogErectOutro {
                elapsed_s: 0.0,
                santa_shot_played: false,
            });
            return CutsceneAudioRequest::PlaySound {
                key: "santa_emotion",
            };
        }

        if is_story_mode {
            let video_key = match song.as_str() {
                "2hot" => Some("2hotCutscene"),
                "blazin" => Some("blazinCutscene"),
                _ => None,
            };
            if let Some(key) = video_key {
                if let Some(vid) = VideoCutsceneState::open(key, assets, gpu, volume) {
                    self.kind = Some(ActiveCutsceneKind::Video(Box::new(vid)));
                    return CutsceneAudioRequest::StopMusic;
                }
            }
        }

        CutsceneAudioRequest::None
    }

    /// Advances active cutscene timer/dialogue/video. Returns `(finished_now, audio_request)`.
    pub fn update(
        &mut self,
        elapsed_s: f32,
        gpu: &GpuContext,
        volume: f32,
        accept_pressed: bool,
        back_pressed: bool,
    ) -> (bool, CutsceneAudioRequest) {
        let Some(kind) = self.kind.as_mut() else {
            return (false, CutsceneAudioRequest::None);
        };
        let dt = elapsed_s.clamp(0.0, 0.1);
        let mut req = CutsceneAudioRequest::None;
        let mut done = false;

        match kind {
            ActiveCutsceneKind::Video(vid) => {
                if vid.update(dt, gpu, volume, accept_pressed, back_pressed)
                    == VideoCutsceneAction::Finished
                {
                    done = true;
                }
            }
            ActiveCutsceneKind::Dialogue(dlg) => {
                match dlg.update(dt, accept_pressed, back_pressed) {
                    DialogueAction::Finished => {
                        done = true;
                        req = CutsceneAudioRequest::StopMusic;
                    }
                    DialogueAction::PlayTypeTick => {
                        req = CutsceneAudioRequest::PlaySound { key: "pixelText" };
                    }
                    DialogueAction::PlayClick => {
                        req = CutsceneAudioRequest::PlaySound { key: "clickText" };
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
                    req = CutsceneAudioRequest::PlaySound {
                        key: "Lights_Turn_On",
                    };
                }
                if *elapsed_s >= 2.1 || back_pressed {
                    done = true;
                }
            }
            ActiveCutsceneKind::Week6SenpaiIntro { elapsed_s, dialogue } => {
                *elapsed_s += dt;
                if *elapsed_s >= 2.0 {
                    match dialogue.update(dt, accept_pressed, back_pressed) {
                        DialogueAction::Finished => {
                            done = true;
                            req = CutsceneAudioRequest::StopMusic;
                        }
                        DialogueAction::PlayTypeTick => {
                            req = CutsceneAudioRequest::PlaySound { key: "pixelText" };
                        }
                        DialogueAction::PlayClick => {
                            req = CutsceneAudioRequest::PlaySound { key: "clickText" };
                        }
                        DialogueAction::None => {}
                    }
                } else if back_pressed {
                    done = true;
                    req = CutsceneAudioRequest::StopMusic;
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
                            req = CutsceneAudioRequest::PlaySound { key: "Senpai_Dies" };
                            if let Some(bank) = senpai_bank {
                                if let Some(&idx) = bank.anim_index.get("idle") {
                                    bank.play_index(idx, true);
                                }
                            }
                        }
                    }
                } else if !dialogue.music_asset.is_some() || *elapsed_s >= 4.2 {
                    if let Some(bank) = senpai_bank {
                        bank.update(dt);
                    }
                    if *elapsed_s >= 4.2 {
                        match dialogue.update(dt, accept_pressed, back_pressed) {
                            DialogueAction::Finished => {
                                done = true;
                                req = CutsceneAudioRequest::StopMusic;
                            }
                            DialogueAction::PlayTypeTick => {
                                req = CutsceneAudioRequest::PlaySound { key: "pixelText" };
                            }
                            DialogueAction::PlayClick => {
                                req = CutsceneAudioRequest::PlaySound { key: "clickText" };
                            }
                            DialogueAction::None => {}
                        }
                    } else if back_pressed {
                        done = true;
                        req = CutsceneAudioRequest::StopMusic;
                    }
                } else if let Some(bank) = senpai_bank {
                    bank.update(dt);
                }
            }
            ActiveCutsceneKind::EggnogErectOutro {
                elapsed_s,
                santa_shot_played,
            } => {
                *elapsed_s += dt;
                if !*santa_shot_played && *elapsed_s >= 11.375 {
                    *santa_shot_played = true;
                    req = CutsceneAudioRequest::PlaySound {
                        key: "santa_shot_n_falls",
                    };
                }
                if *elapsed_s >= 15.5 || (accept_pressed && *elapsed_s > 0.6) || back_pressed {
                    done = true;
                }
            }
        }

        if done {
            self.kind = None;
        }
        (done, req)
    }

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
            ActiveCutsceneKind::Video(vid) => {
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
            ActiveCutsceneKind::EggnogErectOutro { elapsed_s, .. } => {
                if *elapsed_s >= 14.5 {
                    let fade = ((*elapsed_s - 14.5) / 1.0).clamp(0.0, 1.0);
                    if let Some(white_tex) = self.white_tex {
                        batcher.push(
                            white_tex,
                            BlendMode::Normal,
                            FilterMode::Nearest,
                            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                                .with_color([0.0, 0.0, 0.0, fade]),
                        );
                    }
                }
            }
        }
    }
}
