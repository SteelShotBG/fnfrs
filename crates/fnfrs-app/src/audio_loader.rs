use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use fnfrs_audio::{decode_file, AudioEngine, StreamStemSpec, StreamedSongConfig, TrackKind};
use fnfrs_core::constants::DEFAULT_CHARACTER;
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::song::{SongChartData, SongMetadata};
use fnfrs_game::cutscene::{CutsceneAudioQueue, CutsceneAudioRequest};
use fnfrs_game::{
    CountdownAssets, CountdownSfx, DeathQuoteKind, GameEventAction, GameOverAudioCue,
    GameplaySfxSlots, HealthIcon, PlayScene, ScriptSfxSlots,
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
    pub pico_gasp: Option<usize>,
    pub pico_shoot: Option<usize>,
    pub pico_spin: Option<usize>,
    pub pico_cigarette: Option<usize>,
    pub pico_cigarette2: Option<usize>,
    pub pico_explode: Option<usize>,
    pub stress_end: Option<usize>,
    pub darnell_lighter: Option<usize>,
    pub gun_prep: Option<usize>,
    pub kick_can_up: Option<usize>,
    pub kick_can_forward: Option<usize>,
    pub shot1: Option<usize>,
    pub darnell_laugh: Option<usize>,
    pub nene_laugh: Option<usize>,
    pub sserafim_start: Option<usize>,
    pub death_quotes: [Option<usize>; 25],
    pub death_quote_count: usize,
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

fn start_gameover_music(
    assets: &AssetLocator,
    engine: &mut AudioEngine,
    primary: &str,
    default_key: &str,
    bare_key: &str,
) -> bool {
    if play_music_by_key(assets, engine, primary) {
        return true;
    }
    if let Some(idx) = primary.rfind('-') {
        let stripped = &primary[..idx];
        if stripped != default_key && play_music_by_key(assets, engine, stripped) {
            return true;
        }
    }
    play_music_by_key(assets, engine, default_key) || play_music_by_key(assets, engine, bare_key)
}

pub(crate) fn handle_game_over_audio(
    gp: &mut ActiveGameplay,
    audio: Option<&mut AudioEngine>,
    cue: GameOverAudioCue,
) {
    let Some(engine) = audio else {
        return;
    };
    let assets = &gp.level_assets;
    match cue {
        GameOverAudioCue::PlayLossSfx => {
            let _ = engine.pause();
            let slot = if gp.scene.game_over.blue_ball_suffix.as_str() == "-pico-explode" {
                gp.game_over_loss_explode_sfx_slot.or(gp.game_over_loss_sfx_slot)
            } else {
                gp.game_over_loss_sfx_slot
            };
            if let Some(slot) = slot {
                let _ = engine.play_sfx(slot, 1.0);
            }
        }
        GameOverAudioCue::StartGameOverLoop => {
            let mut buf = StackTextBuffer::<64>::new();
            let primary = gp.scene.game_over.loop_music_key(&mut buf);
            if start_gameover_music(
                assets,
                engine,
                primary,
                "gameplay/gameover/gameOver",
                "gameOver",
            ) {
                let _ = engine.set_track_gain(TrackKind::Inst, 1.0);
                gp.song_audio_replaced = true;
            }
        }
        GameOverAudioCue::StartGameOverLoopQuietWithQuote(_kind) => {
            let mut buf = StackTextBuffer::<64>::new();
            let primary = gp.scene.game_over.loop_music_key(&mut buf);
            if start_gameover_music(
                assets,
                engine,
                primary,
                "gameplay/gameover/gameOver",
                "gameOver",
            ) {
                let _ = engine.set_track_gain(TrackKind::Inst, 0.2);
                gp.song_audio_replaced = true;
            }
            let count = gp.cutscene_sfx.death_quote_count;
            if count > 0 {
                let seed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.subsec_nanos() as usize);
                if let Some(slot) = gp.cutscene_sfx.death_quotes[seed % count] {
                    let _ = engine.play_sfx(slot, 1.0);
                }
            }
        }
        GameOverAudioCue::RestoreLoopMusicGain => {
            let _ = engine.set_track_gain(TrackKind::Inst, 1.0);
        }
        GameOverAudioCue::PlayGameOverEnd => {
            let mut buf = StackTextBuffer::<64>::new();
            let primary = gp.scene.game_over.end_music_key(&mut buf);
            if start_gameover_music(
                assets,
                engine,
                primary,
                "gameplay/gameover/gameOverEnd",
                "gameOverEnd",
            ) {
                let _ = engine.set_track_gain(TrackKind::Inst, 1.0);
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

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn register_all_sfx(
    assets: &AssetLocator,
    engine: &mut AudioEngine,
    sample_rate: u32,
    countdown_assets: &mut CountdownAssets,
    menu_sfx: &mut MenuSfxSlots,
    blue_ball_suffix: &str,
    song_id: &str,
    stage_id: &str,
    quote_kind: DeathQuoteKind,
    naughtyness: bool,
) -> (
    GameplaySfxSlots,
    Option<usize>,
    Option<usize>,
    ScriptSfxSlots,
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

    let mut loss_slot = None;
    let mut bb_primary = StackTextBuffer::<64>::new();
    let _ = write!(bb_primary, "gameplay/gameover/fnf_loss_sfx{blue_ball_suffix}");
    let mut bb_bare = StackTextBuffer::<48>::new();
    let _ = write!(bb_bare, "fnf_loss_sfx{blue_ball_suffix}");
    let loss_candidates = [
        bb_primary.as_str(),
        bb_bare.as_str(),
        "gameplay/gameover/fnf_loss_sfx",
        "fnf_loss_sfx",
    ];
    for key in loss_candidates {
        let path = assets.sound(key);
        if let Ok(decoded) = decode_file(&path, sample_rate) {
            loss_slot = Some(bank.len());
            bank.push(decoded);
            break;
        }
    }

    let mut loss_explode_slot = None;
    if song_id.eq_ignore_ascii_case("2hot") {
        let explode_path = assets.sound("gameplay/gameover/fnf_loss_sfx-pico-explode");
        if let Ok(decoded) = decode_file(&explode_path, sample_rate) {
            loss_explode_slot = Some(bank.len());
            bank.push(decoded);
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
        if stage_lower.starts_with("phillystreets") || song_id.eq_ignore_ascii_case("2hot") {
            if let Ok(decoded) = decode_file(&assets.sound("Gun_Prep"), sample_rate) {
                script_slots.gun_prep = Some(bank.len());
                bank.push(decoded);
            }
            if let Ok(decoded) = decode_file(&assets.sound("Pico_Bonk"), sample_rate) {
                script_slots.pico_bonk = Some(bank.len());
                bank.push(decoded);
            }
            for (idx, key) in ["shot1", "shot2", "shot3", "shot4"].iter().enumerate() {
                if let Ok(decoded) = decode_file(&assets.sound(key), sample_rate) {
                    script_slots.shot[idx] = Some(bank.len());
                    bank.push(decoded);
                }
            }
        }
    }

    for (slot_ref, key) in [
        (&mut menu_sfx.scroll, "scrollMenu"),
        (&mut menu_sfx.confirm, "confirmMenu"),
        (&mut menu_sfx.cancel, "cancelMenu"),
        (&mut menu_sfx.vol_up, "soundtray/Volup"),
        (&mut menu_sfx.vol_down, "soundtray/Voldown"),
        (&mut menu_sfx.vol_max, "soundtray/VolMAX"),
    ] {
        let path = assets.sound(key);
        if let Ok(decoded) = decode_file(&path, sample_rate) {
            *slot_ref = Some(bank.len());
            bank.push(decoded);
        }
    }

    let song_lower = song_id.to_ascii_lowercase();
    let mut cs_slots = CutsceneSfxSlots::default();
    let load_sfx = |slot_ref: &mut Option<usize>, key: &str, bank: &mut Vec<_>| {
        let path = assets.sound(key);
        if let Ok(decoded) = decode_file(&path, sample_rate) {
            *slot_ref = Some(bank.len());
            bank.push(decoded);
        }
    };

    if matches!(
        song_lower.as_str(),
        "senpai" | "roses" | "thorns" | "winter-horrorland"
    ) {
        load_sfx(&mut cs_slots.pixel_text, "pixelText", &mut bank);
        load_sfx(&mut cs_slots.click_text, "clickText", &mut bank);
        load_sfx(&mut cs_slots.angry_box, "ANGRY_TEXT_BOX", &mut bank);
        load_sfx(&mut cs_slots.senpai_dies, "Senpai_Dies", &mut bank);
        load_sfx(&mut cs_slots.lights_turn_on, "Lights_Turn_On", &mut bank);
    }
    if song_lower == "eggnog" {
        load_sfx(&mut cs_slots.santa_emotion, "santa_emotion", &mut bank);
        load_sfx(&mut cs_slots.santa_shot, "santa_shot_n_falls", &mut bank);
    }
    if stage_lower == "phillytrainerect" {
        load_sfx(&mut cs_slots.pico_gasp, "cutscene/picoGasp", &mut bank);
        load_sfx(&mut cs_slots.pico_shoot, "cutscene/picoShoot", &mut bank);
        load_sfx(&mut cs_slots.pico_spin, "cutscene/picoSpin", &mut bank);
        load_sfx(&mut cs_slots.pico_cigarette, "cutscene/picoCigarette", &mut bank);
        load_sfx(&mut cs_slots.pico_cigarette2, "cutscene/picoCigarette2", &mut bank);
        load_sfx(&mut cs_slots.pico_explode, "cutscene/picoExplode", &mut bank);
    }
    if song_lower == "stress" {
        load_sfx(&mut cs_slots.stress_end, "erect/endCutscene", &mut bank);
    }
    if song_lower == "darnell" {
        load_sfx(&mut cs_slots.darnell_lighter, "Darnell_Lighter", &mut bank);
        load_sfx(&mut cs_slots.gun_prep, "Gun_Prep", &mut bank);
        load_sfx(&mut cs_slots.kick_can_up, "Kick_Can_UP", &mut bank);
        load_sfx(&mut cs_slots.kick_can_forward, "Kick_Can_FORWARD", &mut bank);
        load_sfx(&mut cs_slots.shot1, "shot1", &mut bank);
        load_sfx(&mut cs_slots.darnell_laugh, "cutscene/darnell_laugh", &mut bank);
        load_sfx(&mut cs_slots.nene_laugh, "cutscene/nene_laugh", &mut bank);
    }
    if stage_lower == "sserafim" {
        load_sfx(&mut cs_slots.sserafim_start, "cutscene/startCutscene", &mut bank);
    }

    match quote_kind {
        DeathQuoteKind::Tankman => {
            for i in 1..=25_u8 {
                if !naughtyness && matches!(i, 1 | 3 | 8 | 13 | 17 | 21) {
                    continue;
                }
                let mut key = StackTextBuffer::<48>::new();
                let _ = write!(key, "jeffGameover/jeffGameover-{i}");
                if let Ok(decoded) = decode_file(&assets.sound(key.as_str()), sample_rate) {
                    let idx = cs_slots.death_quote_count;
                    if idx < cs_slots.death_quotes.len() {
                        cs_slots.death_quotes[idx] = Some(bank.len());
                        cs_slots.death_quote_count += 1;
                        bank.push(decoded);
                    }
                }
            }
        }
        DeathQuoteKind::PicoHoldingNene => {
            for i in 1..=10_u8 {
                let mut key = StackTextBuffer::<48>::new();
                let _ = write!(key, "jeffGameover-pico/jeffGameover-{i}");
                if let Ok(decoded) = decode_file(&assets.sound(key.as_str()), sample_rate) {
                    let idx = cs_slots.death_quote_count;
                    if idx < cs_slots.death_quotes.len() {
                        cs_slots.death_quotes[idx] = Some(bank.len());
                        cs_slots.death_quote_count += 1;
                        bank.push(decoded);
                    }
                }
            }
        }
        DeathQuoteKind::None => {}
    }

    if !bank.is_empty() {
        let _ = engine.register_sfx_bank(bank);
    }
    (slots, loss_slot, loss_explode_slot, script_slots, cs_slots)
}

pub(crate) fn dispatch_cutscene_audio(
    assets: &AssetLocator,
    audio: Option<&mut AudioEngine>,
    sfx: CutsceneSfxSlots,
    queue: CutsceneAudioQueue,
) {
    let Some(engine) = audio else {
        return;
    };
    for &req in &queue.items[..queue.count] {
        match req {
            CutsceneAudioRequest::None => {}
            CutsceneAudioRequest::StopMusic => {
                let _ = engine.stop_bg_loop();
            }
            CutsceneAudioRequest::StopAllSfx => {
                let _ = engine.stop_all_sfx();
            }
            CutsceneAudioRequest::PauseBgLoop(paused) => {
                let _ = engine.set_bg_loop_paused(paused);
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
                    "cutscene/picoGasp" => sfx.pico_gasp,
                    "cutscene/picoShoot" => sfx.pico_shoot,
                    "cutscene/picoSpin" => sfx.pico_spin,
                    "cutscene/picoCigarette" => sfx.pico_cigarette,
                    "cutscene/picoCigarette2" => sfx.pico_cigarette2,
                    "cutscene/picoExplode" => sfx.pico_explode,
                    "erect/endCutscene" => sfx.stress_end,
                    "Darnell_Lighter" => sfx.darnell_lighter,
                    "Gun_Prep" => sfx.gun_prep,
                    "Kick_Can_UP" => sfx.kick_can_up,
                    "Kick_Can_FORWARD" => sfx.kick_can_forward,
                    "shot1" => sfx.shot1,
                    "cutscene/darnell_laugh" => sfx.darnell_laugh,
                    "cutscene/nene_laugh" => sfx.nene_laugh,
                    "cutscene/startCutscene" => sfx.sserafim_start,
                    _ => None,
                };
                if let Some(s) = slot {
                    let _ = engine.play_sfx(s, 0.9);
                }
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
