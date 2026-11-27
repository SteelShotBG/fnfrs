use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::song::SongNoteData;
use fnfrs_render::{
    AtlasFrame, BlendMode, FilterMode, GpuContext, SparrowAtlas, SpriteBatcher, SpriteInstance,
    TextureHandle, SPRITE_FLAG_FLIP_X, SPRITE_FLAG_ROTATED_90_CCW,
};

use super::ScriptRng;
use crate::character::{apply_camera_transform, compute_flipped_trim_offset, upload_png_path};
#[derive(Debug, Clone)]
struct TankRunner {
    active: bool,
    visible: bool,
    strum_time_ms: f64,
    going_right: bool,
    ending_offset: f32,
    run_speed: f32,
    scale: f32,
    x: f32,
    y: f32,
    shot: bool,
    shot_variant: usize,
    anim_time_s: f32,
    flicker_timer_s: f32,
    flickering: bool,
}

impl Default for TankRunner {
    fn default() -> Self {
        Self {
            active: false,
            visible: true,
            strum_time_ms: 0.0,
            going_right: false,
            ending_offset: 0.0,
            run_speed: 0.8,
            scale: 1.0,
            x: 9999.0,
            y: 250.0,
            shot: false,
            shot_variant: 0,
            anim_time_s: 0.0,
            flicker_timer_s: 0.0,
            flickering: false,
        }
    }
}

#[derive(Debug)]
pub(super) struct TankmanRunners {
    pub(super) enabled: bool,
    is_erect: bool,
    pub(super) z_index: i32,
    texture: Option<TextureHandle>,
    tex_size: [f32; 2],
    run_frames: Vec<AtlasFrame>,
    shot_frames: [Vec<AtlasFrame>; 2],
    timemap: Vec<(f64, bool)>,
    next_spawn: usize,
    pool: [TankRunner; 4],
}

impl Default for TankmanRunners {
    fn default() -> Self {
        Self {
            pub(super) enabled: false,
            is_erect: false,
            pub(super) z_index: 30,
            texture: None,
            tex_size: [1.0, 1.0],
            run_frames: Vec::new(),
            shot_frames: [Vec::new(), Vec::new()],
            timemap: Vec::new(),
            next_spawn: 0,
            pool: std::array::from_fn(|_| TankRunner::default()),
        }
    }
}

impl TankmanRunners {
    pub(super) fn load(
        song_id: &str,
        stage_id: &str,
        picospeaker_notes: &[SongNoteData],
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        rng: &mut ScriptRng,
    ) -> Self {
        let stage_lower = stage_id.to_ascii_lowercase();
        if !song_id.eq_ignore_ascii_case("stress")
            || (stage_lower != "tankmanbattlefield" && stage_lower != "tankmanbattlefielderect")
            || picospeaker_notes.is_empty()
        {
            return Self::default();
        }
        let is_erect = stage_lower == "tankmanbattlefielderect";
        let xml_path = assets.sparrow_xml("tankmanKilled1");
        let png_path = assets.image("tankmanKilled1");
        let (Ok(xml), Some((tex, tex_size))) = (
            std::fs::read_to_string(&xml_path),
            upload_png_path(gpu, Some("tankmanKilled1"), &png_path),
        ) else {
            return Self::default();
        };
        let Ok(sparrow) = SparrowAtlas::from_xml(&xml) else {
            return Self::default();
        };

        let mut sorted: Vec<&SongNoteData> = picospeaker_notes.iter().collect();
        sorted.sort_by(|a, b| a.time.total_cmp(&b.time));
        let mut timemap = Vec::new();
        for note in sorted {
            if rng.range_i32(0, 15) == 0 {
                let going_right = note.data != 2 && note.data != 3;
                timemap.push((note.time, going_right));
            }
        }

        Self {
            pub(super) enabled: true,
            is_erect,
            pub(super) z_index: 30,
            texture: Some(tex),
            tex_size,
            run_frames: sparrow.frames_by_prefix("tankman running").cloned().collect(),
            shot_frames: [
                sparrow.frames_by_prefix("John Shot 1").cloned().collect(),
                sparrow.frames_by_prefix("John Shot 2").cloned().collect(),
            ],
            timemap,
            next_spawn: 0,
            pool: std::array::from_fn(|_| TankRunner::default()),
        }
    }

    pub(super) fn reset(&mut self) {
        self.next_spawn = 0;
        for r in &mut self.pool {
            r.active = false;
        }
    }

