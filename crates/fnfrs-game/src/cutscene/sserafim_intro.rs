//! Sserafim (`spaghetti`) intro & outro cutscenes (`sserafim.hxc:572-930`).

use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{BitmapFont, BlendMode, FilterMode, GpuContext, SpriteBatcher, SpriteInstance, TextureHandle};

use crate::camera::FlxEase;
use crate::play_state::PlayScene;

use super::common::{
    CutsceneAtlasProp, CutsceneAudioQueue, CutsceneAudioRequest, SkipPromptState,
};

#[derive(Debug)]
pub struct SserafimIntroState {
    main_spr: Option<CutsceneAtlasProp>,
    gf_getup: Option<CutsceneAtlasProp>,
    bf_getup: Option<CutsceneAtlasProp>,
    skip: SkipPromptState,
    elapsed_s: f32,
    sound_started: bool,
    crashed: bool,
    pan_to_stage: bool,
    started_getup: bool,
}

impl SserafimIntroState {
    #[must_use]
    pub fn start(assets: &AssetLocator, gpu: &mut GpuContext, scene: &mut PlayScene) -> Self {
        Self::set_cutscene_visibility(scene, true);

        let main_spr = CutsceneAtlasProp::load_animate(
            assets,
            gpu,
            "cutscene/cutsceneMain",
            [-395.0, 10.0],
            None,
            false,
        );
        let mut gf_getup = CutsceneAtlasProp::load_animate(
            assets,
            gpu,
            "cutscene/gfGetUp",
            [655.0, -104.0],
            Some("static"),
            false,
        );
        if let Some(g) = &mut gf_getup {
            g.visible = false;
            g.alpha = 0.5;
        }
        let mut bf_getup = CutsceneAtlasProp::load_animate(
            assets,
            gpu,
            "cutscene/bfGetUp",
            [1220.0, 531.0],
            Some("static"),
            false,
        );
        if let Some(b) = &mut bf_getup {
            b.visible = false;
        }

        scene.camera_rig.cam_game.snap_to(660.0, -200.0);
        scene.camera_rig.cam_game.tween_zoom(0.5, 0.01, FlxEase::Linear);
        scene.camera_rig.cam_game.tween_to(660.0, 300.0, 3.0, FlxEase::CircOut);
        scene.camera_rig.cam_game.tween_zoom(0.7, 3.0, FlxEase::CircOut);

        Self {
            main_spr,
            gf_getup,
            bf_getup,
            skip: SkipPromptState::new(),
            elapsed_s: 0.0,
            sound_started: false,
            crashed: false,
            pan_to_stage: false,
            started_getup: false,
        }
    }

    fn set_cutscene_visibility(scene: &mut PlayScene, in_cutscene: bool) {
        if let Some(stage) = &mut scene.stage {
            for name in ["truck", "backTables", "backStools", "frontStool"] {
                if let Some(p) = stage.prop_mut(name) {
                    p.visible = !in_cutscene;
                }
            }
            if let Some(p) = stage.prop_mut("truckDoor") {
                p.visible = false;
            }
            for name in ["backTablesCutscene", "burgerCutscene"] {
                if let Some(p) = stage.prop_mut(name) {
                    p.visible = in_cutscene;
                }
            }
        }
        if let Some(bf) = &mut scene.bf {
            bf.visible = !in_cutscene;
        }
        if let Some(dad) = &mut scene.dad {
            dad.visible = !in_cutscene;
        }
        if let Some(gf) = &mut scene.gf {
            gf.visible = false;
        }
    }

