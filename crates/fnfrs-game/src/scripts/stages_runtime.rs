use fnfrs_render::{BlendMode, FilterMode, SpriteBatcher, SpriteInstance, TextureHandle};

use super::spraycan::QuadCarTween;
use super::{NativeScriptHost, ScriptStepOutput, StageScript};
use crate::character::{apply_camera_transform, BaseCharacter};
use crate::stage::Stage;
impl NativeScriptHost {
    /// Per-frame update with zero heap allocations.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        elapsed_s: f32,
        song_pos_ms: f64,
        beat_length_ms: f64,
        health: f32,
        mut stage: Option<&mut Stage>,
        mut bf: Option<&mut BaseCharacter>,
        mut dad: Option<&mut BaseCharacter>,
        mut gf: Option<&mut BaseCharacter>,
    ) -> ScriptStepOutput {
        let mut out = ScriptStepOutput::default();
        self.spirit_trail.update(elapsed_s, dad.as_deref());
        let shoot_dir = self
            .speaker_shooter
            .update(song_pos_ms, gf.as_deref_mut(), &mut self.rng);
        if let Some(dir) = shoot_dir {
            self.abot.trigger_muzzle_flash(dir);
        }
        self.runners.update(elapsed_s, song_pos_ms, &mut self.rng);
        self.spraycans.update(elapsed_s, stage.as_deref_mut());

        if self.rain_enabled {
            let scale_mult = if let StageScript::PhillyBlazin { rain_time_scale, .. } = &self.stage_script {
                *rain_time_scale
            } else {
                1.0
            };
            self.rain_time += elapsed_s.max(0.0) * scale_mult;
            if song_pos_ms >= 0.0 && self.rain_duration_ms > 1.0 {
                let t = (song_pos_ms / self.rain_duration_ms).clamp(0.0, 1.0) as f32;
                self.rain_intensity = self.rain_start_intensity
                    + (self.rain_end_intensity - self.rain_start_intensity) * t;
            } else {
                self.rain_intensity = self.rain_start_intensity;
            }
        }

        let mut train_passing_flag = false;
        match &mut self.stage_script {
            StageScript::SpookyMansion {
                is_erect: true,
                flash_timer_s: Some(t),
                ..
            } => {
                *t += elapsed_s.max(0.0);
                if *t < 0.06 {
                    // Initial bright flash (silhouettes)
                } else if *t < 0.12 {
                    if let Some(ref mut st) = stage {
                        for nm in ["bgLight", "stairsLight"] {
                            if let Some(p) = st.get_named_prop_mut(nm) {
                                p.alpha = 0.0;
                            }
                        }
                    }
                    if let Some(b) = bf.as_deref_mut() {
                        b.alpha = 1.0;
                    }
                    if let Some(d) = dad.as_deref_mut() {
                        d.alpha = 1.0;
                    }
                    if let Some(g) = gf.as_deref_mut() {
                        g.alpha = 1.0;
                    }
                } else if *t < 1.62 {
                    let fade = ((*t - 0.12) / 1.5).clamp(0.0, 1.0);
                    if let Some(ref mut st) = stage {
                        for nm in ["bgLight", "stairsLight"] {
                            if let Some(p) = st.get_named_prop_mut(nm) {
                                p.alpha = 1.0 - fade;
                            }
                        }
                    }
                    if let Some(b) = bf.as_deref_mut() {
                        b.alpha = fade;
                    }
                    if let Some(d) = dad.as_deref_mut() {
                        d.alpha = fade;
                    }
                    if let Some(g) = gf.as_deref_mut() {
                        g.alpha = fade;
                    }
                } else {
                    if let Some(ref mut st) = stage {
                        for nm in ["bgLight", "stairsLight"] {
                            if let Some(p) = st.get_named_prop_mut(nm) {
                                p.alpha = 0.0;
                            }
                        }
                    }
                    if let Some(b) = bf.as_deref_mut() {
                        b.alpha = 1.0;
                    }
                    if let Some(d) = dad.as_deref_mut() {
                        d.alpha = 1.0;
                    }
                    if let Some(g) = gf.as_deref_mut() {
                        g.alpha = 1.0;
                    }
                    if let StageScript::SpookyMansion { flash_timer_s, .. } = &mut self.stage_script {
                        *flash_timer_s = None;
                    }
                }
            }
            StageScript::PhillyTrain {
                light_alpha,
                train_moving,
                train_started,
                train_finishing,
                train_cars,
                train_timer_s,
                train_sound_ms,
                ..
            } => {
                let beat_s = (beat_length_ms / 1000.0).max(0.1) as f32;
                *light_alpha = (*light_alpha - (beat_s * elapsed_s * 1.5)).max(0.0);
                if let Some(ref mut st) = stage {
                    if let Some(lights) = st.get_named_prop_mut("lights") {
                        lights.alpha = *light_alpha;
                    }
                }
                if *train_moving {
                    *train_sound_ms += elapsed_s * 1000.0;
                    *train_timer_s += elapsed_s;
                    if *train_timer_s >= 1.0 / 24.0 {
                        *train_timer_s = 0.0;
                        if *train_sound_ms >= 4700.0 {
                            *train_started = true;
                            if let Some(g) = gf.as_deref_mut() {
                                g.play_animation("hairBlow", false, false);
                            }
                        }
                        if *train_started {
                            if let Some(ref mut st) = stage {
                                if let Some(train) = st.get_named_prop_mut("train") {
                                    train.position[0] -= 400.0;
                                    if train.position[0] < -2000.0 && !*train_finishing {
                                        train.position[0] = -1150.0;
                                        *train_cars -= 1;
                                        if *train_cars <= 0 {
                                            *train_finishing = true;
                                        }
                                    }
                                    if train.position[0] < -4000.0 && *train_finishing {
                                        train.position[0] = 1480.0;
                                        *train_moving = false;
                                        *train_started = false;
                                        *train_finishing = false;
                                        *train_cars = 8;
                                        *train_sound_ms = 0.0;
                                        if let Some(g) = gf.as_deref_mut() {
                                            g.play_animation("hairFall", true, false);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                train_passing_flag = *train_started;
            }
            StageScript::PhillyBlazin {
                sky_scroll_x,
                rain_time_scale,
                lightning_timer_s,
                lightning_fade_s,
                lightning_fading,
                ..
            } => {
                let dt = elapsed_s.max(0.0);
                *sky_scroll_x -= dt * 35.0;
                let lerp_t = 1.0 - (-dt * 1.535 * 3.0).exp();
                *rain_time_scale += (0.02 - *rain_time_scale) * lerp_t;

                *lightning_timer_s -= dt;
                if *lightning_timer_s <= 0.0 {
                    *lightning_timer_s = self.rng.range_f32(7.0, 15.0);
                    *lightning_fading = true;
                    *lightning_fade_s = 0.0;
                    if let Some(ref mut st) = stage {
                        let lx = if self.rng.bool_pct(65) {
                            self.rng.range_i32(-250, 280) as f32
                        } else {
                            self.rng.range_i32(780, 900) as f32
                        };
                        if let Some(p) = st.get_named_prop_mut("lightning") {
                            p.visible = true;
                            p.position[0] = lx;
                            if let Some(bop) = &mut p.bopper {
                                bop.play_animation("strike", true);
                            }
                        }
                        if let Some(p) = st.get_named_prop_mut("skyAdditive") {
                            p.visible = true;
                            p.alpha = 0.7;
                        }
                        if let Some(p) = st.get_named_prop_mut("foregroundMultiply") {
                            p.visible = true;
                            p.alpha = 0.64;
                        }
                        if let Some(p) = st.get_named_prop_mut("additionalLighten") {
                            p.visible = true;
                            p.alpha = 0.3;
                        }
                    }
                    let s_idx = (self.rng.next_u32() as usize) % 3;
                    if let Some(slot) = self.sfx.lightning[s_idx] {
                        out.push(slot, 1.0);
                    }
                }

                if *lightning_fading {
                    *lightning_fade_s += dt;
                    let t_full = (*lightning_fade_s / 1.5).clamp(0.0, 1.0);
                    let t_fast = (*lightning_fade_s / 0.3).clamp(0.0, 1.0);
                    if let Some(ref mut st) = stage {
                        if let Some(p) = st.get_named_prop_mut("skyAdditive") {
                            p.alpha = 0.7 * (1.0 - t_full);
                            if t_full >= 1.0 {
                                p.visible = false;
                            }
                        }
                        if let Some(p) = st.get_named_prop_mut("foregroundMultiply") {
                            p.alpha = 0.64 * (1.0 - t_full);
                            if t_full >= 1.0 {
                                p.visible = false;
                            }
                        }
                        if let Some(p) = st.get_named_prop_mut("additionalLighten") {
                            p.alpha = 0.3 * (1.0 - t_fast);
                            if t_fast >= 1.0 {
                                p.visible = false;
                            }
                        }
                        if t_full >= 1.0 {
                            if let Some(p) = st.get_named_prop_mut("lightning") {
                                p.visible = false;
                            }
                        }
                    }
                    let char_c = 0.376 + (0.871 - 0.376) * t_fast;
                    let gf_c = 0.376 + (0.533 - 0.376) * t_fast;
                    if let Some(b) = bf {
                        b.color = [char_c, char_c, char_c, 1.0];
                    }
                    if let Some(d) = dad {
                        d.color = [char_c, char_c, char_c, 1.0];
                    }
                    if let Some(g) = gf.as_deref_mut() {
                        g.color = [gf_c, gf_c, gf_c, 1.0];
                    }
                    if t_full >= 1.0 {
                        *lightning_fading = false;
                    }
                }
            }
            StageScript::SchoolEvil { time_s } => {
                *time_s += elapsed_s.max(0.0);
                let t = *time_s;
                if let Some(ref mut st) = stage {
                    for (nm, w) in [
                        ("backTrees", [t, 1.6, 1.6, 0.011]),
                        ("school", [t, 2.0, 4.0, 0.017]),
                        ("street", [t, 2.0, 4.0, 0.007]),
                        ("trees", [t, 2.0, 4.0, 0.007]),
                        ("evilstreet", [t, 2.0, 4.0, 0.007]),
                        ("backspikes", [t, 1.6, 1.6, 0.011]),
                        ("backspike", [t, 2.0, 4.0, 0.010]),
                    ] {
                        if let Some(p) = st.get_named_prop_mut(nm) {
                            p.wiggle = Some(w);
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
                if *fast_car_active {
                    *fast_car_timer_s += elapsed_s;
                    if let Some(ref mut st) = stage {
                        if let Some(car) = st.get_named_prop_mut("fastCar") {
                            car.position[0] += *fast_car_vx * elapsed_s;
                        }
                    }
                    if *fast_car_timer_s >= 2.0 {
                        *fast_car_active = false;
                        *fast_car_can_drive = true;
                        if let Some(ref mut st) = stage {
                            if let Some(car) = st.get_named_prop_mut("fastCar") {
                                car.position[0] = -12600.0;
                                car.position[1] = self.rng.range_i32(350, 380) as f32;
                            }
                        }
                    }
                }
            }
            StageScript::TankmanBattlefield {
                is_erect: false,
                clouds_x,
                tank_angle,
                tank_speed,
                ..
            } => {
                *clouds_x += 8.0 * elapsed_s;
                *tank_angle += elapsed_s * *tank_speed;
                if let Some(st) = stage {
                    if let Some(tank) = st.get_named_prop_mut("tankRolling") {
                        tank.angle = *tank_angle - 90.0 + 15.0;
                        let rad = (*tank_angle + 180.0).to_radians();
                        tank.position[0] = 400.0 + rad.cos() * 1500.0;
                        tank.position[1] = 1300.0 + rad.sin() * 1100.0;
                    }
                }
            }
            StageScript::PhillyStreets {
                sky_scroll_x,
                lights_stop,
                car_waiting,
                car_interruptable,
                car2_interruptable,
                car1,
                car2,
                ..
            } => {
                *sky_scroll_x -= elapsed_s * 22.0;
                if let Some(st) = stage {
                    if let Some(c1) = st.get_named_prop_mut("phillyCars") {
                        let was_stopping = car1.stopping_at_lights;
                        let [ref mut c1_x, ref mut c1_y] = c1.position;
                        let (_, done) = car1.eval(elapsed_s, c1_x, c1_y, &mut c1.angle);
                        if done {
                            if was_stopping {
                                if !*lights_stop {
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
                                } else {
                                    *car_waiting = true;
                                }
                            } else {
                                *car_interruptable = true;
                            }
                        }
                    }
                    if let Some(c2) = st.get_named_prop_mut("phillyCars2") {
                        let [ref mut c2_x, ref mut c2_y] = c2.position;
                        let (_, done) = car2.eval(elapsed_s, c2_x, c2_y, &mut c2.angle);
                        if done {
                            *car2_interruptable = true;
                        }
                    }
                }
            }
            StageScript::Sserafim {
                yunjin,
                chaewon,
                eunchae,
                dust_time_s,
                pulse_alpha,
                pulse_dur,
                truck_light_alpha,
                truck_light_dur,
                dark_amount,
                dark_start,
                dark_target,
                dark_elapsed,
                dark_dur,
                kick_waiting_door,
                ..
            } => {
                let dt = elapsed_s.max(0.0);
                *dust_time_s += dt;
                if let Some(y) = yunjin.as_deref_mut() {
                    y.update(dt, beat_length_ms, false);
                    if *kick_waiting_door && y.active_frame_snapshot().is_some_and(|(_, f, _)| f >= 23) {
                        *kick_waiting_door = false;
                        if let Some(ref mut st) = stage {
                            if let Some(d) = st.get_named_prop_mut("truckDoor") {
                                d.alpha = 1.0;
                            }
                        }
                    }
                }
                if let Some(c) = chaewon.as_deref_mut() {
                    c.update(dt, beat_length_ms, false);
                }
                if let Some(e) = eunchae.as_deref_mut() {
                    e.update(dt, beat_length_ms, false);
                }
                if *pulse_alpha > 0.0 {
                    *pulse_alpha = (*pulse_alpha - dt / (*pulse_dur).max(0.05)).max(0.0);
                    if let Some(ref mut st) = stage {
                        if let Some(p) = st.get_named_prop_mut("backLightColor") {
                            p.alpha = *pulse_alpha;
                        }
                        if let Some(p) = st.get_named_prop_mut("backLightWhite") {
                            p.alpha = (*pulse_alpha * 0.85).clamp(0.0, 1.0);
                        }
                    }
                }
                if *truck_light_alpha > 0.0 {
                    *truck_light_alpha =
                        (*truck_light_alpha - dt / (*truck_light_dur).max(0.05)).max(0.0);
                    if let Some(ref mut st) = stage {
                        for nm in ["truckLight1", "truckLight2"] {
                            if let Some(p) = st.get_named_prop_mut(nm) {
                                p.alpha = *truck_light_alpha;
                            }
                        }
                    }
                }
                if *dark_dur > 0.0 {
                    *dark_elapsed += dt;
                    let t = (*dark_elapsed / *dark_dur).clamp(0.0, 1.0);
                    *dark_amount = *dark_start + (*dark_target - *dark_start) * t;
                    if t >= 1.0 {
                        *dark_dur = 0.0;
                    }
                }
                let shade = (1.0 - *dark_amount).clamp(0.0, 1.0);
                if let Some(ref mut st) = stage {
                    for prop in &mut st.props {
                        if prop.name.as_deref() != Some("solidCover") {
                            prop.color = [shade, shade, shade, prop.color[3]];
                        }
                    }
                }
            }
            _ => {}
        }

        self.abot
            .update(elapsed_s, song_pos_ms, health, train_passing_flag, gf);
        out
    }

    /// Draws script-injected stage layers (`tankClouds`, `phillySkybox`, `phillyBlazin/skyBlur`,
    /// `TankmanSpriteGroup`, `SpraycanPool`, `Sserafim` members/dust, `SpiritTrail`, `ABotSpeaker`).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_range(
        &self,
        batcher: &mut SpriteBatcher,
        z_start: i32,
        z_end: i32,
        cam_pos: [f32; 2],
        cam_zoom: f32,
        dad: Option<&BaseCharacter>,
        gf: Option<&BaseCharacter>,
    ) {
        let in_range = |z: i32| z >= z_start && z < z_end;

        match &self.stage_script {
            StageScript::PhillyStreets {
                sky_tex: Some((tex, tex_sz)),
                sky_scroll_x,
                ..
            } if in_range(10) => {
                draw_tiled_x(
                    batcher,
                    *tex,
                    *tex_sz,
                    [-650.0, -375.0],
                    0.65,
                    [0.1, 0.1],
                    *sky_scroll_x,
                    cam_pos,
                    cam_zoom,
                );
            }
            StageScript::PhillyBlazin {
                sky_blur_tex: Some((tex, tex_sz)),
                sky_scroll_x,
                ..
            } if in_range(10) => {
                draw_tiled_x(
                    batcher,
                    *tex,
                    *tex_sz,
                    [-700.0, -120.0],
                    1.0,
                    [0.0, 0.0],
                    *sky_scroll_x,
                    cam_pos,
                    cam_zoom,
                );
            }
            StageScript::TankmanBattlefield {
                is_erect: false,
                clouds_tex: Some((tex, tex_sz)),
                clouds_x,
                ..
            } if in_range(12) => {
                draw_tiled_x(
                    batcher,
                    *tex,
                    *tex_sz,
                    [*clouds_x, 20.0],
                    1.0,
                    [0.25, 0.25],
                    0.0,
                    cam_pos,
                    cam_zoom,
                );
            }
            StageScript::Sserafim {
                yunjin,
                chaewon,
                eunchae,
                dust_mid_tex,
                dust_back_tex,
                dust_time_s,
                ..
            } => {
                if in_range(1000) {
                    if let Some(y) = yunjin.as_deref() {
                        y.draw(batcher, cam_pos, cam_zoom);
                    }
                    if let Some(c) = chaewon.as_deref() {
                        c.draw(batcher, cam_pos, cam_zoom);
                    }
                    if let Some(e) = eunchae.as_deref() {
                        e.draw(batcher, cam_pos, cam_zoom);
                    }
                }
                if in_range(2000) {
                    if let Some((tex, sz)) = dust_mid_tex {
                        draw_tiled_x(
                            batcher,
                            *tex,
                            *sz,
                            [-650.0, -200.0],
                            1.5,
                            [1.1, 1.1],
                            *dust_time_s * 350.0,
                            cam_pos,
                            cam_zoom,
                        );
                    }
                    if let Some((tex, sz)) = dust_back_tex {
                        draw_tiled_x(
                            batcher,
                            *tex,
                            *sz,
                            [-650.0, -250.0],
                            1.5,
                            [1.15, 1.15],
                            -*dust_time_s * 300.0,
                            cam_pos,
                            cam_zoom,
                        );
                    }
                }
            }
            _ => {}
        }

        if self.spraycans.enabled && in_range(290) {
            self.spraycans.draw(batcher, cam_pos, cam_zoom);
        }

        if self.runners.enabled && in_range(self.runners.z_index) {
            self.runners.draw(batcher, cam_pos, cam_zoom);
        }

        if self.spirit_trail.enabled && in_range(self.spirit_trail.z_index) {
            self.spirit_trail.draw(batcher, cam_pos, cam_zoom, dad);
        }

        if self.abot.enabled {
            let abot_z = self.abot.gf_z - 10;
            if in_range(abot_z) {
                if let Some(g) = gf {
                    self.abot.draw_back(batcher, g, cam_pos, cam_zoom);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_tiled_x(
    batcher: &mut SpriteBatcher,
    tex: TextureHandle,
    tex_sz: [f32; 2],
    pos: [f32; 2],
    scale: f32,
    scroll: [f32; 2],
    scroll_x: f32,
    cam_pos: [f32; 2],
    cam_zoom: f32,
) {
    let tile_w = (tex_sz[0] * scale).max(1.0);
    let tile_h = tex_sz[1] * scale;
    let wrap = scroll_x.rem_euclid(tile_w);
    for copy in -1..=2 {
        let x = pos[0] + (copy as f32) * tile_w + wrap;
        let (sp, ss) = apply_camera_transform([x, pos[1]], [tile_w, tile_h], scroll, cam_pos, cam_zoom);
        batcher.push(
            tex,
            BlendMode::Normal,
            FilterMode::Linear,
            SpriteInstance::new(sp, ss),
        );
    }
}
