//! Philly Train Erect Pico Doppelganger intro cutscene (`phillyTrainErect.hxc`,
//! `PicoDopplegangerSprite.hxc`, `PicoBloodPool.hxc`).

use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{BitmapFont, BlendMode, FilterMode, GpuContext, SpriteBatcher, SpriteInstance, TextureHandle};

use crate::play_state::PlayScene;

use super::common::{
    CutsceneAtlasProp, CutsceneAudioQueue, CutsceneAudioRequest, SkipPromptState,
};

const PHILLY_SHADER: [f32; 4] = [-26.0, -16.0, 0.0, -5.0];

#[derive(Debug)]
pub struct PhillyDoppelState {
    pico_player: Option<CutsceneAtlasProp>,
    pico_opponent: Option<CutsceneAtlasProp>,
    blood_pool: Option<CutsceneAtlasProp>,
    cigarette: Option<CutsceneAtlasProp>,
    player_shoots: bool,
    pub explode: bool,
    shooter_cam: [f32; 2],
    cig_cam: [f32; 2],
    skip: SkipPromptState,
    elapsed_s: f32,
    beat_accum_s: f32,
    gasp_played: bool,
    pan_cig: bool,
    pan_shooter: bool,
    pan_final: bool,
    victim_sfx_1: bool,
    victim_sfx_2: bool,
    shooter_sfx_1: bool,
    shooter_sfx_2: bool,
    blood_started: bool,
    cig_started: bool,
    explode_loop_started: bool,
}

