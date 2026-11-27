//! Weekend 1 `darnell` in-game pre-song cutscene (`darnell.hxc:117-240`).

use fnfrs_render::{BitmapFont, BlendMode, FilterMode, SpriteBatcher, SpriteInstance, TextureHandle};

use crate::camera::FlxEase;
use crate::play_state::PlayScene;

use super::common::{CutsceneAudioQueue, CutsceneAudioRequest, SkipPromptState};

#[derive(Debug)]
pub struct DarnellIntroState {
    dad_focus: [f32; 2],
    bf_focus: [f32; 2],
    skip: SkipPromptState,
    elapsed_s: f32,
    beat_accum_s: f32,
    music_started: bool,
    lighter_done: bool,
    gun_prep_done: bool,
    kick_up_done: bool,
    knee_fwd_done: bool,
    shoot_done: bool,
    darnell_laugh_done: bool,
    nene_laugh_done: bool,
    end_pan_done: bool,
}

impl DarnellIntroState {
    #[must_use]
    pub fn start(scene: &mut PlayScene) -> Self {
        let dad_focus = scene
            .dad
            .as_ref()
            .map_or([500.0, 500.0], |d| d.camera_focus_point());
        let bf_focus = scene
            .bf
            .as_ref()
            .map_or([1200.0, 550.0], |b| b.camera_focus_point());

        scene
            .camera_rig
            .cam_game
            .snap_to(bf_focus[0] + 150.0, bf_focus[1]);
        scene.camera_rig.cam_game.tween_zoom(1.3, 0.01, FlxEase::Linear);

        Self {
            dad_focus,
            bf_focus,
            skip: SkipPromptState::new(),
            elapsed_s: 0.0,
            beat_accum_s: 0.0,
            music_started: false,
            lighter_done: false,
            gun_prep_done: false,
            kick_up_done: false,
            knee_fwd_done: false,
            shoot_done: false,
            darnell_laugh_done: false,
            nene_laugh_done: false,
            end_pan_done: false,
        }
    }