    pub fn update(
        &mut self,
        dt: f32,
        advance_pressed: bool,
        scene: &mut PlayScene,
        audio: &mut CutsceneAudioQueue,
    ) -> bool {
        let allow_skip = self.elapsed_s < (499.0 / 24.0) && !self.skip.skipped;
        if self.skip.update(dt, advance_pressed, allow_skip) {
            self.finish(scene, audio);
            return true;
        }
        if self.skip.skipped {
            audio.push(CutsceneAudioRequest::StopAllSfx);
            scene.camera_rig.update(dt);
            return false;
        }

        self.elapsed_s += dt;
        if let Some(m) = &mut self.main_spr {
            m.update(dt);
        }
        if let Some(g) = &mut self.gf_getup {
            g.update(dt);
        }
        if let Some(b) = &mut self.bf_getup {
            b.update(dt);
        }
        scene.camera_rig.update(dt);

        if !self.sound_started && self.elapsed_s >= 20.0 / 24.0 {
            self.sound_started = true;
            audio.push(CutsceneAudioRequest::PlaySound {
                key: "cutscene/startCutscene",
            });
        }
        if !self.crashed && self.elapsed_s >= 563.0 / 24.0 {
            self.crashed = true;
            Self::set_cutscene_visibility(scene, false);
            if let Some(m) = &mut self.main_spr {
                m.visible = false;
            }
            if let Some(stage) = &mut scene.stage {
                if let Some(cov) = stage.prop_mut("solidCover") {
                    cov.alpha = 1.0;
                }
            }
            scene
                .camera_rig
                .cam_game
                .flash([1.0, 1.0, 1.0, 1.0], 30.0 / 24.0);
        }
        if !self.pan_to_stage && self.elapsed_s >= 650.0 / 24.0 {
            self.pan_to_stage = true;
            scene.camera_rig.cam_game.snap_to(1070.0, 470.0);
            scene.camera_rig.cam_game.tween_zoom(0.55, 3.0, FlxEase::CircOut);
            if let Some(g) = &mut self.gf_getup {
                g.visible = true;
            }
            if let Some(b) = &mut self.bf_getup {
                b.visible = true;
            }
        }
        if self.pan_to_stage {
            if let Some(stage) = &mut scene.stage {
                if let Some(cov) = stage.prop_mut("solidCover") {
                    cov.alpha = (1.0 - (self.elapsed_s - 650.0 / 24.0) / 3.0).clamp(0.0, 1.0);
                }
            }
        }
        if !self.started_getup && self.elapsed_s >= 710.0 / 24.0 {
            self.started_getup = true;
            if let Some(g) = &mut self.gf_getup {
                g.play_label("getup", false);
            }
            if let Some(b) = &mut self.bf_getup {
                b.play_label("getup", false);
            }
        }
        if self.elapsed_s >= 730.0 / 24.0 {
            self.finish(scene, audio);
            return true;
        }
        false
    }

    fn finish(&self, scene: &mut PlayScene, audio: &mut CutsceneAudioQueue) {
        audio.push(CutsceneAudioRequest::StopAllSfx);
        Self::set_cutscene_visibility(scene, false);
        scene.camera_rig.cam_game.snap_to(1070.0, 470.0);
        scene.camera_rig.cam_game.tween_zoom(0.55, 1.0, FlxEase::CircOut);
    }

    pub fn draw_world(&self, batcher: &mut SpriteBatcher, cam_pos: [f32; 2], cam_zoom: f32) {
        if let Some(m) = &self.main_spr {
            m.draw(batcher, cam_pos, cam_zoom);
        }
        if let Some(g) = &self.gf_getup {
            g.draw(batcher, cam_pos, cam_zoom);
        }
        if let Some(b) = &self.bf_getup {
            b.draw(batcher, cam_pos, cam_zoom);
        }
    }

    pub fn draw_overlay(
        &self,
        batcher: &mut SpriteBatcher,
        white_tex: Option<TextureHandle>,
        font: &BitmapFont,
        font_tex: TextureHandle,
    ) {
        if self.elapsed_s < 3.0 {
            if let Some(wt) = white_tex {
                let a = (1.0 - self.elapsed_s / 3.0).clamp(0.0, 1.0);
                batcher.push(
                    wt,
                    BlendMode::Normal,
                    FilterMode::Nearest,
                    SpriteInstance::new([0.0, 0.0], [1280.0, 720.0])
                        .with_color([0.0, 0.0, 0.0, a]),
                );
            }
        }
        self.skip.draw(batcher, white_tex, font, font_tex);
    }
}