impl PhillyDoppelState {
    #[must_use]
    pub fn start(
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        scene: &mut PlayScene,
        audio: &mut CutsceneAudioQueue,
    ) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1u32, |d| (d.subsec_nanos() ^ d.as_secs() as u32).max(1));
        let player_shoots = (seed & 1) == 0;
        let explode = (seed % 100) < 8;

        let bf_pos = scene.bf.as_ref().map_or([850.0, 100.0], |c| c.pos);
        let dad_pos = scene.dad.as_ref().map_or([100.0, 100.0], |c| c.pos);
        let bf_cam = scene
            .bf
            .as_ref()
            .map_or([700.0, 350.0], |c| c.camera_focus_point());
        let dad_cam = scene
            .dad
            .as_ref()
            .map_or([350.0, 350.0], |c| c.camera_focus_point());

        if let Some(bf) = &mut scene.bf {
            bf.visible = false;
        }
        if let Some(dad) = &mut scene.dad {
            dad.visible = false;
        }

        let (shooter_cam, cig_cam, blood_pos, cig_pos, cig_flip) = if player_shoots {
            (
                bf_cam,
                dad_cam,
                [dad_pos[0] - 30.0, dad_pos[1] + 460.0],
                [bf_pos[0] - 10.0, bf_pos[1] + 260.0],
                true,
            )
        } else {
            (
                dad_cam,
                bf_cam,
                [bf_pos[0] + 640.0, bf_pos[1] + 460.0],
                [bf_pos[0] - 350.0, bf_pos[1] + 260.0],
                false,
            )
        };

        let p_label = if player_shoots {
            "shootPlayer"
        } else if explode {
            "explodePlayer"
        } else {
            "cigarettePlayer"
        };
        let o_label = if !player_shoots {
            "shootOpponent"
        } else if explode {
            "explodeOpponent"
        } else {
            "cigaretteOpponent"
        };

        let mut pico_player = CutsceneAtlasProp::load_animate(
            assets,
            gpu,
            "philly/erect/pico_doppleganger",
            [bf_pos[0] - 402.0, bf_pos[1] - 144.0],
            Some(p_label),
            false,
        );
        if let Some(p) = &mut pico_player {
            p.adjust_color = Some(PHILLY_SHADER);
        }

        let mut pico_opponent = CutsceneAtlasProp::load_animate(
            assets,
            gpu,
            "philly/erect/pico_doppleganger",
            [dad_pos[0] - 333.0, dad_pos[1] - 144.5],
            Some(o_label),
            false,
        );
        if let Some(o) = &mut pico_opponent {
            o.adjust_color = Some(PHILLY_SHADER);
        }

        let mut blood_pool = CutsceneAtlasProp::load_animate(
            assets,
            gpu,
            "philly/erect/bloodPool",
            blood_pos,
            Some("poolAnim"),
            false,
        );
        if let Some(b) = &mut blood_pool {
            b.visible = false;
            b.adjust_color = Some(PHILLY_SHADER);
        }

        let mut cigarette = CutsceneAtlasProp::load_sparrow(
            assets,
            gpu,
            "philly/erect/cigarette",
            "cigarette spit",
            cig_pos,
            false,
        );
        if let Some(c) = &mut cigarette {
            c.visible = false;
            c.flip_x = cig_flip;
        }

        let mid_x = (shooter_cam[0] + cig_cam[0]) * 0.5;
        let mid_y = (shooter_cam[1] + cig_cam[1]) * 0.5;
        scene.camera_rig.cam_game.reset(true, true, true);
        scene.camera_rig.cam_game.set_target(mid_x, mid_y);

        audio.push(CutsceneAudioRequest::PlayMusic {
            key: if explode {
                "cutscene/cutscene2"
            } else {
                "cutscene/cutscene"
            },
            looped: false,
        });

        Self {
            pico_player,
            pico_opponent,
            blood_pool,
            cigarette,
            player_shoots,
            explode,
            shooter_cam,
            cig_cam,
            skip: SkipPromptState::new(),
            elapsed_s: 0.0,
            beat_accum_s: 0.0,
            gasp_played: false,
            pan_cig: false,
            pan_shooter: false,
            pan_final: false,
            victim_sfx_1: false,
            victim_sfx_2: false,
            shooter_sfx_1: false,
            shooter_sfx_2: false,
            blood_started: false,
            cig_started: false,
            explode_loop_started: false,
        }
    }

    pub fn update(
        &mut self,
        dt: f32,
        advance_pressed: bool,
        scene: &mut PlayScene,
        audio: &mut CutsceneAudioQueue,
    ) -> bool {
        let allow_skip = self.elapsed_s < 8.75 && !self.skip.skipped;
        if self.skip.update(dt, advance_pressed, allow_skip) {
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
        self.beat_accum_s += dt;
        if self.beat_accum_s >= 0.4615 {
            self.beat_accum_s -= 0.4615;
            if let Some(gf) = &mut scene.gf {
                if !gf.current_animation().eq_ignore_ascii_case("drop70") {
                    gf.dance(true);
                }
            }
        }
        if let Some(gf) = &mut scene.gf {
            gf.update(dt, 461.5, false);
        }
        if let Some(p) = &mut self.pico_player {
            p.update(dt);
        }
        if let Some(o) = &mut self.pico_opponent {
            o.update(dt);
        }
        if let Some(b) = &mut self.blood_pool {
            b.update(dt);
            if b.visible && b.finished {
                let ext = 0.02 * dt;
                b.scale[0] += ext;
                b.scale[1] += ext;
            }
        }
        if let Some(c) = &mut self.cigarette {
            c.update(dt);
        }
        scene.camera_rig.update(dt);

        if self.explode && !self.explode_loop_started {
            let victim = if self.player_shoots {
                &mut self.pico_opponent
            } else {
                &mut self.pico_player
            };
            if let Some(v) = victim {
                if v.finished {
                    self.explode_loop_started = true;
                    let loop_lbl = if self.player_shoots {
                        "loopOpponent"
                    } else {
                        "loopPlayer"
                    };
                    v.play_label(loop_lbl, true);
                }
            }
        }

        if !self.gasp_played && self.elapsed_s >= 0.3 {
            self.gasp_played = true;
            audio.push(CutsceneAudioRequest::PlaySound {
                key: "cutscene/picoGasp",
            });
        }
        if !self.victim_sfx_1 && self.elapsed_s >= 3.7 {
            self.victim_sfx_1 = true;
            audio.push(CutsceneAudioRequest::PlaySound {
                key: if self.explode {
                    "cutscene/picoCigarette2"
                } else {
                    "cutscene/picoCigarette"
                },
            });
        }
        if !self.pan_cig && self.elapsed_s >= 4.0 {
            self.pan_cig = true;
            scene
                .camera_rig
                .cam_game
                .set_target(self.cig_cam[0], self.cig_cam[1]);
        }
        if !self.shooter_sfx_1 && self.elapsed_s >= 6.29 {
            self.shooter_sfx_1 = true;
            audio.push(CutsceneAudioRequest::PlaySound {
                key: "cutscene/picoShoot",
            });
        }
        if !self.pan_shooter && self.elapsed_s >= 6.3 {
            self.pan_shooter = true;
            scene
                .camera_rig
                .cam_game
                .set_target(self.shooter_cam[0], self.shooter_cam[1]);
        }
        if !self.pan_final && self.elapsed_s >= 8.75 {
            self.pan_final = true;
            scene
                .camera_rig
                .cam_game
                .set_target(self.cig_cam[0], self.cig_cam[1]);
            if self.explode {
                if let Some(gf) = &mut scene.gf {
                    gf.play_animation("drop70", true, true);
                }
            }
        }
        if self.explode && !self.victim_sfx_2 && self.elapsed_s >= 8.75 {
            self.victim_sfx_2 = true;
            audio.push(CutsceneAudioRequest::PlaySound {
                key: "cutscene/picoExplode",
            });
        }
        if !self.shooter_sfx_2 && self.elapsed_s >= 10.33 {
            self.shooter_sfx_2 = true;
            audio.push(CutsceneAudioRequest::PlaySound {
                key: "cutscene/picoSpin",
            });
        }
        if self.explode && !self.blood_started && self.elapsed_s >= 11.2 {
            self.blood_started = true;
            if let Some(b) = &mut self.blood_pool {
                b.visible = true;
            }
        }
        if !self.explode && !self.cig_started && self.elapsed_s >= 11.5 {
            self.cig_started = true;
            if let Some(c) = &mut self.cigarette {
                c.visible = true;
            }
        }

        if self.elapsed_s >= 13.0 {
            self.finish(scene, audio);
            return true;
        }
        false
    }

    fn finish(&self, scene: &mut PlayScene, audio: &mut CutsceneAudioQueue) {
        audio.push(CutsceneAudioRequest::StopAllSfx);
        audio.push(CutsceneAudioRequest::StopMusic);
        if let Some(bf) = &mut scene.bf {
            bf.visible = true;
        }
        if let Some(dad) = &mut scene.dad {
            dad.visible = true;
        }
    }

    pub fn draw_world(&self, batcher: &mut SpriteBatcher, cam_pos: [f32; 2], cam_zoom: f32) {
        if let Some(c) = &self.cigarette {
            c.draw(batcher, cam_pos, cam_zoom);
        }
        if let Some(b) = &self.blood_pool {
            b.draw(batcher, cam_pos, cam_zoom);
        }
        if self.player_shoots {
            if let Some(o) = &self.pico_opponent {
                o.draw(batcher, cam_pos, cam_zoom);
            }
            if let Some(p) = &self.pico_player {
                p.draw(batcher, cam_pos, cam_zoom);
            }
        } else {
            if let Some(p) = &self.pico_player {
                p.draw(batcher, cam_pos, cam_zoom);
            }
            if let Some(o) = &self.pico_opponent {
                o.draw(batcher, cam_pos, cam_zoom);
            }
        }
    }

    pub fn draw_overlay(
        &self,
        batcher: &mut SpriteBatcher,
        white_tex: Option<TextureHandle>,
        font: &BitmapFont,
        font_tex: TextureHandle,
    ) {
        self.skip.draw(batcher, white_tex, font, font_tex);
        let _ = (BlendMode::Normal, FilterMode::Nearest, SpriteInstance::new([0.0, 0.0], [1.0, 1.0]));
    }
}