    pub fn update(
        &mut self,
        dt: f32,
        advance_pressed: bool,
        scene: &mut PlayScene,
        audio: &mut CutsceneAudioQueue,
    ) -> bool {
        if self.skip.update(dt, advance_pressed, !self.skip.skipped) {
            self.finish(scene, audio);
            return true;
        }
        if self.skip.skipped {
            audio.push(CutsceneAudioRequest::StopAllSfx);
            audio.push(CutsceneAudioRequest::StopMusic);
            scene.camera_rig.update(dt);
            return false;
        }

        self.elapsed_s += dt;
        let beat_ms = 60_000.0 / 168.0;
        if self.music_started {
            self.beat_accum_s += dt;
            if self.beat_accum_s >= beat_ms as f32 / 1000.0 {
                self.beat_accum_s -= beat_ms as f32 / 1000.0;
                if let Some(gf) = &mut scene.gf {
                    gf.dance(true);
                }
            }
        }
        if let Some(bf) = &mut scene.bf {
            bf.update(dt, beat_ms, false);
        }
        if let Some(dad) = &mut scene.dad {
            dad.update(dt, beat_ms, false);
        }
        if let Some(gf) = &mut scene.gf {
            gf.update(dt, beat_ms, false);
        }
        scene.camera_rig.update(dt);

        let d = 2.0_f32;
        if !self.music_started && self.elapsed_s >= d {
            self.music_started = true;
            audio.push(CutsceneAudioRequest::PlayMusic {
                key: "darnellCanCutscene",
                looped: false,
            });
            scene.camera_rig.cam_game.tween_zoom(0.77, 1.8, FlxEase::QuadInOut);
        }
        if !self.lighter_done && self.elapsed_s >= d + 0.7 {
            self.lighter_done = true;
            if let Some(dad) = &mut scene.dad {
                dad.play_animation("lightCan", true, true);
            }
            audio.push(CutsceneAudioRequest::PlaySound {
                key: "Darnell_Lighter",
            });
            scene.camera_rig.cam_game.tween_to(
                self.dad_focus[0] + 180.0,
                self.dad_focus[1],
                1.0,
                FlxEase::QuadOut,
            );
        }
        if !self.gun_prep_done && self.elapsed_s >= d + 2.0 {
            self.gun_prep_done = true;
            if let Some(bf) = &mut scene.bf {
                bf.play_animation("intro1", true, true);
            }
            scene.camera_rig.cam_game.tween_to(
                self.bf_focus[0] + 180.0,
                self.bf_focus[1],
                0.4,
                FlxEase::BackOut,
            );
            audio.push(CutsceneAudioRequest::PlaySound { key: "Gun_Prep" });
        }
        if !self.kick_up_done && self.elapsed_s >= d + 4.4 {
            self.kick_up_done = true;
            if let Some(dad) = &mut scene.dad {
                dad.play_animation("kickCan", true, true);
            }
            audio.push(CutsceneAudioRequest::PlaySound { key: "Kick_Can_UP" });
        }
        if !self.knee_fwd_done && self.elapsed_s >= d + 4.9 {
            self.knee_fwd_done = true;
            if let Some(dad) = &mut scene.dad {
                dad.play_animation("kneeCan", true, true);
            }
            audio.push(CutsceneAudioRequest::PlaySound {
                key: "Kick_Can_FORWARD",
            });
        }
        if !self.shoot_done && self.elapsed_s >= d + 5.1 {
            self.shoot_done = true;
            if let Some(bf) = &mut scene.bf {
                bf.play_animation("intro2", true, true);
            }
            audio.push(CutsceneAudioRequest::PlaySound { key: "shot1" });
            scene.camera_rig.cam_game.tween_to(
                self.dad_focus[0] + 100.0,
                self.dad_focus[1],
                1.0,
                FlxEase::QuadInOut,
            );
        }
        if !self.darnell_laugh_done && self.elapsed_s >= d + 5.9 {
            self.darnell_laugh_done = true;
            if let Some(dad) = &mut scene.dad {
                dad.play_animation("laughCutscene", true, true);
            }
            audio.push(CutsceneAudioRequest::PlaySound {
                key: "cutscene/darnell_laugh",
            });
        }
        if !self.nene_laugh_done && self.elapsed_s >= d + 6.2 {
            self.nene_laugh_done = true;
            if let Some(gf) = &mut scene.gf {
                gf.play_animation("laughCutscene", true, true);
            }
            audio.push(CutsceneAudioRequest::PlaySound {
                key: "cutscene/nene_laugh",
            });
        }
        if !self.end_pan_done && self.elapsed_s >= d + 8.0 {
            self.end_pan_done = true;
            scene.camera_rig.cam_game.tween_to(
                self.dad_focus[0] + 180.0,
                self.dad_focus[1],
                2.0,
                FlxEase::SineInOut,
            );
            scene.camera_rig.cam_game.tween_zoom(0.77, 2.0, FlxEase::SineInOut);
            self.finish(scene, audio);
            return true;
        }
        false
    }

    fn finish(&self, _scene: &mut PlayScene, audio: &mut CutsceneAudioQueue) {
        audio.push(CutsceneAudioRequest::StopAllSfx);
        audio.push(CutsceneAudioRequest::StopMusic);
    }

    pub fn draw_overlay(
        &self,
        batcher: &mut SpriteBatcher,
        white_tex: Option<TextureHandle>,
        font: &BitmapFont,
        font_tex: TextureHandle,
    ) {
        if self.elapsed_s < 2.0 {
            if let Some(wt) = white_tex {
                let alpha = (1.0 - self.elapsed_s / 2.0).clamp(0.0, 1.0);
                batcher.push(
                    wt,
                    BlendMode::Normal,
                    FilterMode::Nearest,
                    SpriteInstance::new([0.0, 0.0], [1280.0, 720.0])
                        .with_color([0.0, 0.0, 0.0, alpha]),
                );
            }
        }
        self.skip.draw(batcher, white_tex, font, font_tex);
    }
}
