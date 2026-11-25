pub mod conductor;
pub mod constants;
pub mod paths;
pub mod pool;
pub mod schema;
pub mod scoring;

#[cfg(test)]
mod schema_tests {
    use crate::schema::character::{CharacterData, CharacterRenderType};
    use crate::schema::level::LevelData;
    use crate::schema::notestyle::NoteStyleData;
    use crate::schema::parse_hex_color;
    use crate::schema::song::{SongChartData, SongMetadata};
    use crate::schema::stage::StageData;

    #[test]
    fn parses_vslice_metadata_and_chart() {
        let meta_json = r#"{
            "version": "2.2.4",
            "songName": "Bopeebo",
            "artist": "Kawai Sprite",
            "offsets": {},
            "playData": {
                "songVariations": ["erect", "pico"],
                "difficulties": ["easy", "normal", "hard"],
                "characters": {
                    "player": "bf",
                    "girlfriend": "gf",
                    "opponent": "dad",
                    "altInstrumentals": ["pico"]
                },
                "stage": "mainStage",
                "noteStyle": "funkin"
            },
            "timeChanges": [{ "t": 0, "b": 0, "bpm": 100, "bt": [4, 4, 4, 4] }]
        }"#;
        let meta: SongMetadata = serde_json::from_str(meta_json).expect("parse metadata");
        assert_eq!(meta.song_name, "Bopeebo");
        assert!((meta.time_changes[0].bpm - 100.0).abs() < 1e-6);

        let chart_json = r#"{
            "version": "2.0.0",
            "scrollSpeed": { "easy": 1.2, "normal": 1.3, "hard": 1.6 },
            "events": [{ "t": 0, "e": "FocusCamera", "v": 1 }],
            "notes": {
                "normal": [
                    { "t": 2400, "d": 4, "l": 825 },
                    { "t": 4200, "d": 2, "k": "" }
                ]
            },
            "generatedBy": "Friday Night Funkin' - v0.8.5"
        }"#;
        let chart: SongChartData = serde_json::from_str(chart_json).expect("parse chart");
        assert!((chart.get_scroll_speed("normal") - 1.3).abs() < 1e-6);
        let notes = chart.get_notes("normal");
        assert_eq!(notes.len(), 2);
        assert!(!notes[0].is_player_note(4));
        assert!(notes[0].is_hold_note());
        assert!(notes[1].is_player_note(4));
        assert_eq!(notes[1].direction(4), 2);
        assert!(notes[1].kind.is_none());
    }

    #[test]
    fn parses_stage_character_notestyle_and_level() {
        let stage_json = r##"{
            "name": "Philly Streets",
            "cameraZoom": 0.77,
            "directory": "weekend1",
            "props": [
                { "name": "solid", "zIndex": 0, "assetPath": "#8E9191", "position": [-500, -1000], "scale": [4000, 3000], "scroll": [0, 0] },
                { "zIndex": 10, "assetPath": "sky", "position": [0, 0], "scale": 1.5 }
            ],
            "characters": {
                "bf": { "zIndex": 300, "position": [989.5, 885], "cameraOffsets": [-100, -100] },
                "dad": { "zIndex": 200, "position": [335, 885] },
                "gf": { "zIndex": 100, "position": [751.5, 787] }
            }
        }"##;
        let stage: StageData = serde_json::from_str(stage_json).expect("parse stage");
        let s0 = stage.props[0].scale.as_xy();
        let s1 = stage.props[1].scale.as_xy();
        assert!((s0[0] - 4000.0).abs() < 1e-4 && (s0[1] - 3000.0).abs() < 1e-4);
        assert!((s1[0] - 1.5).abs() < 1e-4 && (s1[1] - 1.5).abs() < 1e-4);

        let char_json = r#"{
            "version": "1.0.0",
            "name": "Boyfriend",
            "renderType": "multianimateatlas",
            "assetPath": "shared:characters/bf",
            "flipX": true,
            "animations": [{ "name": "idle", "prefix": "Idle" }]
        }"#;
        let character: CharacterData = serde_json::from_str(char_json).expect("parse char");
        assert_eq!(character.render_type, CharacterRenderType::MultiAnimateAtlas);
        assert!(character.flip_x);

        let ns_json = r#"{
            "version": "1.1.0",
            "name": "Pixel",
            "author": "Moawling",
            "fallback": "funkin",
            "assets": {
                "note": {
                    "assetPath": "week6:weeb/pixelUI/arrows-pixels",
                    "scale": 6.0,
                    "isPixel": true
                }
            }
        }"#;
        let ns: NoteStyleData = serde_json::from_str(ns_json).expect("parse notestyle");
        assert_eq!(ns.fallback.as_deref(), Some("funkin"));

        let lvl_json = r##"{
            "version": "1.0.0",
            "name": "DADDY DEAREST",
            "titleAsset": "storymenu/titles/week1",
            "background": "#F9CF51",
            "songs": ["bopeebo", "fresh", "dadbattle"]
        }"##;
        let lvl: LevelData = serde_json::from_str(lvl_json).expect("parse level");
        assert_eq!(lvl.songs.len(), 3);
        let c = parse_hex_color("#FF8000");
        assert!((c[0] - 1.0).abs() < 1e-4);
    }
}