    pub(super) fn update(&mut self, elapsed_s: f32, song_pos_ms: f64, rng: &mut ScriptRng) {
        if !self.enabled {
            return;
        }
        let cutoff = song_pos_ms + 3000.0;
        while self.next_spawn < self.timemap.len() && self.timemap[self.next_spawn].0 <= cutoff {
            let (strum_time, going_right) = self.timemap[self.next_spawn];
            self.next_spawn += 1;
            if let Some(slot) = self.pool.iter_mut().find(|r| !r.active) {
                *slot = TankRunner {
                    active: true,
                    visible: true,
                    strum_time_ms: strum_time,
                    going_right,
                    ending_offset: rng.range_f32(50.0, 200.0),
                    run_speed: rng.range_f32(0.6, 1.0),
                    scale: if self.is_erect { 1.10 } else { 1.0 },
                    x: 9999.0,
                    y: if self.is_erect {
                        350.0
                    } else {
                        (200 + rng.range_i32(50, 100)) as f32
                    },
                    shot: false,
                    shot_variant: (rng.next_u32() & 1) as usize,
                    anim_time_s: rng.range_f32(0.0, 0.5),
                    flicker_timer_s: 0.0,
                    flickering: false,
                };
            }
        }

        for r in &mut self.pool {
            if !r.active {
                continue;
            }
            r.anim_time_s += elapsed_s.max(0.0);
            if !r.shot && song_pos_ms >= r.strum_time_ms {
                r.shot = true;
                r.anim_time_s = 0.0;
            }
            if !r.shot {
                let run_factor = ((song_pos_ms - r.strum_time_ms) as f32) * r.run_speed;
                r.x = if !r.going_right {
                    (1280.0 * 0.02 - r.ending_offset) + run_factor
                } else {
                    (1280.0 * 0.74 + r.ending_offset) - run_factor
                };
            } else {
                let frame_num = (r.anim_time_s * 24.0).floor() as usize;
                if frame_num >= 10 {
                    r.flickering = true;
                }
                if r.flickering {
                    r.flicker_timer_s += elapsed_s.max(0.0);
                    if r.flicker_timer_s < 0.3 {
                        let flick = (r.flicker_timer_s * 10.0) as i32;
                        r.visible = (flick & 1) == 0;
                    } else {
                        let flick = ((r.flicker_timer_s - 0.3) * 20.0) as i32;
                        r.visible = (flick & 1) == 0;
                    }
                    if r.flicker_timer_s >= 0.6 {
                        r.active = false;
                    }
                }
            }
        }
    }

    pub(super) fn draw(&self, batcher: &mut SpriteBatcher, cam_pos: [f32; 2], cam_zoom: f32) {
        if !self.enabled {
            return;
        }
        let Some(tex) = self.texture else {
            return;
        };
        const ORIGIN: [f32; 2] = [252.5, 239.0];
        for r in &self.pool {
            if !r.active || !r.visible {
                continue;
            }
            let frames = if r.shot {
                &self.shot_frames[r.shot_variant]
            } else {
                &self.run_frames
            };
            if frames.is_empty() {
                continue;
            }
            let idx = (r.anim_time_s * 24.0).floor() as usize;
            let frame = if r.shot {
                &frames[idx.min(frames.len() - 1)]
            } else {
                &frames[idx % frames.len()]
            };
            let src = frame.source_size();
            let trimmed = frame.upright_trimmed_size();
            let flip_x = !r.going_right;
            let trim = compute_flipped_trim_offset(
                src,
                trimmed,
                [frame.frame_x, frame.frame_y],
                flip_x,
                false,
            );
            let offset = if r.shot {
                [300.0, 200.0]
            } else {
                [151.5, 143.4]
            };
            let draw_x =
                r.x - offset[0] + ORIGIN[0] * (1.0 - r.scale) + trim[0] * r.scale;
            let draw_y =
                r.y - offset[1] + ORIGIN[1] * (1.0 - r.scale) + trim[1] * r.scale;
            let (pos, size) = apply_camera_transform(
                [draw_x, draw_y],
                [trimmed[0] * r.scale, trimmed[1] * r.scale],
                [1.0, 1.0],
                cam_pos,
                cam_zoom,
            );
            let mut flags = 0u32;
            if frame.rotated {
                flags |= SPRITE_FLAG_ROTATED_90_CCW;
            }
            if flip_x ^ frame.flip_x {
                flags |= SPRITE_FLAG_FLIP_X;
            }
            let mut inst = SpriteInstance::new(pos, size)
                .with_uv_rect(frame.uv_bounds_inset(self.tex_size[0], self.tex_size[1], 1.0))
                .with_flags(flags);
            if self.is_erect {
                inst = inst.with_adjust_color(-38.0, -20.0, -46.0, -25.0);
            }
            batcher.push(tex, BlendMode::Normal, FilterMode::Linear, inst);
        }
    }
}
