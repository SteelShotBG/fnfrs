//! Eggnog (Erect) post-song Santa execution cutscene (`eggnog-erect.hxc`,
//! `SantaDiesSprite.hxc`, `DadShootsSprite.hxc`).

use fnfrs_core::constants::VIRTUAL_WIDTH;
use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{BitmapFont, BlendMode, FilterMode, GpuContext, SpriteBatcher, SpriteInstance, StackTextBuffer, TextureHandle};

use crate::camera::FlxEase;
use crate::play_state::PlayScene;
use crate::video::SubtitleCue;

use super::common::{
    draw_subtitle_cue, load_subtitles, CutsceneAtlasProp, CutsceneAudioQueue,
    CutsceneAudioRequest, SkipPromptState,
};

#[derive(Debug)]
pub struct EggnogErectOutroState {
    santa_dead: Option<CutsceneAtlasProp>,
    parents_shoot: Option<CutsceneAtlasProp>,
    subtitles: Vec<SubtitleCue>,
    sub_buf: StackTextBuffer<160>,
    skip: SkipPromptState,
    elapsed_s: f32,
    pan_stage_started: bool,
    shot_sfx_played: bool,
    shake_triggered: bool,
}

impl EggnogErectOutroState {
    #[must_use]
    pub fn start(assets: &AssetLocator, gpu: &mut GpuContext, scene: &mut PlayScene) -> Self {
        if let Some(santa_prop) = scene.stage.as_mut().and_then(|s| s.prop_mut("santa")) {
            santa_prop.visible = false;
        }
        if let Some(dad) = &mut scene.dad {
            dad.visible = false;
        }

        let santa_dead = CutsceneAtlasProp::load_animate(
            assets,
            gpu,
            "christmas/santa_speaks_assets",
            [-1300.0, 100.0],
            None,
            false,
        );
        let parents_shoot = CutsceneAtlasProp::load_animate(
            assets,
            gpu,
            "christmas/parents_shoot_assets",
            [-602.0, -3.5],
            None,
            false,
        );

        let santa_x = -1300.0_f32;
        let santa_y = 100.0_f32;
        scene
            .camera_rig
            .cam_game
            .tween_to(santa_x + 1200.0, santa_y + 300.0, 2.8, FlxEase::ExpoOut);
        scene
            .camera_rig
            .cam_game
            .tween_zoom(0.73, 2.0, FlxEase::QuadInOut);

        Self {
            santa_dead,
            parents_shoot,
            subtitles: load_subtitles(assets, "songs/eggnog/subtitles/santa-emotions"),
            sub_buf: StackTextBuffer::new(),
            skip: SkipPromptState::new(),
            elapsed_s: 0.0,
            pan_stage_started: false,
            shot_sfx_played: false,
            shake_triggered: false,
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
            self.cleanup(scene, audio);
            return true;
        }
        if self.skip.skipped {
            audio.push(CutsceneAudioRequest::StopAllSfx);
            scene.camera_rig.update(dt);
            return false;
        }

        self.elapsed_s += dt;
        if let Some(s) = &mut self.santa_dead {
            s.update(dt);
        }
        if let Some(p) = &mut self.parents_shoot {
            p.update(dt);
        }
        scene.camera_rig.update(dt);

        let santa_x = -1300.0_f32;
        let santa_y = 100.0_f32;
        if !self.pan_stage_started && self.elapsed_s >= 2.8 {
            self.pan_stage_started = true;
            scene
                .camera_rig
                .cam_game
                .tween_to(santa_x + 1050.0, santa_y + 300.0, 9.0, FlxEase::QuartInOut);
            scene
                .camera_rig
                .cam_game
                .tween_zoom(0.79, 9.0, FlxEase::QuadInOut);
        }
        if !self.shot_sfx_played && self.elapsed_s >= 11.375 {
            self.shot_sfx_played = true;
            audio.push(CutsceneAudioRequest::PlaySound {
                key: "santa_shot_n_falls",
            });
        }
        if !self.shake_triggered && self.elapsed_s >= 12.83 {
            self.shake_triggered = true;
            scene.camera_rig.cam_game.shake(0.005, 0.2);
            scene
                .camera_rig
                .cam_game
                .tween_to(santa_x + 1060.0, santa_y + 380.0, 5.0, FlxEase::ExpoOut);
        }

        if self.elapsed_s >= 16.0 {
            self.cleanup(scene, audio);
            return true;
        }
        false
    }

    fn cleanup(&self, scene: &mut PlayScene, audio: &mut CutsceneAudioQueue) {
        audio.push(CutsceneAudioRequest::StopAllSfx);
        if let Some(santa_prop) = scene.stage.as_mut().and_then(|s| s.prop_mut("santa")) {
            santa_prop.visible = true;
        }
        if let Some(dad) = &mut scene.dad {
            dad.visible = true;
        }
    }

    pub fn draw_world(&self, batcher: &mut SpriteBatcher, cam_pos: [f32; 2], cam_zoom: f32) {
        if let Some(p) = &self.parents_shoot {
            p.draw(batcher, cam_pos, cam_zoom);
        }
        if let Some(s) = &self.santa_dead {
            s.draw(batcher, cam_pos, cam_zoom);
        }
    }

    pub fn draw_overlay(
        &mut self,
        batcher: &mut SpriteBatcher,
        white_tex: Option<TextureHandle>,
        font: &BitmapFont,
        font_tex: TextureHandle,
    ) {
        draw_subtitle_cue(
            &self.subtitles,
            self.elapsed_s,
            batcher,
            font,
            font_tex,
            &mut self.sub_buf,
            0.0,
        );
        if !self.skip.skipped && self.elapsed_s >= 15.0 {
            if let Some(wt) = white_tex {
                let fade = ((self.elapsed_s - 15.0) / 1.0).clamp(0.0, 1.0);
                batcher.push(
                    wt,
                    BlendMode::Normal,
                    FilterMode::Nearest,
                    SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, 720.0])
                        .with_color([0.0, 0.0, 0.0, fade]),
                );
            }
        }
        self.skip.draw(batcher, white_tex, font, font_tex);
    }
}
