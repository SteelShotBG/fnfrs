use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::song::SongNoteData;
use fnfrs_render::{BlendMode, GpuContext};

use super::abot::ABotSpeaker;
use super::blazin::BlazinFighterState;
use super::speaker::SpeakerShooter;
use super::spirit::SpiritTrail;
use super::spraycan::{QuadCarTween, SpraycanPool};
use super::tankmen::TankmanRunners;
use super::{NativeScriptHost, ScriptRng, ScriptSfxSlots, StageScript};
use crate::character::{upload_png_path, BaseCharacter, CharacterKind};
use crate::stage::Stage;
pub(super) fn place_extra_sserafim_char(
    mut ch: BaseCharacter,
    x: f32,
    y: f32,
    scroll: [f32; 2],
    hsbc: [f32; 4],
) -> Box<BaseCharacter> {
    let origin = ch.character_origin();
    ch.position = [x - origin[0], y - origin[1]];
    ch.original_position = ch.position;
    ch.scroll_factor = scroll;
    ch.z_index = 1000;
    ch.adjust_color = Some(hsbc);
    ch.reset_character(false);
    Box::new(ch)
}

pub(super) fn apply_erect_adjust_color(
    stage_lower: &str,
    stage: Option<&mut Stage>,
    bf: Option<&mut BaseCharacter>,
    dad: Option<&mut BaseCharacter>,
    gf: Option<&mut BaseCharacter>,
) {
    let set_char = |ch: Option<&mut BaseCharacter>, hsbc: [f32; 4]| {
        if let Some(c) = ch {
            c.adjust_color = Some(hsbc);
        }
    };
    let set_prop = |st: &mut Stage, name: &str, hsbc: [f32; 4]| {
        if let Some(p) = st.get_named_prop_mut(name) {
            p.adjust_color = Some(hsbc);
        }
    };

    match stage_lower {
        "mainstageerect" => {
            set_char(bf, [12.0, 0.0, -23.0, 7.0]);
            set_char(gf, [-9.0, 0.0, -30.0, -4.0]);
            set_char(dad, [-32.0, 0.0, -33.0, -23.0]);
        }
        "limorideerect" => {
            let hsbc = [-30.0, -20.0, -30.0, 0.0];
            set_char(bf, hsbc);
            set_char(gf, hsbc);
            set_char(dad, hsbc);
            if let Some(st) = stage {
                for name in [
                    "limoDancer1",
                    "limoDancer2",
                    "limoDancer3",
                    "limoDancer4",
                    "limoDancer5",
                    "bgLimo",
                ] {
                    set_prop(st, name, hsbc);
                }
            }
        }
        "mallxmaserect" => {
            let chars_hsbc = [5.0, 20.0, 0.0, 0.0];
            set_char(bf, chars_hsbc);
            set_char(gf, chars_hsbc);
            set_char(dad, chars_hsbc);
            if let Some(st) = stage {
                let boppers_hsbc = [15.0, 0.0, 20.0, 0.0];
                set_prop(st, "bottomBoppers", boppers_hsbc);
                set_prop(st, "upperBoppers", boppers_hsbc);
            }
        }
        "phillytrainerect" => {
            let hsbc = [-26.0, -16.0, -5.0, 0.0];
            set_char(bf, hsbc);
            set_char(gf, hsbc);
            set_char(dad, hsbc);
        }
        "phillystreetserect" => {
            let hsbc = [-5.0, -40.0, -20.0, -25.0];
            set_char(bf, hsbc);
            set_char(gf, hsbc);
            set_char(dad, hsbc);
            if let Some(st) = stage {
                for name in ["phillyCars", "phillyCars2", "phillyTraffic", "spraycanPile"] {
                    set_prop(st, name, hsbc);
                }
            }
        }
        "schoolerect" => {
            set_char(bf, [-10.0, 0.0, -16.0, 12.0]);
            set_char(gf, [-10.0, 0.0, -20.0, 8.0]);
            set_char(dad, [-10.0, 0.0, -16.0, 12.0]);
        }
        "tankmanbattlefielderect" => {
            let hsbc = [-38.0, -20.0, -46.0, -25.0];
            set_char(bf, hsbc);
            set_char(gf, hsbc);
            set_char(dad, hsbc);
        }
        _ => {}
    }
}

