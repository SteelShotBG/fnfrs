//! Core V-Slice gameplay session state (`PlayState.hx`) coordinating
//! `Conductor`, dual `Strumline`s, `InputQueue`, `PopUpPool`, `ScoreTallies`,
//! health mechanics, and `AudioEngine` SFX/stem controls with zero per-frame
//! heap allocations.

pub mod scene;
pub mod session;

pub use scene::{PlayScene, PlaySceneConfig, PlaySceneStepEvents};
pub use session::{GameplaySfxSlots, PlayConfig, PlaySession, PlayStepEvents};

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use fnfrs_core::constants::HIT_WINDOW_MS;
    use fnfrs_core::schema::character::{AnimationData, CharacterData, CharacterRenderType};
    use fnfrs_core::schema::song::{SongEventData, SongNoteData};
    use fnfrs_core::schema::stage::{ScaleSpec, StageData, StageDataCharacters, StageDataProp};
    use fnfrs_core::scoring::Judgement;
    use fnfrs_render::TextureHandle;
    use winit::keyboard::KeyCode;

    use crate::character::{BaseCharacter, CharacterKind};
    use crate::countdown::{CountdownAssets, CountdownSfx, CountdownStep};
    use crate::events::SongEventQueue;
    use crate::hud::HealthIcon;
    use crate::input::{InputQueue, NoteDirection};
    use crate::stage::Stage;
    use crate::strumline::StrumlineVisualStyle;

    fn make_anim(name: &str, looped: bool) -> AnimationData {
        AnimationData {
            name: name.to_owned(),
            prefix: Some(name.to_owned()),
            asset_path: None,
            offsets: [0.0, 0.0],
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

    fn make_test_character(id: &str, kind: CharacterKind) -> BaseCharacter {
        let data = CharacterData {
            version: "1.0.2".to_owned(),
            name: id.to_owned(),
            render_type: CharacterRenderType::Sparrow,
            asset_path: format!("characters/{id}"),
            scale: 1.0,
            health_icon: None,
            death: None,
            offsets: [0.0, 0.0],
            camera_offsets: [0.0, 0.0],
            is_pixel: false,
            dance_every: 1.0,
            sing_time: 4.0,
            animations: vec![
                make_anim("idle", false),
                make_anim("singLEFT", false),
                make_anim("singDOWN", false),
                make_anim("singUP", false),
                make_anim("singRIGHT", false),
                make_anim("singLEFTmiss", false),
                make_anim("hey", false),
            ],
            starting_animation: "idle".to_owned(),
            flip_x: false,
            apply_stage_matrix: false,
            atlas_settings: None,
        };
        BaseCharacter::new_synthetic(id, kind, [200.0, 300.0], data)
    }

    #[test]
    fn play_session_hits_and_misses_within_pbot1_window() {
        let notes = vec![
            SongNoteData {
                time: 1000.0,
                data: 0, // Left
                length: 0.0,
                kind: None,
                params: Vec::new(),
            },
            SongNoteData {
                time: 2000.0,
                data: 3, // Right
                length: 0.0,
                kind: None,
                params: Vec::new(),
            },
        ];

        let mut session = PlaySession::from_raw_notes(120.0, &notes, &[], PlayConfig::default());
        let style = StrumlineVisualStyle::fallback(TextureHandle(0));
        let mut inputs = InputQueue::default();
        let t0 = Instant::now();

        // Advance near 1005ms (5ms late -> Sick, ~500 pts).
        session.update(900.0, 1.0 / 60.0, t0, &mut inputs, None, &style);
        inputs.record_key(KeyCode::KeyA, true, false, t0);
        let ev = session.update(1004.0, 1.0 / 60.0, t0, &mut inputs, None, &style);

        assert_eq!(ev.last_judgement, Some(Judgement::Sick));
        assert_eq!(session.tallies.sick, 1);
        assert_eq!(session.tallies.combo, 1);
        assert!(session.score() >= 490);

        // Advance past 2000 + HIT_WINDOW_MS without hitting second note -> miss.
        let later = t0 + Duration::from_millis(1200);
        let ev_miss = session.update(
            2000.0 + HIT_WINDOW_MS + 10.0,
            1.0 / 60.0,
            later,
            &mut inputs,
            None,
            &style,
        );
        assert!(ev_miss.missed_note);
        assert_eq!(session.tallies.missed, 1);
        assert_eq!(session.tallies.combo, 0);
    }

    #[test]
    fn play_scene_drives_countdown_events_camera_chars_and_icons() {
        let player_notes = vec![SongNoteData {
            time: 1000.0,
            data: 0,
            length: 0.0,
            kind: None,
            params: Vec::new(),
        }];
        let opponent_notes = vec![SongNoteData {
            time: 500.0,
            data: 6, // Up on opponent strumline
            length: 0.0,
            kind: None,
            params: Vec::new(),
        }];

        let play_cfg = PlayConfig {
            scroll_speed: 1.5,
            ..Default::default()
        };
        let session =
            PlaySession::from_raw_notes(120.0, &player_notes, &opponent_notes, play_cfg);

        let mut stage_chars = StageDataCharacters::default();
        stage_chars.bf.position = [950.0, 800.0];
        stage_chars.bf.camera_offsets = [-100.0, -100.0];
        stage_chars.dad.position = [250.0, 800.0];
        stage_chars.dad.camera_offsets = [100.0, -100.0];

        let stage_data = StageData {
            version: "1.0.0".to_owned(),
            name: "Test".to_owned(),
            props: vec![StageDataProp {
                name: Some("bg".to_owned()),
                asset_path: "#112233".to_owned(),
                position: [0.0, 0.0],
                z_index: 10,
                is_pixel: false,
                flip_x: false,
                flip_y: false,
                scale: ScaleSpec::Pair([1280.0, 720.0]),
                alpha: 1.0,
                dance_every: 0.0,
                scroll: [0.0, 0.0],
                animations: Vec::new(),
                starting_animation: None,
                anim_type: "sparrow".to_owned(),
                angle: 0.0,
                blend: String::new(),
                color: "#FFFFFF".to_owned(),
                atlas_settings: None,
            }],
            characters: stage_chars,
            camera_zoom: 0.9,
            directory: "shared".to_owned(),
        };
        let stage = Stage::from_data_headless("test", stage_data);
        let bf = make_test_character("bf", CharacterKind::Bf);
        let dad = make_test_character("dad", CharacterKind::Dad);
        let gf = make_test_character("gf", CharacterKind::Gf);

        let raw_events: Vec<SongEventData> = serde_json::from_str(
            r#"[
                {"t": 0.0, "e": "FocusCamera", "v": {"char": 0, "ease": "INSTANT"}},
                {"t": 0.0, "e": "ZoomCamera", "v": {"zoom": 1.2, "duration": 0, "mode": "direct", "ease": "INSTANT"}},
                {"t": 250.0, "e": "PlayAnimation", "v": {"target": "bf", "anim": "hey", "force": true}},
                {"t": 250.0, "e": "ScrollSpeed", "v": {"scroll": 2.0, "duration": 0, "ease": "INSTANT", "absolute": true}}
            ]"#,
        )
        .expect("valid event json");
        let events = SongEventQueue::from_raw_events(&raw_events);

        let mut scene = PlayScene::new(
            session,
            Some(stage),
            Some(bf),
            Some(dad),
            Some(gf),
            events,
            HealthIcon::placeholder("bf", true, false),
            HealthIcon::placeholder("dad", false, false),
            Some(CountdownAssets::empty(false)),
            PlaySceneConfig {
                song_name: "Test Song".to_owned(),
                difficulty: "hard".to_owned(),
                start_countdown: true,
                song_duration_ms: Some(2500.0),
                death_count: 0,
            },
        );

        let style = StrumlineVisualStyle::fallback(TextureHandle(0));
        let mut inputs = InputQueue::default();
        let t0 = Instant::now();

        // 1. Pre-roll countdown (-4 * 500ms = -2000ms -> 0ms at 120 BPM).
        let c0 = scene.update(0.0, 0.0, t0, &mut inputs, None, &style);
        assert_eq!(c0.countdown_step, Some(CountdownStep::Three));
        assert_eq!(c0.countdown_sfx, Some(CountdownSfx::Intro3));
        assert!(scene.icon_p1.bop_scale_mult > 1.0);

        let c1 = scene.update(0.0, 0.5, t0, &mut inputs, None, &style);
        assert_eq!(c1.countdown_step, Some(CountdownStep::Two));
        assert_eq!(c1.countdown_sfx, Some(CountdownSfx::Intro2));

        let c2 = scene.update(0.0, 0.5, t0, &mut inputs, None, &style);
        assert_eq!(c2.countdown_step, Some(CountdownStep::One));
        assert_eq!(c2.countdown_sfx, Some(CountdownSfx::Intro1));

        let c3 = scene.update(0.0, 0.5, t0, &mut inputs, None, &style);
        assert_eq!(c3.countdown_step, Some(CountdownStep::Go));
        assert_eq!(c3.countdown_sfx, Some(CountdownSfx::IntroGo));

        let c4 = scene.update(0.0, 0.5, t0, &mut inputs, None, &style);
        assert!(c4.countdown_finished_now);
        assert!(!scene.countdown.active);

        // FocusCamera (BF) + ZoomCamera (1.2 direct) fired at t=0.
        let bf_focus = scene.bf.as_ref().unwrap().get_camera_focus_point();
        assert!((scene.camera_rig.cam_game.follow_point[0] - bf_focus[0]).abs() < 1e-3);
        assert!((scene.camera_rig.cam_game.zoom - 1.2).abs() < 1e-4);

        // 2. Advance to 250ms -> PlayAnimation("hey") on BF + ScrollSpeed(2.0).
        let _ = scene.update(250.0, 1.0 / 60.0, t0, &mut inputs, None, &style);
        assert_eq!(scene.bf.as_ref().unwrap().current_animation(), "hey");
        assert!((scene.session.player_strumline.scroll_speed - 2.0).abs() < 1e-6);
        assert!((scene.session.opponent_strumline.scroll_speed - 2.0).abs() < 1e-6);

        // 3. Advance to 500ms -> Opponent hits Up note and sings singUP.
        let opp_ev = scene.update(500.0, 1.0 / 60.0, t0, &mut inputs, None, &style);
        assert_eq!(opp_ev.gameplay.opponent_hit_dir, Some(NoteDirection::Up));
        assert_eq!(scene.dad.as_ref().unwrap().current_animation(), "singUP");

        // 4. Hit player Left note at 1000ms -> BF sings singLEFT.
        inputs.record_key(KeyCode::KeyA, true, false, t0);
        let p_ev = scene.update(1000.0, 1.0 / 60.0, t0, &mut inputs, None, &style);
        assert_eq!(p_ev.gameplay.player_hit_dir, Some(NoteDirection::Left));
        assert_eq!(scene.bf.as_ref().unwrap().current_animation(), "singLEFT");

        // 5. Reach song_duration_ms (2500ms) -> ResultsSummaryState activates.
        let end_ev = scene.update(2500.0, 1.0 / 60.0, t0, &mut inputs, None, &style);
        assert!(end_ev.song_finished_now);
        assert!(scene.results.is_active);
    }
}
