//! Stress (Pico Mix) post-song ending cutscene (`stress-pico.hxc` + `tankman-bloody.hxc`).

use fnfrs_core::constants::VIRTUAL_WIDTH;
use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{BitmapFont, BlendMode, FilterMode, SpriteBatcher, SpriteInstance, StackTextBuffer, TextureHandle};

use crate::camera::FlxEase;
use crate::play_state::PlayScene;
use crate::video::SubtitleCue;

use super::common::{
    draw_subtitle_cue, load_subtitles, CutsceneAudioQueue, CutsceneAudioRequest, SkipPromptState,
};

#[derive(Debug)]
pub struct StressPicoOutroState {
    tankman_focus: [f32; 2],
    subtitles: Vec<SubtitleCue>,
    sub_buf: StackTextBuffer<160>,
    skip: SkipPromptState,
    elapsed_s: f32,
    laugh_played: bool,
    pan_up_started: bool,
}

impl StressPicoOutroState {
    #[must_use]
    pub fn start(
        assets: &AssetLocator,
        scene: &mut PlayScene,
        audio: &mut CutsceneAudioQueue,
    ) -> Self {
        let tankman_focus = scene
            .dad
            .as_ref()
            .map_or([400.0, 350.0], |d| d.camera_focus_point());

        scene.camera_rig.cam_game.tween_to(
            tankman_focus[0] + 320.0,
            tankman_focus[1] - 70.0,
            2.8,
            FlxEase::ExpoOut,
        );
        scene
            .camera_rig
            .cam_game
            .tween_zoom(0.65, 2.0, FlxEase::ExpoOut);

        if let Some(dad) = &mut scene.dad {
            dad.can_play_other_anims = true;
            dad.play_animation("stressPicoEnding", true, true);
        }
        audio.push(CutsceneAudioRequest::PlaySound {
            key: "erect/endCutscene",
        });

        Self {
            tankman_focus,
            subtitles: load_subtitles(assets, "songs/stress/subtitles/end-cutscene-pico"),
            sub_buf: StackTextBuffer::new(),
            skip: SkipPromptState::new(),
            elapsed_s: 0.0,
            laugh_played: false,
            pan_up_started: false,
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
            audio.push(CutsceneAudioRequest::StopAllSfx);
            return true;
        }
        if self.skip.skipped {
            audio.push(CutsceneAudioRequest::StopAllSfx);
            scene.camera_rig.update(dt);
            return false;
        }

        self.elapsed_s += dt;
        let beat_ms = scene.session.conductor.beat_length_ms();
        if let Some(dad) = &mut scene.dad {
            dad.update(dt, beat_ms, false);
        }
        if let Some(bf) = &mut scene.bf {
            bf.update(dt, beat_ms, false);
        }
        if let Some(gf) = &mut scene.gf {
            gf.update(dt, beat_ms, false);
        }
        scene.camera_rig.update(dt);

        if !self.laugh_played && self.elapsed_s >= 176.0 / 24.0 {
            self.laugh_played = true;
            if let Some(bf) = &mut scene.bf {
                bf.can_play_other_anims = true;
                bf.play_animation("laughEnd", true, true);
            }
        }
        if !self.pan_up_started && self.elapsed_s >= 270.0 / 24.0 {
            self.pan_up_started = true;
            scene.camera_rig.cam_game.tween_to(
                self.tankman_focus[0] + 320.0,
                self.tankman_focus[1] - 370.0,
                2.0,
                FlxEase::QuadInOut,
            );
        }
        if self.elapsed_s >= 320.0 / 24.0 {
            audio.push(CutsceneAudioRequest::StopAllSfx);
            return true;
        }
        false
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
            42.0,
        );
        let fade_start = 270.0 / 24.0;
        if !self.skip.skipped && self.elapsed_s >= fade_start {
            if let Some(wt) = white_tex {
                let fade = ((self.elapsed_s - fade_start) / 2.0).clamp(0.0, 1.0);
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