impl NativeScriptHost {
    /// Initializes native script controllers from song/stage/character IDs and loads
    /// any auxiliary script textures/atlases upfront.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        song_id: &str,
        stage_id: &str,
        dad_id: &str,
        gf_id: &str,
        gf: Option<&BaseCharacter>,
        picospeaker_notes: &[SongNoteData],
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        sfx: ScriptSfxSlots,
    ) -> Self {
        let mut rng = ScriptRng(0x5A17_C0DE);
        let stage_lower = stage_id.to_ascii_lowercase();
        let stage_script = if stage_lower == "spookymansion" || stage_lower == "spookymansionerect"
        {
            StageScript::SpookyMansion {
                is_erect: stage_lower == "spookymansionerect",
                lightning_strike_beat: 0,
                lightning_strike_offset: 8,
                flash_timer_s: None,
            }
        } else if stage_lower == "phillytrain" || stage_lower == "phillytrainerect" {
            StageScript::PhillyTrain {
                light_alpha: 1.0,
                cur_light: 0,
                train_moving: false,
                train_started: false,
                train_finishing: false,
                train_cars: 8,
                train_cooldown: 0,
                train_timer_s: 0.0,
                train_sound_ms: 0.0,
            }
        } else if stage_lower == "phillyblazin" {
            let sky_blur_tex = upload_png_path(
                gpu,
                Some("phillyBlazin/skyBlur"),
                &assets.image("phillyBlazin/skyBlur"),
            );
            StageScript::PhillyBlazin {
                sky_blur_tex,
                sky_scroll_x: 0.0,
                rain_time_scale: 1.0,
                lightning_timer_s: 3.0,
                lightning_fade_s: 0.0,
                lightning_fading: false,
            }
        } else if stage_lower == "limoride" {
            StageScript::LimoRide {
                fast_car_can_drive: true,
                fast_car_active: false,
                fast_car_timer_s: 0.0,
                fast_car_vx: 0.0,
            }
        } else if stage_lower == "school" || stage_lower == "schoolerect" {
            StageScript::School
        } else if stage_lower == "schoolevil" || stage_lower == "schoolevilerect" {
            StageScript::SchoolEvil { time_s: 0.0 }
        } else if stage_lower == "tankmanbattlefield" || stage_lower == "tankmanbattlefielderect" {
            let is_erect = stage_lower == "tankmanbattlefielderect";
            let clouds_tex = if !is_erect {
                upload_png_path(gpu, Some("tankClouds"), &assets.image("tankClouds"))
            } else {
                None
            };
            StageScript::TankmanBattlefield {
                is_erect,
                clouds_tex,
                clouds_x: -1100.0,
                tank_angle: rng.range_i32(-90, 45) as f32,
                tank_speed: rng.range_f32(5.0, 7.0),
            }
        } else if stage_lower == "phillystreets" || stage_lower == "phillystreetserect" {
            let sky_tex = upload_png_path(
                gpu,
                Some("phillySkybox"),
                &assets.image("phillyStreets/phillySkybox"),
            );
            StageScript::PhillyStreets {
                sky_tex,
                sky_scroll_x: 0.0,
                lights_stop: false,
                last_change: 0,
                change_interval: 8,
                car_waiting: false,
                car_interruptable: true,
                car2_interruptable: true,
                car1: QuadCarTween::default(),
                car2: QuadCarTween::default(),
            }
        } else if stage_lower == "sserafim" {
            let yunjin = BaseCharacter::load_by_id("sserafim-yunjin", CharacterKind::Other, assets, gpu)
                .map(|c| place_extra_sserafim_char(c, -621.0, 154.0, [0.95, 0.95], [0.0; 4]));
            let chaewon = BaseCharacter::load_by_id("sserafim-chaewon", CharacterKind::Other, assets, gpu)
                .map(|c| place_extra_sserafim_char(c, 687.0, 98.0, [0.95, 0.95], [0.0; 4]));
            let eunchae = BaseCharacter::load_by_id("sserafim-eunchae", CharacterKind::Other, assets, gpu)
                .map(|c| place_extra_sserafim_char(c, 770.0, 675.0, [0.97, 0.97], [0.0; 4]));
            let dust_mid_tex = upload_png_path(gpu, Some("dustMid"), &assets.image("dust/dustMid"));
            let dust_back_tex = upload_png_path(gpu, Some("dustBack"), &assets.image("dust/dustBack"));
            StageScript::Sserafim {
                yunjin,
                chaewon,
                eunchae,
                dust_mid_tex,
                dust_back_tex,
                dust_time_s: 0.0,
                visible: [true, false, false, false, false, false],
                singing: [false; 6],
                lights_enabled: false,
                light_colors: [[1.0; 4]; 8],
                light_durs: [0.5; 8],
                light_ints: [0.8; 8],
                light_count: 0,
                light_idx: 0,
                pulse_alpha: 0.0,
                pulse_dur: 0.5,
                truck_light_alpha: 0.0,
                truck_light_dur: 0.5,
                cover_alpha: 0.0,
                dark_amount: 0.0,
                dark_start: 0.0,
                dark_target: 0.0,
                dark_elapsed: 0.0,
                dark_dur: 0.0,
                kick_waiting_door: false,
            }
        } else {
            StageScript::None
        };

        let spirit_trail = SpiritTrail {
            enabled: dad_id.eq_ignore_ascii_case("spirit"),
            ..SpiritTrail::default()
        };
        let speaker_shooter = SpeakerShooter::new(gf_id, picospeaker_notes);
        let abot = ABotSpeaker::load(gf_id, stage_id, gf, assets, gpu);
        let runners = TankmanRunners::load(song_id, stage_id, picospeaker_notes, assets, gpu, &mut rng);
        let spraycans = SpraycanPool::load(song_id, assets, gpu);

        let rain_enabled = stage_lower == "phillystreets"
            || stage_lower == "phillystreetserect"
            || stage_lower == "phillyblazin"
            || stage_lower == "spookymansionerect";
        let (rain_scale, rain_start_intensity, rain_end_intensity) = if stage_lower == "phillyblazin"
        {
            (3.6, 0.5, 0.5)
        } else if stage_lower == "spookymansionerect" {
            (7.2, 0.4, 0.4)
        } else if stage_lower == "phillystreets" || stage_lower == "phillystreetserect" {
            let song_lower = song_id.to_ascii_lowercase();
            if song_lower == "darnell" {
                (3.6, 0.0, 0.1)
            } else if song_lower == "lit-up" {
                (3.6, 0.1, 0.2)
            } else if song_lower == "2hot" {
                (3.6, 0.2, 0.4)
            } else {
                (3.6, 0.1, 0.2)
            }
        } else {
            (3.6, 0.0, 0.0)
        };

        Self {
            song_id: song_id.to_owned(),
            stage_id_lower: stage_lower,
            dad_is_gf: dad_id.eq_ignore_ascii_case("gf"),
            stage_script,
            spirit_trail,
            speaker_shooter,
            abot,
            runners,
            spraycans,
            blazin: BlazinFighterState::default(),
            sfx,
            rng,
            rain_enabled,
            rain_time: 1.0,
            rain_scale,
            rain_intensity: rain_start_intensity,
            rain_start_intensity,
            rain_end_intensity,
            rain_duration_ms: 180_000.0,
            rain_color: [0.659, 0.678, 0.710],
        }
    }

    /// Applies one-time initial stage setup & Erect `AdjustColor` shaders after placement.
    /// Places `dad` at `stage.gf_placement()` when `dad` is `"gf"` (matching `gf.hxc` `onAdd`).
    pub fn sync_gf_opponent_position(
        &self,
        stage: Option<&Stage>,
        dad: Option<&mut BaseCharacter>,
    ) {
        if !self.dad_is_gf {
            return;
        }
        if let (Some(st), Some(d)) = (stage, dad) {
            let gf_place = st.gf_placement();
            let origin = d.character_origin();
            d.position = [
                gf_place.position[0] - origin[0],
                gf_place.position[1] - origin[1],
            ];
            d.original_position = d.position;
            d.stage_camera_offsets = gf_place.camera_offsets;
            d.reset_camera_focus_point();
        }
    }

    pub fn on_stage_ready(
        &mut self,
        mut stage: Option<&mut Stage>,
        mut bf: Option<&mut BaseCharacter>,
        mut dad: Option<&mut BaseCharacter>,
        mut gf: Option<&mut BaseCharacter>,
    ) {
        self.sync_gf_opponent_position(stage.as_deref(), dad.as_deref_mut());
        apply_erect_adjust_color(
            &self.stage_id_lower,
            stage.as_deref_mut(),
            bf.as_deref_mut(),
            dad.as_deref_mut(),
            gf.as_deref_mut(),
        );
        match &mut self.stage_script {
            StageScript::SpookyMansion { is_erect: true, .. } => {
                if let Some(st) = stage {
                    for nm in ["bgLight", "stairsLight"] {
                        if let Some(p) = st.get_named_prop_mut(nm) {
                            p.alpha = 0.0;
                        }
                    }
                }
            }
            StageScript::PhillyBlazin { .. } => {
                if let Some(st) = stage {
                    for (nm, blend) in [
                        ("skyAdditive", BlendMode::Add),
                        ("lightning", BlendMode::Normal),
                        ("foregroundMultiply", BlendMode::Multiply),
                        ("additionalLighten", BlendMode::Add),
                    ] {
                        if let Some(p) = st.get_named_prop_mut(nm) {
                            p.visible = false;
                            p.blend = blend;
                        }
                    }
                }
                if let Some(b) = bf {
                    b.color = [0.871, 0.871, 0.871, 1.0];
                    b.dance_every = 0.0;
                    b.play_animation("idle", true, false);
                    b.z_index = 2000;
                }
                if let Some(d) = dad {
                    d.color = [0.871, 0.871, 0.871, 1.0];
                    d.dance_every = 0.0;
                    d.play_animation("idle", true, false);
                    d.z_index = 2000;
                }
                if let Some(g) = gf {
                    g.color = [0.533, 0.533, 0.533, 1.0];
                }
            }
            StageScript::School => {
                if self.song_id.eq_ignore_ascii_case("roses") {
                    if let Some(st) = stage {
                        if let Some(freaks) = st.get_named_prop_mut("freaks") {
                            if let Some(bop) = &mut freaks.bopper {
                                bop.idle_suffix.clear();
                                bop.idle_suffix.push_str("-scared");
                                bop.dance(true);
                            }
                        }
                    }
                }
            }
            StageScript::TankmanBattlefield { is_erect: true, .. } => {
                if let Some(st) = stage {
                    if let Some(bricks) = st.get_named_prop_mut("tankBricks") {
                        bricks.position = [445.0, 774.0];
                    }
                }
            }
            StageScript::PhillyTrain { .. } => {
                if let Some(st) = stage {
                    if let Some(train) = st.get_named_prop_mut("train") {
                        train.position[0] = 1480.0;
                    }
                }
            }
            StageScript::LimoRide { .. } => {
                if let Some(st) = stage {
                    if let Some(car) = st.get_named_prop_mut("fastCar") {
                        car.position = [-12600.0, 360.0];
                    }
                }
            }
            StageScript::PhillyStreets { .. } => {
                if let Some(st) = stage {
                    for prop in &mut st.props {
                        if prop
                            .name
                            .as_deref()
                            .is_some_and(|n| n.ends_with("_lightmap"))
                        {
                            prop.blend = BlendMode::Add;
                            prop.alpha = 0.6;
                        }
                    }
                    if let Some(traffic) = st.get_named_prop_mut("phillyTraffic") {
                        if let Some(bop) = &mut traffic.bopper {
                            bop.play_animation("togreen", true);
                        }
                    }
                }
            }
            StageScript::Sserafim {
                yunjin,
                chaewon,
                eunchae,
                visible,
                ..
            } => {
                if let Some(st) = stage {
                    for nm in [
                        "solidCover",
                        "backTablesCutscene",
                        "burgerCutscene",
                        "backLightColor",
                        "backLightWhite",
                        "truckLight1",
                        "truckLight2",
                        "truckDoor",
                    ] {
                        if let Some(p) = st.get_named_prop_mut(nm) {
                            p.alpha = 0.0;
                        }
                    }
                }
                if let Some(y) = yunjin.as_deref_mut() {
                    y.visible = visible[0];
                }
                if let Some(d) = dad {
                    d.visible = visible[1];
                }
                if let Some(c) = chaewon.as_deref_mut() {
                    c.visible = visible[2];
                }
                if let Some(e) = eunchae.as_deref_mut() {
                    e.visible = visible[3];
                }
                if let Some(b) = bf {
                    b.visible = visible[4];
                }
            }
            _ => {}
        }
    }


    /// Resets stage/character script timers cleanly on song retry.
    pub fn reset(&mut self, mut stage: Option<&mut Stage>) {
        self.spirit_trail.reset();
        self.speaker_shooter.reset();
        self.abot.reset();
        self.runners.reset();
        self.spraycans.reset();
        self.blazin.reset();
        if let Some(st) = stage.as_deref_mut() {
            for prop in &mut st.props {
                prop.color = [1.0, 1.0, 1.0, 1.0];
                prop.wiggle = None;
            }
        }
        match &mut self.stage_script {
            StageScript::SpookyMansion {
                is_erect,
                lightning_strike_beat,
                lightning_strike_offset,
                flash_timer_s,
            } => {
                *lightning_strike_beat = 0;
                *lightning_strike_offset = 8;
                *flash_timer_s = None;
                if *is_erect {
                    if let Some(st) = stage {
                        for nm in ["bgLight", "stairsLight"] {
                            if let Some(p) = st.get_named_prop_mut(nm) {
                                p.alpha = 0.0;
                            }
                        }
                    }
                }
            }
            StageScript::PhillyTrain {
                train_moving,
                train_started,
                train_finishing,
                train_cars,
                train_cooldown,
                train_timer_s,
                train_sound_ms,
                light_alpha,
                ..
            } => {
                *train_moving = false;
                *train_started = false;
                *train_finishing = false;
                *train_cars = 8;
                *train_cooldown = 0;
                *train_timer_s = 0.0;
                *train_sound_ms = 0.0;
                *light_alpha = 1.0;
                if let Some(st) = stage {
                    if let Some(train) = st.get_named_prop_mut("train") {
                        train.position[0] = 1480.0;
                    }
                }
            }
            StageScript::PhillyBlazin {
                sky_scroll_x,
                rain_time_scale,
                lightning_timer_s,
                lightning_fade_s,
                lightning_fading,
                ..
            } => {
                *sky_scroll_x = 0.0;
                *rain_time_scale = 1.0;
                *lightning_timer_s = 3.0;
                *lightning_fade_s = 0.0;
                *lightning_fading = false;
            }
            StageScript::LimoRide {
                fast_car_can_drive,
                fast_car_active,
                fast_car_timer_s,
                fast_car_vx,
            } => {
                *fast_car_can_drive = true;
                *fast_car_active = false;
                *fast_car_timer_s = 0.0;
                *fast_car_vx = 0.0;
                if let Some(st) = stage {
                    if let Some(car) = st.get_named_prop_mut("fastCar") {
                        car.position = [-12600.0, 360.0];
                    }
                }
            }
            StageScript::TankmanBattlefield { is_erect: true, .. } => {
                if let Some(st) = stage {
                    if let Some(bricks) = st.get_named_prop_mut("tankBricks") {
                        bricks.position = [445.0, 774.0];
                    }
                }
            }
            StageScript::PhillyStreets {
                lights_stop,
                last_change,
                change_interval,
                car_waiting,
                car_interruptable,
                car2_interruptable,
                car1,
                car2,
                ..
            } => {
                *lights_stop = false;
                *last_change = 0;
                *change_interval = 8;
                *car_waiting = false;
                *car_interruptable = true;
                *car2_interruptable = true;
                car1.active = false;
                car2.active = false;
                if let Some(st) = stage {
                    for nm in ["phillyCars", "phillyCars2"] {
                        if let Some(car) = st.get_named_prop_mut(nm) {
                            car.position = [1200.0, 818.0];
                            car.angle = 0.0;
                        }
                    }
                    if let Some(traffic) = st.get_named_prop_mut("phillyTraffic") {
                        if let Some(bop) = &mut traffic.bopper {
                            bop.play_animation("togreen", true);
                        }
                    }
                }
            }
            StageScript::Sserafim {
                yunjin,
                chaewon,
                eunchae,
                visible,
                singing,
                lights_enabled,
                pulse_alpha,
                truck_light_alpha,
                cover_alpha,
                dark_amount,
                kick_waiting_door,
                ..
            } => {
                *visible = [true, false, false, false, false, false];
                *singing = [false; 6];
                *lights_enabled = false;
                *pulse_alpha = 0.0;
                *truck_light_alpha = 0.0;
                *cover_alpha = 0.0;
                *dark_amount = 0.0;
                *kick_waiting_door = false;
                if let Some(y) = yunjin.as_deref_mut() {
                    y.reset_character(false);
                    y.visible = true;
                }
                if let Some(c) = chaewon.as_deref_mut() {
                    c.reset_character(false);
                    c.visible = false;
                }
                if let Some(e) = eunchae.as_deref_mut() {
                    e.reset_character(false);
                    e.visible = false;
                }
                if let Some(st) = stage {
                    for nm in ["solidCover", "backLightColor", "backLightWhite", "truckLight1", "truckLight2", "truckDoor"] {
                        if let Some(p) = st.get_named_prop_mut(nm) {
                            p.alpha = 0.0;
                        }
                    }
                }
            }
            StageScript::None | StageScript::School | StageScript::SchoolEvil { .. } | StageScript::TankmanBattlefield { .. } => {}
        }
    }
}
