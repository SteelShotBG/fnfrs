use std::f32::consts::PI;

use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{AnimateAtlas, BlendMode, EvaluatedPart, GpuContext, SpriteBatcher, TextureHandle};

use crate::character::{draw_animate_parts, load_animate_source, AtlasDrawParams, LoadedAtlasSource};
use crate::stage::Stage;
#[derive(Debug, Clone, Default)]
pub(crate) struct QuadCarTween {
    pub(super) active: bool,
    pub(super) elapsed_s: f32,
    pub(super) duration_s: f32,
    pub(super) start_delay_s: f32,
    pub(super) p0: [f32; 2],
    pub(super) p1: [f32; 2],
    pub(super) p2: [f32; 2],
    pub(super) rot_start: f32,
    pub(super) rot_end: f32,
    pub(super) stopping_at_lights: bool,
}

impl QuadCarTween {
    pub(super) fn eval(&mut self, elapsed_s: f32, sprite_x: &mut f32, sprite_y: &mut f32, sprite_angle: &mut f32) -> (bool, bool) {
        // Returns (still_active, finished_this_frame)
        if !self.active {
            return (false, false);
        }
        self.elapsed_s += elapsed_s.max(0.0);
        if self.elapsed_s < self.start_delay_s {
            return (true, false);
        }
        let active_t = self.elapsed_s - self.start_delay_s;
        let raw_t = (active_t / self.duration_s.max(0.01)).clamp(0.0, 1.0);
        // cubeOut when stopping at lights, sineIn when pulling away, linear otherwise
        let t = if self.stopping_at_lights {
            let u = 1.0 - raw_t;
            1.0 - u * u * u
        } else if self.start_delay_s > 0.0 {
            1.0 - (raw_t * PI * 0.5).cos()
        } else {
            raw_t
        };
        let u = 1.0 - t;
        *sprite_x = u * u * self.p0[0] + 2.0 * u * t * self.p1[0] + t * t * self.p2[0];
        *sprite_y = u * u * self.p0[1] + 2.0 * u * t * self.p1[1] + t * t * self.p2[1];
        *sprite_angle = self.rot_start + (self.rot_end - self.rot_start) * t;
        if raw_t >= 1.0 {
            self.active = false;
            return (false, true);
        }
        (true, false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CanState {
    #[default]
    Inactive,
    Arcing,
    Shot,
    HitPico,
}

#[derive(Debug, Clone, Copy, Default)]
struct SpraycanSlot {
    state: CanState,
    pos: [f32; 2],
    anim_time_s: f32,
}

#[derive(Debug)]
pub(super) struct SpraycanPool {
    pub(super) enabled: bool,
    atlas: Option<AnimateAtlas>,
    textures: Vec<(TextureHandle, [f32; 2])>,
    bounds_origin: [f32; 2],
    bounds_size: [f32; 2],
    parts_bufs: [Vec<EvaluatedPart>; 4],
    slots: [SpraycanSlot; 4],
    pub(super) gun_cocked_timer_s: f32,
    darken_timer_s: Option<f32>,
}

impl Default for SpraycanPool {
    fn default() -> Self {
        Self {
            enabled: false,
            atlas: None,
            textures: Vec::new(),
            bounds_origin: [0.0, 0.0],
            bounds_size: [1.0, 1.0],
            parts_bufs: Default::default(),
            slots: [SpraycanSlot::default(); 4],
            gun_cocked_timer_s: 0.0,
            darken_timer_s: None,
        }
    }
}

impl SpraycanPool {
    pub(super) fn load(song_id: &str, assets: &AssetLocator, gpu: &mut GpuContext) -> Self {
        if !song_id.eq_ignore_ascii_case("2hot") {
            return Self::default();
        }
        let Some(LoadedAtlasSource::Animate {
            atlas,
            textures,
            bounds_origin,
            bounds_size,
            ..
        }) = load_animate_source(assets, gpu, "spraycanAtlas", None)
        else {
            return Self::default();
        };
        Self {
            enabled: true,
            atlas: Some(atlas),
            textures,
            bounds_origin,
            bounds_size,
            parts_bufs: [
                Vec::with_capacity(24),
                Vec::with_capacity(24),
                Vec::with_capacity(24),
                Vec::with_capacity(24),
            ],
            slots: [SpraycanSlot::default(); 4],
            gun_cocked_timer_s: 0.0,
            darken_timer_s: None,
        }
    }

    pub(super) fn reset(&mut self) {
        self.slots = [SpraycanSlot::default(); 4];
        self.gun_cocked_timer_s = 0.0;
        self.darken_timer_s = None;
    }

    pub(super) fn spawn(&mut self, stage: Option<&Stage>) {
        if !self.enabled {
            return;
        }
        let pile_pos = stage
            .and_then(|st| st.get_named_prop("spraycanPile"))
            .map_or([890.0, 740.0], |p| p.position);
        for slot in &mut self.slots {
            if slot.state == CanState::Inactive {
                slot.state = CanState::Arcing;
                slot.pos = [pile_pos[0] - 10.0, pile_pos[1] - 550.0];
                slot.anim_time_s = 0.0;
                break;
            }
        }
    }

    pub(super) fn shoot(&mut self) -> bool {
        if !self.enabled || self.gun_cocked_timer_s <= 0.0 {
            return false;
        }
        self.gun_cocked_timer_s = 0.0;
        for slot in &mut self.slots {
            if slot.state == CanState::Arcing {
                slot.state = CanState::Shot;
                slot.anim_time_s = 0.0;
                self.darken_timer_s = Some(0.0);
                return true;
            }
        }
        false
    }

    pub(super) fn update(&mut self, elapsed_s: f32, mut stage: Option<&mut Stage>) {
        if !self.enabled {
            return;
        }
        let dt = elapsed_s.max(0.0);
        if self.gun_cocked_timer_s > 0.0 {
            self.gun_cocked_timer_s = (self.gun_cocked_timer_s - dt).max(0.0);
        }
        let Some(atlas) = &self.atlas else {
            return;
        };
        let fps = atlas.frame_rate().max(1.0);
        for (idx, slot) in self.slots.iter_mut().enumerate() {
            if slot.state == CanState::Inactive {
                self.parts_bufs[idx].clear();
                continue;
            }
            slot.anim_time_s += dt;
            let label = match slot.state {
                CanState::Inactive => continue,
                CanState::Arcing => "Can Start",
                CanState::Shot => "Can Shot",
                CanState::HitPico => "Hit Pico",
            };
            let dur = atlas.label_span(label).map_or(1, |s| s.duration.max(1));
            let frame = (slot.anim_time_s * fps).floor() as u32;
            if frame >= dur {
                if slot.state == CanState::Arcing {
                    slot.state = CanState::HitPico;
                    slot.anim_time_s = 0.0;
                    atlas.evaluate_label("Hit Pico", 0, &mut self.parts_bufs[idx]);
                } else {
                    slot.state = CanState::Inactive;
                    self.parts_bufs[idx].clear();
                }
            } else {
                atlas.evaluate_label(label, frame, &mut self.parts_bufs[idx]);
            }
        }

        if let Some(t) = &mut self.darken_timer_s {
            *t += dt;
            let c = if *t < 1.0 / 24.0 {
                0.067
            } else {
                let u = ((*t - 1.0 / 24.0) / 1.4).clamp(0.0, 1.0);
                0.133 + (1.0 - 0.133) * u
            };
            if let Some(st) = stage.as_deref_mut() {
                for prop in &mut st.props {
                    prop.color = [c, c, c, prop.color[3]];
                }
            }
            if *t >= 1.45 {
                if let Some(st) = stage {
                    for prop in &mut st.props {
                        prop.color = [1.0, 1.0, 1.0, prop.color[3]];
                    }
                }
                self.darken_timer_s = None;
            }
        }
    }

    pub(super) fn draw(&self, batcher: &mut SpriteBatcher, cam_pos: [f32; 2], cam_zoom: f32) {
        if !self.enabled || self.atlas.is_none() {
            return;
        }
        for (idx, slot) in self.slots.iter().enumerate() {
            if slot.state == CanState::Inactive || self.parts_bufs[idx].is_empty() {
                continue;
            }
            let params = AtlasDrawParams {
                pos: slot.pos,
                scale: [1.0, 1.0],
                anim_offsets: [0.0, 0.0],
                global_offsets: [0.0, 0.0],
                scroll_factor: [1.0, 1.0],
                cam_pos,
                cam_zoom,
                angle_deg: 0.0,
                flip_x: false,
                flip_y: false,
                color: [1.0, 1.0, 1.0, 1.0],
                alpha: 1.0,
                blend: BlendMode::Normal,
                is_pixel: false,
                adjust_color: None,
                wiggle: None,
                ref_unscaled_size: None,
            };
            draw_animate_parts(
                batcher,
                &self.textures,
                &self.parts_bufs[idx],
                self.bounds_origin,
                self.bounds_size,
                None,
                &params,
                false,
                false,
            );
        }
    }
}
