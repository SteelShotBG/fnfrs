use fnfrs_core::scoring::Judgement;

use super::blazin::{on_blazin_ghost_miss, on_blazin_note_hit, on_blazin_note_miss};
use super::spraycan::QuadCarTween;
use super::{NativeScriptHost, NoteKindTag, ScriptStepOutput, StageScript};
use crate::character::BaseCharacter;
use crate::input::NoteDirection;
use crate::stage::{parse_stage_color, Stage};
impl NativeScriptHost {
    /// Dispatches song chart custom events (such as `sserafimShow`, `sserafimSing`, etc.).
    #[allow(clippy::too_many_arguments)]
    pub fn on_song_event(
        &mut self,
        event_kind: &str,
        value: &serde_json::Value,
        stage: Option<&mut Stage>,
        bf: Option<&mut BaseCharacter>,
        dad: Option<&mut BaseCharacter>,
        cam_flash_dur: &mut Option<f32>,
    ) -> ScriptStepOutput {
        let mut out = ScriptStepOutput::default();
        let StageScript::Sserafim {
            yunjin,
            chaewon,
            eunchae,
            visible,
            singing,
            lights_enabled,
            light_colors,
            light_durs,
            light_ints,
            light_count,
            light_idx,
            truck_light_alpha,
            truck_light_dur,
            cover_alpha,
            dark_amount,
            dark_start,
            dark_target,
            dark_elapsed,
            dark_dur,
            kick_waiting_door,
            ..
        } = &mut self.stage_script
        else {
            return out;
        };

        match event_kind {
            "sserafimShow" => {
                if let Some(arr) = value.get("visible").and_then(|v| v.as_array()) {
                    for (i, v) in arr.iter().take(5).enumerate() {
                        if let Some(b) = v.as_bool() {
                            visible[i] = b;
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
            }
            "sserafimSing" => {
                if let Some(arr) = value.get("singing").and_then(|v| v.as_array()) {
                    for (i, v) in arr.iter().take(6).enumerate() {
                        if let Some(b) = v.as_bool() {
                            singing[i] = b;
                        }
                    }
                }
            }
            "sserafimDark" => {
                *dark_start = *dark_amount;
                *dark_target = value
                    .get("amount")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as f32;
                *dark_dur = value
                    .get("duration")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.1)
                    .max(0.0) as f32;
                *dark_elapsed = 0.0;
                if *dark_dur <= 0.0 {
                    *dark_amount = *dark_target;
                }
            }
            "sserafimLights" => {
                let amt = value
                    .get("amount")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.7) as f32;
                let dur = value
                    .get("duration")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.5)
                    .max(0.05) as f32;
                *truck_light_alpha = amt.clamp(0.0, 1.0);
                *truck_light_dur = dur;
                if let Some(st) = stage {
                    for nm in ["truckLight1", "truckLight2"] {
                        if let Some(p) = st.get_named_prop_mut(nm) {
                            p.alpha = *truck_light_alpha;
                        }
                    }
                }
            }
            "sserafimCover" => {
                let vis = value
                    .get("visible")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                *cover_alpha = if vis { 1.0 } else { 0.0 };
                if let Some(st) = stage {
                    if let Some(p) = st.get_named_prop_mut("solidCover") {
                        p.alpha = *cover_alpha;
                    }
                }
            }
            "sserafimFlash" => {
                let dur = value
                    .get("duration")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0) as f32;
                *cam_flash_dur = Some(dur);
            }
            "sserafimPulseLights" => {
                *lights_enabled = value
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if let Some(cols) = value.get("colors").and_then(|v| v.as_array()) {
                    *light_count = cols.len().min(8);
                    for (i, c) in cols.iter().take(*light_count).enumerate() {
                        if let Some(s) = c.as_str() {
                            light_colors[i] = parse_stage_color(s);
                        }
                    }
                }
                if let Some(durs) = value.get("durations").and_then(|v| v.as_array()) {
                    for (i, d) in durs.iter().take(8).enumerate() {
                        if let Some(val) = d.as_f64() {
                            light_durs[i] = val.max(0.05) as f32;
                        }
                    }
                }
                if let Some(ints) = value.get("intensities").and_then(|v| v.as_array()) {
                    for (i, val) in ints.iter().take(8).enumerate() {
                        if let Some(n) = val.as_f64() {
                            light_ints[i] = n.clamp(0.0, 1.0) as f32;
                        }
                    }
                }
                *light_idx = 0;
            }
            "sserafimKick" => {
                let is_final = value
                    .get("final")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if is_final {
                    if let Some(y) = yunjin.as_deref_mut() {
                        y.play_animation("kick2", true, false);
                        y.dance_every = 1.0;
                    }
                    *kick_waiting_door = true;
                    if let Some(slot) = self.sfx.door_kick[1] {
                        out.push(slot, 1.0);
                    }
                } else {
                    if let Some(y) = yunjin.as_deref_mut() {
                        y.play_animation("kick1", true, false);
                    }
                    if let Some(slot) = self.sfx.door_kick[0] {
                        out.push(slot, 1.0);
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// Handles `FocusCamera` events for character scripts like A-Bot eye tracking.
    pub fn on_focus_character(&mut self, char_is_bf: bool, char_is_dad: bool) {
        if char_is_bf {
            self.abot.look_right();
        } else if char_is_dad {
            self.abot.look_left();
        }
    }

    /// Returns `true` when `weekend-1-firegun` notes are currently permitted (`gunCocked`).
    #[inline]
    #[must_use]
    pub fn can_fire_gun(&self) -> bool {
        !self.spraycans.enabled || self.spraycans.gun_cocked_timer_s > 0.0
    }

    /// Plays note-kind-aware singing/special animations for player & opponent (`parents-christmas`, `spooky-dark`, `sserafim`, `2hot`, `blazin`, etc.).
    #[allow(clippy::too_many_arguments)]
    pub fn on_note_hit(
        &mut self,
        is_player: bool,
        dir: NoteDirection,
        kind: NoteKindTag,
        mut character: Option<&mut BaseCharacter>,
        dad_for_blazin: Option<&mut BaseCharacter>,
        judgement: Option<Judgement>,
        health: f32,
        stage: Option<&Stage>,
    ) -> ScriptStepOutput {
        let mut out = ScriptStepOutput::default();
        if let StageScript::PhillyBlazin { rain_time_scale, .. } = &mut self.stage_script {
            *rain_time_scale += 0.7;
            if is_player {
                if let NoteKindTag::Blazin(b_kind) = kind {
                    let is_poor = matches!(judgement, Some(Judgement::Bad | Judgement::Shit));
                    let is_low_hp = health <= 0.60;
                    let counter_roll = self.rng.bool_pct(30);
                    out.camera_shake = on_blazin_note_hit(
                        &mut self.blazin,
                        b_kind,
                        is_poor,
                        is_low_hp,
                        counter_roll,
                        character,
                        dad_for_blazin,
                    );
                    return out;
                }
            }
        }
        if let StageScript::Sserafim {
            yunjin,
            chaewon,
            eunchae,
            singing,
            ..
        } = &mut self.stage_script
        {
            if is_player && kind != NoteKindTag::NoAnim {
                if singing[0] {
                    if let Some(y) = yunjin.as_deref_mut() {
                        y.play_sing_animation(dir, false, "");
                    }
                }
                if singing[2] {
                    if let Some(c) = chaewon.as_deref_mut() {
                        c.play_sing_animation(dir, false, "");
                    }
                }
                if singing[3] {
                    if let Some(e) = eunchae.as_deref_mut() {
                        e.play_sing_animation(dir, false, "");
                    }
                }
            }
        }
        if kind == NoteKindTag::NoAnim {
            return out;
        }
        if is_player {
            match kind {
                NoteKindTag::Weekend1CockGun => {
                    self.abot.look_right();
                    self.spraycans.gun_cocked_timer_s = 1.0;
                    if let Some(c) = character {
                        c.play_animation("cock", true, true);
                    }
                    if let Some(slot) = self.sfx.gun_prep {
                        out.push(slot, 1.0);
                    }
                    return out;
                }
                NoteKindTag::Weekend1FireGun => {
                    self.abot.look_right();
                    self.spraycans.shoot();
                    if let Some(c) = character {
                        c.play_animation("shoot", true, true);
                    }
                    let shot_idx = (self.rng.next_u32() as usize) % 4;
                    if let Some(slot) = self.sfx.shot[shot_idx] {
                        out.push(slot, 1.0);
                    }
                    return out;
                }
                _ => {}
            }
            if let Some(c) = character {
                let suffix = if kind == NoteKindTag::Alt { "alt" } else { "" };
                c.play_sing_animation(dir, false, suffix);
            }
        } else {
            match kind {
                NoteKindTag::Cheer => {
                    if let Some(c) = character.as_deref_mut() {
                        if c.id.eq_ignore_ascii_case("spooky-dark") {
                            c.hold_timer = 0.0;
                            c.play_animation("cheer", true, true);
                            return out;
                        }
                    }
                }
                NoteKindTag::Ugh => {
                    if let Some(c) = character.as_deref_mut() {
                        c.play_animation("ugh", true, true);
                        return out;
                    }
                }
                NoteKindTag::HehPrettyGood => {
                    if let Some(c) = character.as_deref_mut() {
                        c.play_animation("hehPrettyGood", true, true);
                        return out;
                    }
                }
                NoteKindTag::Weekend1LightCan => {
                    self.abot.look_left();
                    if let Some(c) = character.as_deref_mut() {
                        c.play_animation("lightCan", true, true);
                    }
                    return out;
                }
                NoteKindTag::Weekend1KickCan => {
                    self.spraycans.spawn(stage);
                    if let Some(c) = character.as_deref_mut() {
                        c.play_animation("kickCan", true, true);
                    }
                    return out;
                }
                NoteKindTag::Weekend1KneeCan => {
                    if let Some(c) = character.as_deref_mut() {
                        c.play_animation("kneeCan", true, true);
                    }
                    return out;
                }
                _ => {}
            }
            if let Some(c) = character {
                let is_parents = c.id.eq_ignore_ascii_case("parents-christmas");
                let suffix = if (kind == NoteKindTag::Mom && is_parents)
                    || kind == NoteKindTag::Alt
                {
                    "alt"
                } else {
                    ""
                };
                c.play_sing_animation(dir, false, suffix);
            }
        }
        out
    }

    /// Handles player miss or ghost miss scripts (e.g. `2hot` spraycan explosion, `blazin` combat).
    pub fn on_player_miss(
        &mut self,
        dir: Option<NoteDirection>,
        kind: NoteKindTag,
        mut bf: Option<&mut BaseCharacter>,
        dad: Option<&mut BaseCharacter>,
        is_ghost: bool,
        is_fatal: bool,
    ) -> ScriptStepOutput {
        let mut out = ScriptStepOutput::default();
        if matches!(self.stage_script, StageScript::PhillyBlazin { .. }) {
            if is_ghost {
                let should_dodge = self.rng.bool_pct(50);
                out.camera_shake = on_blazin_ghost_miss(
                    &mut self.blazin,
                    is_fatal,
                    should_dodge,
                    bf,
                    dad,
                );
                return out;
            }
            if let NoteKindTag::Blazin(b_kind) = kind {
                out.camera_shake =
                    on_blazin_note_miss(&mut self.blazin, b_kind, is_fatal, bf, dad);
                return out;
            }
        }
        if kind == NoteKindTag::Weekend1LightCan {
            self.abot.look_left();
        } else if kind == NoteKindTag::Weekend1CockGun {
            self.abot.look_right();
        } else if kind == NoteKindTag::Weekend1FireGun && self.spraycans.enabled {
            self.abot.look_right();
            self.spraycans.gun_cocked_timer_s = 0.0;
            self.spraycans.miss_next_can();
            if is_fatal {
                self.spraycans.start_blacken_death();
            } else {
                self.spraycans.start_pico_flicker();
            }
            if let Some(c) = bf.as_deref_mut() {
                c.play_animation("shootMISS", true, true);
            }
            if let Some(slot) = self.sfx.pico_bonk {
                out.push(slot, 1.0);
            }
            return out;
        }
        if let (Some(d), Some(c)) = (dir, bf) {
            c.play_sing_animation(d, true, "");
        }
        out
    }

    /// Beat-triggered stage & character scripts (`spookyMansion`, `phillyTrain`, `limoRide`, `tankman`, `phillyStreets`, `sserafim`, `nene`/`abot`).
    pub fn on_beat_hit(
        &mut self,
        beat: i32,
        mut stage: Option<&mut Stage>,
        mut bf: Option<&mut BaseCharacter>,
        dad: Option<&mut BaseCharacter>,
        mut gf: Option<&mut BaseCharacter>,
    ) -> ScriptStepOutput {
        let mut out = ScriptStepOutput::default();
        self.abot.on_beat_hit(beat, gf.as_deref_mut(), &mut self.rng);

        match &mut self.stage_script {
            StageScript::SpookyMansion {
                is_erect,
                lightning_strike_beat,
                lightning_strike_offset,
                flash_timer_s,
            } => {
                let trigger_silent = beat == 4 && self.song_id.eq_ignore_ascii_case("spookeez");
                let trigger_rand = beat >= 0
                    && beat > (*lightning_strike_beat + *lightning_strike_offset)
                    && self.rng.bool_pct(10);
                if trigger_silent || trigger_rand {
                    *lightning_strike_beat = beat;
                    *lightning_strike_offset = self.rng.range_i32(8, 24);
                    if trigger_rand && !trigger_silent {
                        let s_idx = (self.rng.next_u32() & 1) as usize;
                        if let Some(slot) = self.sfx.thunder[s_idx] {
                            out.push(slot, 1.0);
                        }
                    }
                    if *is_erect {
                        *flash_timer_s = Some(0.0);
                        if let Some(st) = stage {
                            for nm in ["bgLight", "stairsLight"] {
                                if let Some(p) = st.get_named_prop_mut(nm) {
                                    p.alpha = 1.0;
                                }
                            }
                        }
                        if let Some(b) = bf.as_deref_mut() {
                            b.alpha = 0.0;
                        }
                        if let Some(d) = dad {
                            d.alpha = 0.0;
                        }
                        if let Some(g) = gf.as_deref_mut() {
                            g.alpha = 0.0;
                        }
                    } else if let Some(st) = stage {
                        if let Some(bg) = st.get_named_prop_mut("halloweenBG") {
                            if let Some(bop) = &mut bg.bopper {
                                bop.play_animation("lightning", true);
                            }
                        }
                    }
                    if let Some(b) = bf {
                        if b.has_animation("scared") && b.current_animation() != "cheer" {
                            b.play_animation("scared", true, true);
                        }
                    }
                    if let Some(g) = gf {
                        if g.has_animation("scared") {
                            g.play_animation("scared", true, true);
                        }
                    }
                }
            }
            StageScript::PhillyTrain {
                light_alpha,
                cur_light,
                train_moving,
                train_cooldown,
                ..
            } => {
                const LIGHT_COLORS: [[f32; 4]; 5] = [
                    [0.192, 0.635, 0.992, 1.0],
                    [0.192, 0.992, 0.549, 1.0],
                    [0.984, 0.200, 0.961, 1.0],
                    [0.984, 0.651, 0.200, 1.0],
                    [0.992, 0.271, 0.192, 1.0],
                ];
                if !*train_moving {
                    *train_cooldown += 1;
                }
                if beat >= 0
                    && beat % 8 == 4
                    && !*train_moving
                    && *train_cooldown > 8
                    && self.rng.bool_pct(30)
                {
                    *train_cooldown = self.rng.range_i32(-4, 0);
                    *train_moving = true;
                    if let Some(slot) = self.sfx.train_passes {
                        out.push(slot, 1.0);
                    }
                }
                if beat.rem_euclid(4) == 0 {
                    *cur_light = (self.rng.next_u32() as usize) % LIGHT_COLORS.len();
                    *light_alpha = 1.0;
                    if let Some(st) = stage {
                        if let Some(lights) = st.get_named_prop_mut("lights") {
                            lights.color = LIGHT_COLORS[*cur_light];
                            lights.alpha = 1.0;
                        }
                    }
                }
            }
            StageScript::LimoRide {
                fast_car_can_drive,
                fast_car_active,
                fast_car_timer_s,
                fast_car_vx,
            } => {
                if beat >= 0 && *fast_car_can_drive && self.rng.bool_pct(10) {
                    *fast_car_can_drive = false;
                    *fast_car_active = true;
                    *fast_car_timer_s = 0.0;
                    *fast_car_vx = self.rng.range_f32(170.0, 220.0) * 180.0;
                    let s_idx = (self.rng.next_u32() & 1) as usize;
                    if let Some(slot) = self.sfx.car_pass[s_idx] {
                        out.push(slot, 0.7);
                    }
                }
            }
            StageScript::TankmanBattlefield { is_erect: true, .. } => {
                if beat >= 0 && self.rng.bool_pct(2) {
                    if let Some(st) = stage {
                        if let Some(sniper) = st.get_named_prop_mut("sniper") {
                            if let Some(bop) = &mut sniper.bopper {
                                bop.play_animation("sip", false);
                            }
                        }
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
                if beat >= 0 && beat != (*last_change + *change_interval) && *car_interruptable && self.rng.bool_pct(10) {
                    *car_interruptable = false;
                    let variant = self.rng.range_i32(1, 4);
                    let anim_str = match variant {
                        1 => "car1",
                        2 => "car2",
                        3 => "car3",
                        _ => "car4",
                    };
                    if let Some(ref mut st) = stage {
                        if let Some(c) = st.get_named_prop_mut("phillyCars") {
                            if let Some(bop) = &mut c.bopper {
                                bop.play_animation(anim_str, true);
                            }
                        }
                    }
                    if !*lights_stop {
                        *car1 = QuadCarTween {
                            active: true,
                            elapsed_s: 0.0,
                            duration_s: match variant {
                                1 => self.rng.range_f32(1.0, 1.7),
                                2 => self.rng.range_f32(0.6, 1.2),
                                _ => self.rng.range_f32(1.5, 2.5),
                            },
                            start_delay_s: 0.0,
                            p0: [1263.4, 850.7],
                            p1: [2093.4, 761.7],
                            p2: [2795.4, 1058.7],
                            rot_start: -8.0,
                            rot_end: 18.0,
                            stopping_at_lights: false,
                        };
                    } else {
                        *car1 = QuadCarTween {
                            active: true,
                            elapsed_s: 0.0,
                            duration_s: match variant {
                                1 => self.rng.range_f32(1.0, 1.7),
                                2 => self.rng.range_f32(0.9, 1.5),
                                _ => self.rng.range_f32(1.5, 2.5),
                            },
                            start_delay_s: 0.0,
                            p0: [1173.4, 860.7],
                            p1: [1383.4, 835.7],
                            p2: [1563.4, 826.7],
                            rot_start: -7.0,
                            rot_end: -5.0,
                            stopping_at_lights: true,
                        };
                    }
                }
                if beat >= 0
                    && beat != (*last_change + *change_interval)
                    && *car2_interruptable
                    && !*lights_stop
                    && self.rng.bool_pct(10)
                {
                    *car2_interruptable = false;
                    let variant = self.rng.range_i32(1, 4);
                    let anim_str = match variant {
                        1 => "car1",
                        2 => "car2",
                        3 => "car3",
                        _ => "car4",
                    };
                    if let Some(ref mut st) = stage {
                        if let Some(c) = st.get_named_prop_mut("phillyCars2") {
                            if let Some(bop) = &mut c.bopper {
                                bop.play_animation(anim_str, true);
                            }
                        }
                    }
                    *car2 = QuadCarTween {
                        active: true,
                        elapsed_s: 0.0,
                        duration_s: match variant {
                            1 => self.rng.range_f32(1.0, 1.7),
                            2 => self.rng.range_f32(0.6, 1.2),
                            _ => self.rng.range_f32(1.5, 2.5),
                        },
                        start_delay_s: 0.0,
                        p0: [2795.4, 1018.7],
                        p1: [2093.4, 781.7],
                        p2: [1263.4, 870.7],
                        rot_start: 18.0,
                        rot_end: -8.0,
                        stopping_at_lights: false,
                    };
                }
                if beat >= 0 && beat == (*last_change + *change_interval) {
                    *last_change = beat;
                    *lights_stop = !*lights_stop;
                    if *lights_stop {
                        *change_interval = 20;
                        if let Some(ref mut st) = stage {
                            if let Some(t) = st.get_named_prop_mut("phillyTraffic") {
                                if let Some(bop) = &mut t.bopper {
                                    bop.play_animation("tored", true);
                                }
                            }
                        }
                    } else {
                        *change_interval = 30;
                        if let Some(ref mut st) = stage {
                            if let Some(t) = st.get_named_prop_mut("phillyTraffic") {
                                if let Some(bop) = &mut t.bopper {
                                    bop.play_animation("togreen", true);
                                }
                            }
                        }
                        if *car_waiting {
                            *car_waiting = false;
                            *car1 = QuadCarTween {
                                active: true,
                                elapsed_s: 0.0,
                                duration_s: self.rng.range_f32(1.8, 3.0),
                                start_delay_s: self.rng.range_f32(0.2, 1.2),
                                p0: [1563.4, 826.7],
                                p1: [2093.4, 761.7],
                                p2: [2795.4, 1058.7],
                                rot_start: -5.0,
                                rot_end: 18.0,
                                stopping_at_lights: false,
                            };
                        }
                    }
                }
            }
            StageScript::Sserafim {
                yunjin,
                chaewon,
                eunchae,
                lights_enabled,
                light_colors,
                light_durs,
                light_ints,
                light_count,
                light_idx,
                pulse_alpha,
                pulse_dur,
                ..
            } => {
                if let Some(y) = yunjin.as_deref_mut() {
                    y.on_beat_hit(beat, false);
                }
                if let Some(c) = chaewon.as_deref_mut() {
                    c.on_beat_hit(beat, false);
                }
                if let Some(e) = eunchae.as_deref_mut() {
                    e.on_beat_hit(beat, false);
                }
                if *lights_enabled && *light_count > 0 {
                    let idx = *light_idx % *light_count;
                    *light_idx = (*light_idx + 1) % *light_count;
                    let col = light_colors[idx];
                    *pulse_dur = light_durs[idx].max(0.05);
                    *pulse_alpha = (light_ints[idx] * 0.8).clamp(0.0, 1.0);
                    if let Some(st) = stage {
                        if let Some(p) = st.get_named_prop_mut("backLightColor") {
                            p.color = col;
                            p.alpha = *pulse_alpha;
                        }
                        if let Some(p) = st.get_named_prop_mut("backLightWhite") {
                            p.alpha = (*pulse_alpha * 0.85).clamp(0.0, 1.0);
                        }
                    }
                }
            }
            StageScript::None
            | StageScript::School
            | StageScript::SchoolEvil { .. }
            | StageScript::PhillyBlazin { .. }
            | StageScript::TankmanBattlefield { .. } => {}
        }

        out
    }
}
