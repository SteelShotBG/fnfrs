//! Character data, animation state machines, and multi-renderer sprite playback
//! (`BaseCharacter`, `SparrowCharacter`, `PackerCharacter`, `AnimateAtlasCharacter`,
//! `MultiSparrowCharacter`, `MultiAnimateAtlasCharacter`).
//!
//! All per-frame animation stepping and batch submission is strictly zero-allocation.

pub mod atlas_bank;
pub mod base_character;

pub use atlas_bank::apply_camera_transform;
pub use base_character::BaseCharacter;
pub(crate) use atlas_bank::{
    build_loaded_anim, compute_flipped_trim_offset, draw_animate_parts, draw_atlas_frame,
    format_stack_join, load_animate_source, load_packer_source, load_sparrow_source,
    make_loaded_anim_entry, upload_png_path, AnimBank, AnimFrames, AtlasDrawParams, LoadedAnim,
    LoadedAtlasSource,
};
/// Role of a character in gameplay (`CharacterType` in `BaseCharacter.hx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CharacterKind {
    Bf,
    Dad,
    Gf,
    #[default]
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;
    use fnfrs_core::schema::character::{AnimationData, CharacterData, CharacterRenderType};
    use fnfrs_core::schema::stage::StageDataCharacter;
    use crate::input::NoteDirection;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn make_anim(name: &str, offsets: [f32; 2], looped: bool) -> AnimationData {
        AnimationData {
            name: name.to_owned(),
            prefix: Some(name.to_owned()),
            asset_path: None,
            offsets,
            looped,
            flip_x: false,
            flip_y: false,
            frame_rate: 24,
            frame_indices: Vec::new(),
            anim_type: "framelabel".to_owned(),
            render_type: String::new(),
            atlas_settings: None,
        }
    }

    fn test_char_data(flip_x: bool, sing_time: f32, anims: Vec<AnimationData>) -> CharacterData {
        CharacterData {
            version: "1.0.2".to_owned(),
            name: "Test".to_owned(),
            render_type: CharacterRenderType::Sparrow,
            asset_path: "characters/test".to_owned(),
            scale: 1.0,
            health_icon: None,
            death: None,
            offsets: [0.0, 0.0],
            camera_offsets: [0.0, 0.0],
            is_pixel: false,
            dance_every: 1.0,
            sing_time,
            animations: anims,
            starting_animation: "idle".to_owned(),
            flip_x,
            apply_stage_matrix: false,
            atlas_settings: None,
        }
    }

    #[test]
    fn sing_timer_reverts_when_not_holding_and_respects_hold() {
        let data = test_char_data(
            false,
            4.0, // 4 steps = 1 beat = 0.5s at 120 BPM (500ms beat)
            vec![
                make_anim("idle", [0.0, 0.0], false),
                make_anim("singLEFT", [10.0, -5.0], false),
                make_anim("singRIGHTmiss", [-5.0, 0.0], false),
            ],
        );
        let mut bf = BaseCharacter::new_synthetic("bf", CharacterKind::Bf, [200.0, 300.0], data);
        assert_eq!(bf.current_animation(), "idle");

        bf.play_sing_animation(NoteDirection::Left, false, "");
        assert_eq!(bf.current_animation(), "singLEFT");
        assert!(bf.is_singing());

        // Hold note past 0.5s: BF stays singing while `is_holding == true`.
        bf.update(0.6, 500.0, true);
        assert_eq!(bf.current_animation(), "singLEFT");

        // Release note: BF reverts to idle on the next update past sing_time.
        bf.update(0.01, 500.0, false);
        assert_eq!(bf.current_animation(), "idle");
        assert!(!bf.is_singing());

        // Miss animation doubles sing_time (0.5s * 2 = 1.0s).
        bf.play_sing_animation(NoteDirection::Right, true, "");
        assert_eq!(bf.current_animation(), "singRIGHTmiss");
        bf.update(0.7, 500.0, false);
        assert_eq!(bf.current_animation(), "singRIGHTmiss");
        bf.update(0.4, 500.0, false);
        assert_eq!(bf.current_animation(), "idle");
    }

    #[test]
    fn dance_left_right_alternates_vs_idle() {
        let gf_data = test_char_data(
            false,
            8.0,
            vec![
                make_anim("danceLeft", [0.0, 0.0], false),
                make_anim("danceRight", [0.0, 0.0], false),
            ],
        );
        let mut gf = BaseCharacter::new_synthetic("gf", CharacterKind::Gf, [300.0, 400.0], gf_data);
        // `finish_init` runs initial `dance(true)` -> `danceLeft`, `has_danced` becomes `true`.
        assert_eq!(gf.current_animation(), "danceLeft");

        gf.on_beat_hit(1, false);
        assert_eq!(gf.current_animation(), "danceRight");
        gf.on_beat_hit(2, false);
        assert_eq!(gf.current_animation(), "danceLeft");

        let dad_data = test_char_data(false, 8.0, vec![make_anim("idle", [0.0, 0.0], false)]);
        let mut dad =
            BaseCharacter::new_synthetic("dad", CharacterKind::Dad, [250.0, 450.0], dad_data);
        assert_eq!(dad.current_animation(), "idle");
        dad.on_beat_hit(1, false);
        assert_eq!(dad.current_animation(), "idle");
    }

    #[test]
    fn feet_position_and_flip_x_offset_math() {
        let mut data = test_char_data(
            false,
            8.0,
            vec![
                make_anim("idle", [0.0, 0.0], false),
                make_anim("singLEFT", [20.0, -10.0], false),
            ],
        );
        data.offsets = [12.0, -8.0];
        data.scale = 1.0;

        let mut bf = BaseCharacter::new_synthetic("bf", CharacterKind::Bf, [200.0, 300.0], data);
        // Bf flips `!data.flip_x` -> `true`.
        assert!(bf.flip_x);

        let placement = StageDataCharacter {
            z_index: 300,
            position: [1000.0, 900.0],
            scale: 2.0,
            camera_offsets: [-100.0, -100.0],
            scroll: [1.0, 1.0],
            alpha: 1.0,
            angle: 0.0,
        };
        bf.place_on_stage(&placement);

        // Hitbox is [200*2, 300*2] = [400, 600], origin = [200, 600].
        // Position = [1000 - 200 + 12, 900 - 600 - 8] = [812, 292].
        assert!(approx_eq(bf.position[0], 812.0));
        assert!(approx_eq(bf.position[1], 292.0));
        let feet = bf.feet_position();
        assert!(approx_eq(feet[0], 1012.0));
        assert!(approx_eq(feet[1], 892.0));

        // Camera focus = [812 + 200 - 100, 292 + 300 - 100] = [912, 492].
        let focus = bf.get_camera_focus_point();
        assert!(approx_eq(focus[0], 912.0));
        assert!(approx_eq(focus[1], 492.0));

        // Play singLEFT (offsets [20, -10]) at scale 2.0:
        // effective_draw_origin = pos - (anim_off - global_off) * scale
        // x = 812 - (20 - 12) * 2 = 796; y = 292 - (-10 - (-8)) * 2 = 296.
        bf.play_sing_animation(NoteDirection::Left, false, "");
        let draw_orig = bf.effective_draw_origin();
        assert!(approx_eq(draw_orig[0], 796.0));
        assert!(approx_eq(draw_orig[1], 296.0));

        // Flipped trim offset math inside [200, 300] for trimmed [120, 260] at (-10, -20).
        let unflipped = compute_flipped_trim_offset(
            [200.0, 300.0],
            [120.0, 260.0],
            [-10.0, -20.0],
            false,
            false,
        );
        assert!(approx_eq(unflipped[0], 10.0));
        assert!(approx_eq(unflipped[1], 20.0));
        let flipped_x = compute_flipped_trim_offset(
            [200.0, 300.0],
            [120.0, 260.0],
            [-10.0, -20.0],
            true,
            false,
        );
        // Remaining right margin was 200 - 10 - 120 = 70 -> flipped X = 70.
        assert!(approx_eq(flipped_x[0], 70.0));
        assert!(approx_eq(flipped_x[1], 20.0));
    }

    #[test]
    fn hold_and_end_suffix_transitions() {
        let mut idle_hold = make_anim("singUP-hold", [0.0, 0.0], true);
        idle_hold.frame_rate = 24;
        let data = test_char_data(
            false,
            2.0, // 2 steps = 0.25s at 120 BPM
            vec![
                make_anim("idle", [0.0, 0.0], false),
                make_anim("singUP", [0.0, 0.0], false),
                idle_hold,
                make_anim("singUP-end", [0.0, 0.0], false),
            ],
        );
        let mut ch = BaseCharacter::new_synthetic("dad", CharacterKind::Dad, [200.0, 300.0], data);
        ch.play_sing_animation(NoteDirection::Up, false, "");
        assert_eq!(ch.current_animation(), "singUP");

        // After 1/24s (~0.05s), 1-frame singUP finishes and switches to singUP-hold.
        ch.update(0.05, 500.0, true);
        assert_eq!(ch.current_animation(), "singUP-hold");

        // After > 0.25s total, Dad reverts via singUP-end then back to idle on finish.
        ch.update(0.25, 500.0, true);
        assert_eq!(ch.current_animation(), "singUP-end");
        ch.update(0.05, 500.0, false);
        assert_eq!(ch.current_animation(), "idle");
    }
}
