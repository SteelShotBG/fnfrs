use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use fnfrs_audio::{decode_file, AudioEngine, StreamStemSpec, StreamedSongConfig};
use fnfrs_core::constants::DEFAULT_CHARACTER;
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::song::{SongChartData, SongMetadata};
use fnfrs_game::cutscene::CutsceneAudioRequest;
use fnfrs_game::{
    CountdownAssets, CountdownSfx, GameEventAction, GameOverAudioCue, GameplaySfxSlots,
    HealthIcon, PlayScene, ScriptSfxSlots,
};
use fnfrs_render::{GpuContext, StackTextBuffer};

use crate::gameplay_host::ActiveGameplay;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MenuSfxSlots {
    pub scroll: Option<usize>,
    pub confirm: Option<usize>,
    pub cancel: Option<usize>,
    pub vol_up: Option<usize>,
    pub vol_down: Option<usize>,
    pub vol_max: Option<usize>,
}

pub(crate) struct LoadedChart {
    pub song_id: String,
    pub difficulty: String,
    pub variation: Option<String>,
    pub metadata: Option<SongMetadata>,
    pub chart: Option<SongChartData>,
    pub label: StackTextBuffer<96>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CutsceneSfxSlots {
    pub pixel_text: Option<usize>,
    pub click_text: Option<usize>,
    pub angry_box: Option<usize>,
    pub senpai_dies: Option<usize>,
    pub lights_turn_on: Option<usize>,
    pub santa_emotion: Option<usize>,
    pub santa_shot: Option<usize>,
}

pub(crate) fn stream_single_track(engine: &mut AudioEngine, path: PathBuf) -> bool {
    let cfg = StreamedSongConfig {
        inst: Some(StreamStemSpec {
            path,
            offset_ms: 0.0,
        }),
        player_vocals: None,
        opponent_vocals: None,
        target_rate: engine.sample_rate(),
    };
    engine.load_song_streamed(cfg, 0.0).is_ok() && engine.play().is_ok()
}

pub(crate) fn play_music_by_key(assets: &AssetLocator, engine: &mut AudioEngine, key: &str) -> bool {
    stream_single_track(engine, assets.music(key))
}

pub(crate) fn register_menu_sfx(assets: &AssetLocator, engine: &mut AudioEngine) -> MenuSfxSlots {
    let sample_rate = engine.sample_rate();
    let mut bank = Vec::with_capacity(6);
    let mut slots = MenuSfxSlots::default();
    for (slot_ref, key) in [
        (&mut slots.scroll, "scrollMenu"),
        (&mut slots.confirm, "confirmMenu"),
        (&mut slots.cancel, "cancelMenu"),
        (&mut slots.vol_up, "soundtray/Volup"),
        (&mut slots.vol_down, "soundtray/Voldown"),
        (&mut slots.vol_max, "soundtray/VolMAX"),
    ] {
        let path = assets.sound(key);
        if let Ok(decoded) = decode_file(&path, sample_rate) {
            *slot_ref = Some(bank.len());
            bank.push(decoded);
        }
    }
    if !bank.is_empty() {
        let _ = engine.register_sfx_bank(bank);
    }
    slots
}

pub(crate) fn handle_game_over_audio(
    assets: &AssetLocator,
    gp: &mut ActiveGameplay,
    audio: Option<&mut AudioEngine>,
    cue: GameOverAudioCue,
) {
    let Some(engine) = audio else {
        return;
    };
    match cue {
        GameOverAudioCue::PlayLossSfx => {
            let _ = engine.pause();
            if let Some(slot) = gp.game_over_loss_sfx_slot {
                let _ = engine.play_sfx(slot, 1.0);
            }
        }
        GameOverAudioCue::StartGameOverLoop => {
            let mut buf = StackTextBuffer::<64>::new();
            let primary = gp.scene.game_over.loop_music_key(&mut buf);
            let fallback = "gameplay/gameover/gameOver";
            if play_music_by_key(assets, engine, primary)
                || play_music_by_key(assets, engine, fallback)
                || play_music_by_key(assets, engine, "gameOver")
            {
                gp.song_audio_replaced = true;
            }
        }
        GameOverAudioCue::PlayGameOverEnd => {
            let mut buf = StackTextBuffer::<64>::new();
            let primary = gp.scene.game_over.end_music_key(&mut buf);
            let fallback = "gameplay/gameover/gameOverEnd";
            if play_music_by_key(assets, engine, primary)
                || play_music_by_key(assets, engine, fallback)
                || play_music_by_key(assets, engine, "gameOverEnd")
            {
                gp.song_audio_replaced = true;
            }
        }
        GameOverAudioCue::StopMusic => {
            let _ = engine.pause();
        }
    }
}

pub(crate) fn load_song_chart(
    assets: &AssetLocator,
    song_id: &str,
    difficulty: &str,
    requested_variation: Option<&str>,
) -> LoadedChart {
    let mut label = StackTextBuffer::<96>::new();
    let effective_var = assets.resolve_song_variation(song_id, difficulty, requested_variation);
    let meta_path = assets.song_metadata(song_id, effective_var.as_deref());
    let chart_path = assets.song_chart(song_id, effective_var.as_deref());

    if let (Ok(meta_raw), Ok(chart_raw)) =
        (fs::read_to_string(&meta_path), fs::read_to_string(&chart_path))
    {
        if let (Ok(meta), Ok(chart)) = (
            serde_json::from_str::<SongMetadata>(&meta_raw),
            serde_json::from_str::<SongChartData>(&chart_raw),
        ) {
            if let Some(var) = effective_var.as_deref() {
                let _ = write!(label, "{song_id} [{difficulty}] ({var})");
            } else {
                let _ = write!(label, "{song_id} [{difficulty}]");
            }
            return LoadedChart {
                song_id: song_id.to_owned(),
                difficulty: difficulty.to_owned(),
                variation: effective_var,
                metadata: Some(meta),
                chart: Some(chart),
                label,
            };
        }
    }

    let _ = write!(label, "Demo Pattern (120 BPM)");
    LoadedChart {
        song_id: song_id.to_owned(),
        difficulty: difficulty.to_owned(),
        variation: effective_var,
        metadata: None,
        chart: None,
        label,
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn register_all_sfx(
    assets: &AssetLocator,
    engine: &mut AudioEngine,
    sample_rate: u32,
    countdown_assets: &mut CountdownAssets,
    bf_is_pixel: bool,
    stage_id: &str,
) -> (
    GameplaySfxSlots,
    Option<usize>,
    ScriptSfxSlots,
    (Option<usize>, Option<usize>, Option<usize>),
    CutsceneSfxSlots,
) {
    let mut bank = Vec::with_capacity(24);
    let mut slots = GameplaySfxSlots::default();
    let mut script_slots = ScriptSfxSlots::default();

    let miss_names = ["missnote1", "missnote2", "missnote3"];
    for (idx, name) in miss_names.iter().enumerate() {
        let path = assets.sound(name);
        if let Ok(decoded) = decode_file(&path, sample_rate) {
            slots.miss_notes[idx] = Some(bank.len());
            bank.push(decoded);
        }
    }

    let cues = [
        CountdownSfx::Intro3,
        CountdownSfx::Intro2,
        CountdownSfx::Intro1,
        CountdownSfx::IntroGo,
    ];
    for cue in cues {
        let idx = cue.as_index();
        let primary = countdown_assets.audio_key(cue).map(|k| assets.sound(k));
        let candidates = [
            primary,
            Some(assets.sound(cue.as_key())),
            Some(assets.sound(cue.vslice_name())),
        ];
        for cand in candidates.into_iter().flatten() {
            if let Ok(decoded) = decode_file(&cand, sample_rate) {
                countdown_assets.sfx_slots[idx] = Some(bank.len());
                bank.push(decoded);
                break;
            }
        }
    }

    let loss_candidates: [&str; 3] = if bf_is_pixel {
        [
            "gameplay/gameover/fnf_loss_sfx-pixel",
            "gameplay/gameover/fnf_loss_sfx",
            "fnf_loss_sfx-pixel",
        ]
    } else {
        [
            "gameplay/gameover/fnf_loss_sfx",
            "fnf_loss_sfx",
            "fnf_loss_sfx",
        ]
    };
    let mut loss_slot = None;
    for key in loss_candidates {
        let path = assets.sound(key);
        if let Ok(decoded) = decode_file(&path, sample_rate) {
            loss_slot = Some(bank.len());
            bank.push(decoded);
            break;
        }
    }

    let stage_lower = stage_id.to_ascii_lowercase();
    if stage_lower == "spookymansion" || stage_lower == "spookymansionerect" {
        for (idx, key) in ["thunder_1", "thunder_2"].iter().enumerate() {
            if let Ok(decoded) = decode_file(&assets.sound(key), sample_rate) {
                script_slots.thunder[idx] = Some(bank.len());
                bank.push(decoded);
            }
        }
    } else if stage_lower == "phillytrain" || stage_lower == "phillytrainerect" {
        if let Ok(decoded) = decode_file(&assets.sound("train_passes"), sample_rate) {
            script_slots.train_passes = Some(bank.len());
            bank.push(decoded);
        }
    } else if stage_lower == "phillyblazin" {
        for (idx, key) in ["Lightning1", "Lightning2", "Lightning3"].iter().enumerate() {
            if let Ok(decoded) = decode_file(&assets.sound(key), sample_rate) {
                script_slots.lightning[idx] = Some(bank.len());
                bank.push(decoded);
            }
        }
    } else if stage_lower == "sserafim" {
        for (idx, key) in ["doorKick1", "doorKick2"].iter().enumerate() {
            if let Ok(decoded) = decode_file(&assets.sound(key), sample_rate) {
                script_slots.door_kick[idx] = Some(bank.len());
                bank.push(decoded);
            }
        }
    } else if stage_lower == "limoride"
        || stage_lower == "phillystreets"
        || stage_lower == "phillystreetserect"
    {
        let mut week4_assets = assets.clone();
        week4_assets.set_current_level(Some("week4"));
        for (idx, key) in ["carPass0", "carPass1"].iter().enumerate() {
            let decoded = decode_file(&assets.sound(key), sample_rate)
                .or_else(|_| decode_file(&week4_assets.sound(key), sample_rate));
            if let Ok(audio) = decoded {
                script_slots.car_pass[idx] = Some(bank.len());
                bank.push(audio);
            }
        }
    }

    let mut tray_slots = (None, None, None);
    for (slot_ref, key) in [
        (&mut tray_slots.0, "soundtray/Volup"),
        (&mut tray_slots.1, "soundtray/Voldown"),
        (&mut tray_slots.2, "soundtray/VolMAX"),
    ] {
        let path = assets.sound(key);
        if let Ok(decoded) = decode_file(&path, sample_rate) {
            *slot_ref = Some(bank.len());
            bank.push(decoded);
        }
    }

    let mut cs_slots = CutsceneSfxSlots::default();
    for (slot_ref, key) in [
        (&mut cs_slots.pixel_text, "pixelText"),
        (&mut cs_slots.click_text, "clickText"),
        (&mut cs_slots.angry_box, "ANGRY_TEXT_BOX"),
        (&mut cs_slots.senpai_dies, "Senpai_Dies"),
        (&mut cs_slots.lights_turn_on, "Lights_Turn_On"),
        (&mut cs_slots.santa_emotion, "santa_emotion"),
        (&mut cs_slots.santa_shot, "santa_shot_n_falls"),
    ] {
        let path = assets.sound(key);
        if let Ok(decoded) = decode_file(&path, sample_rate) {
            *slot_ref = Some(bank.len());
            bank.push(decoded);
        }
    }

    if !bank.is_empty() {
        let _ = engine.register_sfx_bank(bank);
    }
    (slots, loss_slot, script_slots, tray_slots, cs_slots)
}

pub(crate) fn dispatch_cutscene_audio(
    assets: &AssetLocator,
    audio: Option<&mut AudioEngine>,
    sfx: CutsceneSfxSlots,
    req: CutsceneAudioRequest,
) {
    let Some(engine) = audio else {
        return;
    };
    match req {
        CutsceneAudioRequest::None => {}
        CutsceneAudioRequest::StopMusic => {
            let _ = engine.stop_bg_loop();
        }
        CutsceneAudioRequest::PlayMusic { key, .. } => {
            let path = assets.music(key);
            if let Ok(decoded) = decode_file(&path, engine.sample_rate()) {
                let _ = engine.start_bg_loop(decoded);
            }
        }
        CutsceneAudioRequest::PlaySound { key } => {
            let slot = match key {
                "pixelText" => sfx.pixel_text,
                "clickText" => sfx.click_text,
                "ANGRY_TEXT_BOX" => sfx.angry_box,
                "Senpai_Dies" => sfx.senpai_dies,
                "Lights_Turn_On" => sfx.lights_turn_on,
                "santa_emotion" => sfx.santa_emotion,
                "santa_shot_n_falls" => sfx.santa_shot,
                _ => None,
            };
            if let Some(s) = slot {
                let _ = engine.play_sfx(s, 0.9);
            }
        }
    }
}

pub(crate) fn preload_event_icons(
    assets: &AssetLocator,
    gpu: &mut GpuContext,
    scene: &mut PlayScene,
    bf_is_pixel: bool,
    dad_is_pixel: bool,
) {
    struct PreloadIconSpec {
        char_id: i32,
        icon_id: String,
        scale: Option<f32>,
        flip_x: Option<bool>,
        is_pixel: Option<bool>,
        should_bop: Option<bool>,
        offsets: Option<[f32; 2]>,
    }

    let mut to_load: Vec<PreloadIconSpec> = Vec::new();
    for ev in scene.event_queue.events() {
        if let GameEventAction::SetHealthIcon {
            char_id,
            icon_id,
            scale,
            flip_x,
            is_pixel,
            should_bop,
            offsets,
        } = &ev.action
        {
            if (*char_id == 0 || *char_id == 1)
                && !to_load
                    .iter()
                    .any(|spec| spec.char_id == *char_id && spec.icon_id.eq_ignore_ascii_case(icon_id))
            {
                to_load.push(PreloadIconSpec {
                    char_id: *char_id,
                    icon_id: icon_id.clone(),
                    scale: *scale,
                    flip_x: *flip_x,
                    is_pixel: *is_pixel,
                    should_bop: *should_bop,
                    offsets: *offsets,
                });
            }
        }
    }
    for spec in to_load {
        let is_player = spec.char_id == 0;
        let char_pix = if is_player { bf_is_pixel } else { dad_is_pixel };
        let mut icon = HealthIcon::load(assets, gpu, &spec.icon_id, is_player, None, char_pix);
        if let Some(s) = spec.scale {
            icon.base_scale = s.max(0.05);
        }
        if let Some(fx) = spec.flip_x {
            icon.flip_x = if is_player { !fx } else { fx };
        }
        if let Some(px) = spec.is_pixel {
            icon.is_pixel = px;
        }
        if let Some(bop) = spec.should_bop {
            icon.should_bop = bop;
        }
        if let Some(off) = spec.offsets {
            icon.offsets = off;
        }
        scene.register_alternate_icon(spec.char_id, icon);
    }
}

pub(crate) fn try_load_song_audio(
    assets: &AssetLocator,
    engine: &mut AudioEngine,
    sample_rate: u32,
    loaded: &LoadedChart,
) -> Option<Option<f64>> {
    let metadata = loaded.metadata.as_ref();
    let song_id = loaded.song_id.as_str();
    let variation = loaded.variation.as_deref();

    let inst_path = assets.resolve_song_inst(song_id, metadata, variation);
    if !inst_path.exists() {
        return None;
    }

    let inst_override = metadata
        .map(|m| m.play_data.characters.instrumental.trim())
        .filter(|s| !s.is_empty());

    let player_char = metadata
        .map(|m| m.play_data.characters.player.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_CHARACTER);
    let opponent_char = metadata
        .map(|m| m.play_data.characters.opponent.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("dad");

    let inst_offset = metadata.map_or(0.0, |m| m.offsets.get_instrumental_offset(inst_override));
    let pv_offset = metadata.map_or(0.0, |m| {
        m.offsets.get_vocal_offset(player_char, inst_override)
    });
    let ov_offset = metadata.map_or(0.0, |m| {
        m.offsets.get_vocal_offset(opponent_char, inst_override)
    });

    let player_vocals_list = metadata.and_then(|m| m.play_data.characters.player_vocals.as_deref());
    let opponent_vocals_list =
        metadata.and_then(|m| m.play_data.characters.opponent_vocals.as_deref());

    let mut player_vocals = assets
        .resolve_song_voices_for(song_id, player_vocals_list, player_char, variation)
        .filter(|p| p.exists())
        .map(|path| StreamStemSpec {
            path,
            offset_ms: pv_offset,
        });
    let opponent_vocals = assets
        .resolve_song_voices_for(song_id, opponent_vocals_list, opponent_char, variation)
        .filter(|p| p.exists())
        .map(|path| StreamStemSpec {
            path,
            offset_ms: ov_offset,
        });

    if player_vocals.is_none() && opponent_vocals.is_none() {
        player_vocals = assets
            .resolve_song_voices_fallback(song_id, variation)
            .filter(|p| p.exists())
            .map(|path| StreamStemSpec {
                path,
                offset_ms: pv_offset,
            });
    }

    let cfg = StreamedSongConfig {
        inst: Some(StreamStemSpec {
            path: inst_path,
            offset_ms: inst_offset,
        }),
        player_vocals,
        opponent_vocals,
        target_rate: sample_rate,
    };

    engine.load_song_streamed(cfg, 0.0).ok()
}
