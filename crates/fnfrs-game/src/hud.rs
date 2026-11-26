//! V-Slice HUD components and gameplay substates:
//! - [`HealthIcon`] and [`HealthBarLayout`] (`HealthIcon.hx`, `PlayState.hx`)
//! - [`PauseMenuState`] (`funkin/play/PauseSubState.hx`)
//! - [`GameOverState`] (`funkin/play/GameOverSubState.hx`)
//! - [`ResultsSummaryState`] (`funkin/play/ResultState.hx`)
//!
//! All per-frame `update` and `draw` paths are strictly zero-allocation.

use std::fmt::Write as _;
use std::fs;

use fnfrs_core::constants::{
    DEFAULT_DIFFICULTY, DEFAULT_HEALTH_ICON, HEALTH_ICON_ANGLE_HALF_LIFE, HEALTH_ICON_BOP_SCALE,
    HEALTH_ICON_SIZE, HEALTH_MAX, STEPS_PER_BEAT, VIRTUAL_HEIGHT, VIRTUAL_WIDTH,
};
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::character::HealthIconData;
use fnfrs_core::scoring::{
    calculate_rank, tally_completion, ScoreTallies, ScoringRank,
};
use fnfrs_render::{
    AnimateAtlas, AtlasFrame, BitmapFont, BlendMode, EvaluatedPart, FilterMode, GpuContext,
    SparrowAtlas, SpriteBatcher, SpriteInstance, StackTextBuffer, TextAlign, TextStyle,
    TextureHandle, SPRITE_FLAG_AFFINE, SPRITE_FLAG_FLIP_X, SPRITE_FLAG_FLIP_Y,
    SPRITE_FLAG_PIXEL_SNAP, SPRITE_FLAG_ROTATED_90_CCW,
};
use winit::keyboard::KeyCode;

/// Horizontal overlap of each icon across the health bar split (`HealthIcon.POSITION_OFFSET`).
pub const HEALTH_ICON_OVERLAP: f32 = 26.0;

/// Health ratio strictly below which the losing icon frame/animation plays (`< 20%`).
pub const HEALTH_LOSING_RATIO: f32 = 0.2;

/// Health ratio strictly above which the winning icon frame/animation plays (`> 80%`).
pub const HEALTH_WINNING_RATIO: f32 = 0.8;

/// Exponential decay rate (1/s) returning [`HealthIcon::bop`] scale `1.2 -> 1.0`.
const ICON_BOP_DECAY_RATE: f32 = 18.0;

/// Animation state for Sparrow-backed health icons (`HealthIconState` in `HealthIcon.hx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HealthIconState {
    #[default]
    Idle,
    Winning,
    Losing,
    ToWinning,
    ToLosing,
    FromWinning,
    FromLosing,
}

/// Geometry of the V-Slice health bar on the `1280x720` virtual canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HealthBarLayout {
    pub bg_x: f32,
    pub bg_y: f32,
    pub bg_w: f32,
    pub bg_h: f32,
    pub bar_x: f32,
    pub bar_y: f32,
    pub bar_w: f32,
    pub bar_h: f32,
    /// Player health ratio clamped to `[0.0, 1.0]`.
    pub player_ratio: f32,
    /// Virtual X coordinate of the opponent/player fill boundary.
    pub split_x: f32,
}

impl HealthBarLayout {
    /// Computes health bar geometry and `split_x` matching `PlayState.initHealthBar`.
    #[must_use]
    pub fn compute(bg_w: f32, bg_h: f32, is_downscroll: bool, health_lerp: f32) -> Self {
        let safe_bg_w = bg_w.max(10.0);
        let safe_bg_h = bg_h.max(10.0);
        let bg_x = (VIRTUAL_WIDTH - safe_bg_w) * 0.5;
        let bg_y = if is_downscroll {
            VIRTUAL_HEIGHT * 0.1
        } else {
            VIRTUAL_HEIGHT * 0.9
        };
        let bar_x = bg_x + 4.0;
        let bar_y = bg_y + 4.0;
        let bar_w = (safe_bg_w - 8.0).max(1.0);
        let bar_h = (safe_bg_h - 8.0).max(1.0);
        let player_ratio = (health_lerp / HEALTH_MAX).clamp(0.0, 1.0);
        let split_x = bar_x + bar_w * (1.0 - player_ratio);

        Self {
            bg_x,
            bg_y,
            bg_w: safe_bg_w,
            bg_h: safe_bg_h,
            bar_x,
            bar_y,
            bar_w,
            bar_h,
            player_ratio,
            split_x,
        }
    }

    /// Computes the horizontal position for a health icon on `split_x`.
    #[inline]
    #[must_use]
    pub fn icon_x(&self, is_player: bool, display_width: f32) -> f32 {
        if is_player {
            self.split_x - HEALTH_ICON_OVERLAP
        } else {
            self.split_x - (display_width - HEALTH_ICON_OVERLAP)
        }
    }

    /// Computes the vertical position centering an icon on `healthBarBG`.
    #[inline]
    #[must_use]
    pub fn icon_y_centered(&self, display_height: f32) -> f32 {
        self.bg_y + (self.bg_h - display_height) * 0.5
    }
}

/// Pre-collected Sparrow animations for modern animated health icons.
#[derive(Debug, Clone, Default)]
pub struct HealthIconSparrowFrames {
    pub idle: Vec<AtlasFrame>,
    pub losing: Vec<AtlasFrame>,
    pub winning: Vec<AtlasFrame>,
    pub to_losing: Vec<AtlasFrame>,
    pub from_losing: Vec<AtlasFrame>,
    pub to_winning: Vec<AtlasFrame>,
    pub from_winning: Vec<AtlasFrame>,
}

/// Graphic backing for a [`HealthIcon`].
#[derive(Debug, Clone)]
pub enum HealthIconSheet {
    /// No texture loaded; draws a colored fallback square.
    Placeholder,
    /// Legacy square N-frame strip (`1`-frame, `2`-frame neutral+losing, or `3`-frame +winning).
    Strip {
        texture: TextureHandle,
        tex_size: [f32; 2],
        frame_count: u32,
        frame_size: [f32; 2],
    },
    /// Animated SparrowV2 sheet (`images/icons/icon-<id>.xml`).
    Sparrow {
        texture: TextureHandle,
        tex_size: [f32; 2],
        anims: HealthIconSparrowFrames,
    },
}

/// V-Slice health bar character icon (`HealthIcon.hx`).
#[derive(Debug, Clone)]
pub struct HealthIcon {
    pub character_id: String,
    pub is_player: bool,
    pub is_pixel: bool,
    pub should_bop: bool,
    pub auto_update: bool,
    pub bop_every: u32,
    pub bop_angle: f32,
    pub base_scale: f32,
    pub bop_scale_mult: f32,
    pub angle_deg: f32,
    pub flip_x: bool,
    pub offsets: [f32; 2],
    pub state: HealthIconState,
    pub current_frame_index: u32,
    pub x: f32,
    pub y: f32,
    pub sheet: HealthIconSheet,
    anim_frame_idx: usize,
    anim_timer_s: f32,
}

impl HealthIcon {
    /// Creates a headless placeholder icon (for tests or missing icon assets).
    #[must_use]
    pub fn placeholder(character_id: &str, is_player: bool, is_pixel: bool) -> Self {
        Self {
            character_id: character_id.to_owned(),
            is_player,
            is_pixel,
            should_bop: true,
            auto_update: true,
            bop_every: STEPS_PER_BEAT,
            bop_angle: 0.0,
            base_scale: 1.0,
            bop_scale_mult: 1.0,
            angle_deg: 0.0,
            flip_x: is_player,
            offsets: [0.0, 0.0],
            state: HealthIconState::Idle,
            current_frame_index: 0,
            x: 0.0,
            y: 0.0,
            sheet: HealthIconSheet::Placeholder,
            anim_frame_idx: 0,
            anim_timer_s: 0.0,
        }
    }

    /// Creates a strip-backed [`HealthIcon`] from an already uploaded GPU texture size.
    #[must_use]
    pub fn from_strip_texture(
        character_id: &str,
        is_player: bool,
        texture: TextureHandle,
        tex_w: u32,
        tex_h: u32,
        data: Option<&HealthIconData>,
        char_is_pixel: bool,
    ) -> Self {
        let mut icon = Self::placeholder(character_id, is_player, char_is_pixel);
        icon.apply_config(data, char_is_pixel);
        let (frame_count, frame_size) = detect_strip_layout(tex_w, tex_h);
        icon.sheet = HealthIconSheet::Strip {
            texture,
            tex_size: [tex_w as f32, tex_h as f32],
            frame_count,
            frame_size,
        };
        icon
    }

    /// Loads `images/icons/icon-<id>.png` (falling back to `icon-face.png`) and optional
    /// Sparrow XML `images/icons/icon-<id>.xml` from `assets`.
    pub fn load(
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        char_id: &str,
        is_player: bool,
        data: Option<&HealthIconData>,
        char_is_pixel: bool,
    ) -> Self {
        let requested_id = data
            .and_then(|d| d.id.as_deref())
            .filter(|s| !s.is_empty())
            .unwrap_or(char_id);

        let mut icon = Self::placeholder(requested_id, is_player, char_is_pixel);
        icon.apply_config(data, char_is_pixel);

        let mut resolved_id = requested_id;
        let mut key = format!("icons/icon-{resolved_id}");
        let mut png_path = assets.image(&key);

        if !png_path.is_file() {
            resolved_id = DEFAULT_HEALTH_ICON;
            key = format!("icons/icon-{resolved_id}");
            png_path = assets.image(&key);
            icon.character_id = resolved_id.to_owned();
        }

        let Ok(png_bytes) = fs::read(&png_path) else {
            return icon;
        };
        let Ok(texture) = gpu.from_png_bytes(Some(&key), &png_bytes) else {
            return icon;
        };
        let Some((tex_w, tex_h)) = gpu.texture_size(texture) else {
            return icon;
        };

        let xml_path = assets.sparrow_xml(&key);
        if xml_path.is_file() {
            if let Ok(xml_str) = fs::read_to_string(&xml_path) {
                if let Ok(atlas) = SparrowAtlas::from_xml(&xml_str) {
                    let anims = HealthIconSparrowFrames {
                        idle: collect_sparrow_prefix(&atlas, "idle"),
                        losing: collect_sparrow_prefix(&atlas, "losing"),
                        winning: collect_sparrow_prefix(&atlas, "winning"),
                        to_losing: collect_sparrow_prefix(&atlas, "toLosing"),
                        from_losing: collect_sparrow_prefix(&atlas, "fromLosing"),
                        to_winning: collect_sparrow_prefix(&atlas, "toWinning"),
                        from_winning: collect_sparrow_prefix(&atlas, "fromWinning"),
                    };
                    if !anims.idle.is_empty() {
                        icon.sheet = HealthIconSheet::Sparrow {
                            texture,
                            tex_size: [tex_w as f32, tex_h as f32],
                            anims,
                        };
                        return icon;
                    }
                }
            }
        }

        let (frame_count, frame_size) = detect_strip_layout(tex_w, tex_h);
        icon.sheet = HealthIconSheet::Strip {
            texture,
            tex_size: [tex_w as f32, tex_h as f32],
            frame_count,
            frame_size,
        };
        icon
    }

    /// Applies [`HealthIconData`] overrides matching `HealthIcon.configure` and
    /// `BaseCharacter.initHealthIcon`.
    pub fn apply_config(&mut self, data: Option<&HealthIconData>, char_is_pixel: bool) {
        match data {
            None => {
                self.is_pixel = char_is_pixel;
                self.should_bop = true;
                self.base_scale = 1.0;
                self.offsets = [0.0, 0.0];
                self.flip_x = self.is_player;
            }
            Some(cfg) => {
                if let Some(id) = cfg.id.as_deref().filter(|s| !s.is_empty()) {
                    self.character_id.clear();
                    self.character_id.push_str(id);
                }
                self.is_pixel = cfg.is_pixel.unwrap_or(char_is_pixel);
                self.should_bop = cfg.should_bop;
                self.base_scale = cfg.scale.max(0.05);
                self.offsets = cfg.offsets;
                self.flip_x = if self.is_player {
                    !cfg.flip_x
                } else {
                    cfg.flip_x
                };
            }
        }
    }

    /// Returns the number of frames available in a strip icon (`1`, `2`, `3`, etc.).
    #[must_use]
    pub fn strip_frame_count(&self) -> u32 {
        match &self.sheet {
            HealthIconSheet::Strip { frame_count, .. } => *frame_count,
            HealthIconSheet::Placeholder => 2,
            HealthIconSheet::Sparrow { anims, .. } => {
                if !anims.winning.is_empty() {
                    3
                } else if !anims.losing.is_empty() {
                    2
                } else {
                    1
                }
            }
        }
    }

    /// Selects the frame index (`0` = neutral, `1` = losing, `2` = winning) for a
    /// strip icon from normalized `health_ratio` in `[0.0, 1.0]`.
    ///
    /// Matches V-Slice `HealthIcon.hx`:
    /// - `< 20%` (`< 0.2`) => losing frame `1` (if `frame_count >= 2`)
    /// - `> 80%` (`> 0.8`) => winning frame `2` (if `frame_count >= 3`)
    /// - otherwise => neutral frame `0`
    #[inline]
    #[must_use]
    pub fn select_strip_frame(health_ratio: f32, frame_count: u32) -> u32 {
        if health_ratio < HEALTH_LOSING_RATIO {
            if frame_count >= 2 { 1 } else { 0 }
        } else if health_ratio > HEALTH_WINNING_RATIO {
            if frame_count >= 3 { 2 } else { 0 }
        } else {
            0
        }
    }

    /// Triggers a beat bop (`1.2 -> 1.0` scale boost) if `should_bop` is enabled.
    #[inline]
    pub fn bop(&mut self) {
        if !self.should_bop {
            return;
        }
        self.bop_scale_mult = 1.0 + HEALTH_ICON_BOP_SCALE;
        let sign = if self.is_player { 1.0 } else { -1.0 };
        self.angle_deg += self.bop_angle * sign;
    }

    /// Triggers [`Self::bop`] when `cur_step` aligns with `bop_every` (`HealthIcon.onStepHit`).
    #[inline]
    pub fn on_step_hit(&mut self, cur_step: i32) {
        if self.bop_every != 0 && cur_step.rem_euclid(self.bop_every as i32) == 0 {
            self.bop();
        }
    }

    /// Returns the drawn width and height in virtual pixels at the current bop scale.
    #[must_use]
    pub fn display_size(&self) -> [f32; 2] {
        let target = HEALTH_ICON_SIZE * self.base_scale * self.bop_scale_mult;
        match &self.sheet {
            HealthIconSheet::Placeholder => [target, target],
            HealthIconSheet::Strip { frame_size, .. } => {
                let w = frame_size[0].max(1.0);
                let h = frame_size[1].max(1.0);
                let norm = if w >= h { target / w } else { target / h };
                [w * norm, h * norm]
            }
            HealthIconSheet::Sparrow { anims, .. } => {
                let frame = self.active_sparrow_frame(anims);
                let [w, h] = frame.map_or([HEALTH_ICON_SIZE, HEALTH_ICON_SIZE], |f| {
                    f.upright_trimmed_size()
                });
                let safe_w = w.max(1.0);
                let safe_h = h.max(1.0);
                let norm = if safe_w >= safe_h {
                    target / safe_w
                } else {
                    target / safe_h
                };
                [safe_w * norm, safe_h * norm]
            }
        }
    }

    /// Updates exponential bop decay, health-state animation/frame, and health-bar split
    /// position with zero heap allocations.
    pub fn update(
        &mut self,
        elapsed_s: f32,
        health: f32,
        bar: &HealthBarLayout,
    ) {
        let dt = elapsed_s.max(0.0);
        if self.bop_scale_mult > 1.0 {
            let decayed = 1.0 + (self.bop_scale_mult - 1.0) * (-ICON_BOP_DECAY_RATE * dt).exp();
            self.bop_scale_mult = if (decayed - 1.0).abs() < 1e-4 {
                1.0
            } else {
                decayed
            };
        }

        if self.bop_every != 0 && self.angle_deg.abs() > 1e-4 {
            self.angle_deg *= 0.5_f32.powf(dt / HEALTH_ICON_ANGLE_HALF_LIFE);
        } else if self.angle_deg.abs() <= 1e-4 {
            self.angle_deg = 0.0;
        }

        if self.auto_update {
            let icon_health = if self.is_player {
                health
            } else {
                HEALTH_MAX - health
            };
            let ratio = (icon_health / HEALTH_MAX).clamp(0.0, 1.0);
            self.update_health_state(ratio, dt);
        }

        let [disp_w, disp_h] = self.display_size();
        self.x = bar.icon_x(self.is_player, disp_w);
        self.y = bar.icon_y_centered(disp_h);
    }

    fn update_health_state(&mut self, health_ratio: f32, dt: f32) {
        match &self.sheet {
            HealthIconSheet::Placeholder => {
                self.current_frame_index = Self::select_strip_frame(health_ratio, 2);
                self.state = match self.current_frame_index {
                    1 => HealthIconState::Losing,
                    2 => HealthIconState::Winning,
                    _ => HealthIconState::Idle,
                };
            }
            HealthIconSheet::Strip { frame_count, .. } => {
                let idx = Self::select_strip_frame(health_ratio, *frame_count);
                self.current_frame_index = idx;
                self.state = match idx {
                    1 => HealthIconState::Losing,
                    2 => HealthIconState::Winning,
                    _ => HealthIconState::Idle,
                };
            }
            HealthIconSheet::Sparrow { anims, .. } => {
                const FPS: f32 = 24.0;
                self.anim_timer_s += dt;
                let frame_step = (self.anim_timer_s * FPS) as usize;
                if frame_step > 0 {
                    self.anim_timer_s -= frame_step as f32 / FPS;
                    self.anim_frame_idx += frame_step;
                }

                let frames_len = self.sparrow_anim_slice(anims, self.state).len();
                let anim_finished = frames_len == 0 || self.anim_frame_idx >= frames_len;

                let next_state = match self.state {
                    HealthIconState::Idle => {
                        if health_ratio < HEALTH_LOSING_RATIO {
                            if !anims.to_losing.is_empty() {
                                HealthIconState::ToLosing
                            } else if !anims.losing.is_empty() {
                                HealthIconState::Losing
                            } else {
                                HealthIconState::Idle
                            }
                        } else if health_ratio > HEALTH_WINNING_RATIO {
                            if !anims.to_winning.is_empty() {
                                HealthIconState::ToWinning
                            } else if !anims.winning.is_empty() {
                                HealthIconState::Winning
                            } else {
                                HealthIconState::Idle
                            }
                        } else {
                            HealthIconState::Idle
                        }
                    }
                    HealthIconState::Winning => {
                        if health_ratio < HEALTH_WINNING_RATIO {
                            if !anims.from_winning.is_empty() {
                                HealthIconState::FromWinning
                            } else {
                                HealthIconState::Idle
                            }
                        } else {
                            HealthIconState::Winning
                        }
                    }
                    HealthIconState::Losing => {
                        if health_ratio > HEALTH_LOSING_RATIO {
                            if !anims.from_losing.is_empty() {
                                HealthIconState::FromLosing
                            } else {
                                HealthIconState::Idle
                            }
                        } else {
                            HealthIconState::Losing
                        }
                    }
                    HealthIconState::ToLosing => {
                        if anim_finished {
                            if !anims.losing.is_empty() {
                                HealthIconState::Losing
                            } else {
                                HealthIconState::Idle
                            }
                        } else {
                            HealthIconState::ToLosing
                        }
                    }
                    HealthIconState::ToWinning => {
                        if anim_finished {
                            if !anims.winning.is_empty() {
                                HealthIconState::Winning
                            } else {
                                HealthIconState::Idle
                            }
                        } else {
                            HealthIconState::ToWinning
                        }
                    }
                    HealthIconState::FromLosing | HealthIconState::FromWinning => {
                        if anim_finished {
                            HealthIconState::Idle
                        } else {
                            self.state
                        }
                    }
                };

                if next_state != self.state {
                    self.state = next_state;
                    self.anim_frame_idx = 0;
                    self.anim_timer_s = 0.0;
                } else if matches!(
                    self.state,
                    HealthIconState::Idle | HealthIconState::Winning | HealthIconState::Losing
                ) && frames_len > 0
                {
                    self.anim_frame_idx %= frames_len;
                }
            }
        }
    }

    fn sparrow_anim_slice<'a>(
        &self,
        anims: &'a HealthIconSparrowFrames,
        state: HealthIconState,
    ) -> &'a [AtlasFrame] {
        match state {
            HealthIconState::Idle => &anims.idle,
            HealthIconState::Winning => {
                if anims.winning.is_empty() {
                    &anims.idle
                } else {
                    &anims.winning
                }
            }
            HealthIconState::Losing => {
                if anims.losing.is_empty() {
                    &anims.idle
                } else {
                    &anims.losing
                }
            }
            HealthIconState::ToWinning => &anims.to_winning,
            HealthIconState::ToLosing => &anims.to_losing,
            HealthIconState::FromWinning => &anims.from_winning,
            HealthIconState::FromLosing => &anims.from_losing,
        }
    }

    fn active_sparrow_frame<'a>(
        &self,
        anims: &'a HealthIconSparrowFrames,
    ) -> Option<&'a AtlasFrame> {
        let slice = self.sparrow_anim_slice(anims, self.state);
        if slice.is_empty() {
            anims.idle.first()
        } else {
            let idx = self.anim_frame_idx.min(slice.len() - 1);
            Some(&slice[idx])
        }
    }

    /// Queues this health icon into `batcher` with zero heap allocations.
    pub fn draw(&self, batcher: &mut SpriteBatcher, white_tex: TextureHandle) {
        let [disp_w, disp_h] = self.display_size();
        let draw_x = self.x + self.offsets[0];
        let draw_y = self.y + self.offsets[1];
        let filter = if self.is_pixel {
            FilterMode::Nearest
        } else {
            FilterMode::Linear
        };

        let mut base_flags = 0u32;
        if self.flip_x {
            base_flags |= SPRITE_FLAG_FLIP_X;
        }
        if self.is_pixel {
            base_flags |= SPRITE_FLAG_PIXEL_SNAP;
        }
        let rot_rad = self.angle_deg.to_radians();
        let origin = [disp_w * 0.5, disp_h * 0.5];

        match &self.sheet {
            HealthIconSheet::Placeholder => {
                let color = if self.is_player {
                    [0.25, 0.72, 1.0, 1.0]
                } else {
                    [0.72, 0.30, 0.92, 1.0]
                };
                batcher.push(
                    white_tex,
                    BlendMode::Normal,
                    FilterMode::Nearest,
                    SpriteInstance::new([draw_x + 25.0, draw_y + 25.0], [100.0, 100.0])
                        .with_color(color)
                        .with_rotation(rot_rad, [50.0, 50.0])
                        .with_flags(base_flags),
                );
            }
            HealthIconSheet::Strip {
                texture,
                tex_size,
                frame_count,
                ..
            } => {
                let count = (*frame_count).max(1);
                let idx = self.current_frame_index.min(count - 1);
                let inset_u = if !self.is_pixel && count > 1 && tex_size[0] > 0.0 {
                    0.5 / tex_size[0]
                } else {
                    0.0
                };
                let u0 = (idx as f32 / count as f32) + inset_u;
                let u1 = ((idx + 1) as f32 / count as f32) - inset_u;

                batcher.push(
                    *texture,
                    BlendMode::Normal,
                    filter,
                    SpriteInstance::new([draw_x, draw_y], [disp_w, disp_h])
                        .with_uv_rect([u0, 0.0, u1, 1.0])
                        .with_rotation(rot_rad, origin)
                        .with_flags(base_flags),
                );
            }
            HealthIconSheet::Sparrow {
                texture,
                tex_size,
                anims,
            } => {
                let Some(frame) = self.active_sparrow_frame(anims) else {
                    return;
                };
                let mut flags = base_flags;
                if frame.rotated {
                    flags |= SPRITE_FLAG_ROTATED_90_CCW;
                }
                if frame.flip_x {
                    flags ^= SPRITE_FLAG_FLIP_X;
                }
                if frame.flip_y {
                    flags |= SPRITE_FLAG_FLIP_Y;
                }
                let inset_px = if self.is_pixel { 0.0 } else { 0.5 };
                batcher.push(
                    *texture,
                    BlendMode::Normal,
                    filter,
                    SpriteInstance::new([draw_x, draw_y], [disp_w, disp_h])
                        .with_uv_rect(frame.uv_bounds_inset(tex_size[0], tex_size[1], inset_px))
                        .with_rotation(rot_rad, origin)
                        .with_flags(flags),
                );
            }
        }
    }
}

fn detect_strip_layout(tex_w: u32, tex_h: u32) -> (u32, [f32; 2]) {
    if tex_h == 0 || tex_w == 0 {
        return (1, [HEALTH_ICON_SIZE, HEALTH_ICON_SIZE]);
    }
    if tex_w >= tex_h {
        let count = (tex_w / tex_h).max(1);
        let fw = tex_w as f32 / count as f32;
        let fh = tex_h as f32;
        (count, [fw, fh])
    } else {
        (1, [tex_w as f32, tex_h as f32])
    }
}

fn collect_sparrow_prefix(atlas: &SparrowAtlas, prefix: &str) -> Vec<AtlasFrame> {
    atlas.frames_by_prefix(prefix).cloned().collect()
}

// ============================================================================
// Pause Menu Substate (`PauseSubState.hx`)
// ============================================================================

/// Active page inside [`PauseMenuState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PauseMenuMode {
    #[default]
    Main,
    Difficulty,
}

/// Standard V-Slice pause menu options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseMenuOption {
    Resume,
    RestartSong,
    ChangeDifficulty,
    TogglePractice,
    ToggleBotplay,
    ToggleDownscroll,
    Exit,
}

impl PauseMenuOption {
    /// Human-readable label shown on screen.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Resume => "Resume",
            Self::RestartSong => "Restart Song",
            Self::ChangeDifficulty => "Change Difficulty",
            Self::TogglePractice => "Toggle Practice Mode",
            Self::ToggleBotplay => "Toggle Botplay",
            Self::ToggleDownscroll => "Toggle Downscroll",
            Self::Exit => "Exit",
        }
    }
}

/// Discrete input snapshot consumed by [`PauseMenuState::update`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PauseMenuInput {
    pub up_pressed: bool,
    pub down_pressed: bool,
    pub accept_pressed: bool,
    pub back_pressed: bool,
}

impl PauseMenuInput {
    /// Maps a single key press event into a [`PauseMenuInput`].
    #[must_use]
    pub const fn from_key_code(code: KeyCode) -> Self {
        match code {
            KeyCode::ArrowUp | KeyCode::KeyW => Self {
                up_pressed: true,
                down_pressed: false,
                accept_pressed: false,
                back_pressed: false,
            },
            KeyCode::ArrowDown | KeyCode::KeyS => Self {
                up_pressed: false,
                down_pressed: true,
                accept_pressed: false,
                back_pressed: false,
            },
            KeyCode::Enter | KeyCode::Space => Self {
                up_pressed: false,
                down_pressed: false,
                accept_pressed: true,
                back_pressed: false,
            },
            KeyCode::Escape | KeyCode::Backspace => Self {
                up_pressed: false,
                down_pressed: false,
                accept_pressed: false,
                back_pressed: true,
            },
            _ => Self {
                up_pressed: false,
                down_pressed: false,
                accept_pressed: false,
                back_pressed: false,
            },
        }
    }
}

/// High-level action emitted by [`PauseMenuState::update`] when the player confirms an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseMenuAction {
    None,
    Resume,
    RestartSong,
    ChangeDifficulty(usize),
    TogglePractice,
    ToggleBotplay,
    ToggleDownscroll,
    Exit,
}

const MAX_PAUSE_MENU_ITEMS: usize = 8;

/// Zero-allocation V-Slice pause menu state (`PauseSubState.hx`).
#[derive(Debug, Clone)]
pub struct PauseMenuState {
    pub is_open: bool,
    pub mode: PauseMenuMode,
    pub selected: usize,
    pub bg_alpha: f32,
    pub metadata_alpha: f32,
    pub is_practice: bool,
    pub is_botplay: bool,
    pub is_downscroll: bool,
    pub death_count: u32,
    pub song_name: StackTextBuffer<64>,
    pub difficulty: StackTextBuffer<32>,
    difficulties: [StackTextBuffer<32>; MAX_PAUSE_MENU_ITEMS],
    difficulty_count: usize,
    item_x: [f32; MAX_PAUSE_MENU_ITEMS],
    item_y: [f32; MAX_PAUSE_MENU_ITEMS],
    just_opened: bool,
}

impl Default for PauseMenuState {
    fn default() -> Self {
        Self::new("Unknown Song", DEFAULT_DIFFICULTY)
    }
}

impl PauseMenuState {
    const MAIN_OPTIONS: [PauseMenuOption; 7] = [
        PauseMenuOption::Resume,
        PauseMenuOption::RestartSong,
        PauseMenuOption::ChangeDifficulty,
        PauseMenuOption::TogglePractice,
        PauseMenuOption::ToggleBotplay,
        PauseMenuOption::ToggleDownscroll,
        PauseMenuOption::Exit,
    ];

    /// Creates a new pause menu initialized for `song_name` and `difficulty`.
    #[must_use]
    pub fn new(song_name: &str, difficulty: &str) -> Self {
        let mut state = Self {
            is_open: false,
            mode: PauseMenuMode::Main,
            selected: 0,
            bg_alpha: 0.0,
            metadata_alpha: 0.0,
            is_practice: false,
            is_botplay: false,
            is_downscroll: false,
            death_count: 0,
            song_name: StackTextBuffer::new(),
            difficulty: StackTextBuffer::new(),
            difficulties: std::array::from_fn(|_| StackTextBuffer::new()),
            difficulty_count: 0,
            item_x: [90.0; MAX_PAUSE_MENU_ITEMS],
            item_y: [VIRTUAL_HEIGHT * 0.48; MAX_PAUSE_MENU_ITEMS],
            just_opened: false,
        };
        state.set_song_info(song_name, difficulty);
        state.set_difficulties(&["easy", "normal", "hard"]);
        state.snap_item_positions();
        state
    }

    /// Updates displayed song metadata with zero heap allocations.
    pub fn set_song_info(&mut self, song_name: &str, difficulty: &str) {
        self.song_name.clear();
        let _ = self.song_name.write_str(song_name);
        self.difficulty.clear();
        let _ = self.difficulty.write_str(difficulty);
    }

    /// Populates available difficulties for the `"Change Difficulty"` submenu.
    pub fn set_difficulties(&mut self, diffs: &[&str]) {
        // Reserve 1 slot for trailing "Back".
        let cap = (MAX_PAUSE_MENU_ITEMS - 1).min(diffs.len());
        self.difficulty_count = cap;
        for (i, d) in diffs.iter().take(cap).enumerate() {
            self.difficulties[i].clear();
            let _ = self.difficulties[i].write_str(d);
        }
    }

    /// Opens the pause menu and starts the fade-in transition (`PauseSubState.create`).
    pub fn open(&mut self, is_botplay: bool, is_downscroll: bool, death_count: u32) {
        self.is_open = true;
        self.mode = PauseMenuMode::Main;
        self.selected = 0;
        self.bg_alpha = 0.0;
        self.metadata_alpha = 0.0;
        self.is_botplay = is_botplay;
        self.is_downscroll = is_downscroll;
        self.death_count = death_count;
        self.just_opened = true;
        self.snap_item_positions();
    }

    /// Closes the pause menu.
    pub fn close(&mut self) {
        self.is_open = false;
        self.just_opened = false;
    }

    /// Number of visible entries in the current [`PauseMenuMode`].
    #[must_use]
    pub fn item_count(&self) -> usize {
        match self.mode {
            PauseMenuMode::Main => Self::MAIN_OPTIONS.len(),
            PauseMenuMode::Difficulty => self.difficulty_count + 1,
        }
    }

    /// Returns the main-menu option at `self.selected` when in [`PauseMenuMode::Main`].
    #[must_use]
    pub fn selected_main_option(&self) -> Option<PauseMenuOption> {
        if self.mode == PauseMenuMode::Main {
            Self::MAIN_OPTIONS.get(self.selected).copied()
        } else {
            None
        }
    }

    /// Returns the difficulty string at index `idx` in [`PauseMenuMode::Difficulty`].
    #[must_use]
    pub fn difficulty_label(&self, idx: usize) -> Option<&str> {
        if idx < self.difficulty_count {
            Some(self.difficulties[idx].as_str())
        } else {
            None
        }
    }

    /// Moves selection cursor by `delta` with V-Slice wrap-around (`changeSelection`).
    pub fn move_selection(&mut self, delta: i32) {
        let len = self.item_count() as isize;
        if len <= 0 {
            self.selected = 0;
            return;
        }
        let next = (self.selected as isize + delta as isize).rem_euclid(len);
        self.selected = next as usize;
    }

    /// Advances animations and processes menu navigation with zero heap allocations.
    pub fn update(&mut self, elapsed_s: f32, input: PauseMenuInput) -> PauseMenuAction {
        if !self.is_open {
            return PauseMenuAction::None;
        }

        let dt = elapsed_s.max(0.0);
        // Background dims smoothly toward 0.6 (`PauseSubState.transitionIn`).
        let alpha_t = 1.0 - (-4.5 * dt).exp();
        self.bg_alpha += (0.6 - self.bg_alpha) * alpha_t;
        self.metadata_alpha += (1.0 - self.metadata_alpha) * alpha_t;

        // Smooth diagonal menu cascade (`PauseSubState.changeSelection`).
        let item_t = 1.0 - (-12.0 * dt).exp();
        let count = self.item_count().min(MAX_PAUSE_MENU_ITEMS);
        for i in 0..count {
            let rel = i as f32 - self.selected as f32;
            let target_x = rel * 26.0 + 90.0;
            let target_y = rel * 110.0 + VIRTUAL_HEIGHT * 0.48;
            self.item_x[i] += (target_x - self.item_x[i]) * item_t;
            self.item_y[i] += (target_y - self.item_y[i]) * item_t;
        }

        if input.up_pressed {
            self.move_selection(-1);
        }
        if input.down_pressed {
            self.move_selection(1);
        }

        // Guard against closing on the exact frame the pause key opened the menu.
        if self.just_opened {
            self.just_opened = false;
            return PauseMenuAction::None;
        }

        if input.back_pressed {
            if self.mode == PauseMenuMode::Difficulty {
                self.mode = PauseMenuMode::Main;
                self.selected = 0;
                self.snap_item_positions();
                return PauseMenuAction::None;
            }
            self.close();
            return PauseMenuAction::Resume;
        }

        if input.accept_pressed {
            return self.activate_selected();
        }

        PauseMenuAction::None
    }

    fn activate_selected(&mut self) -> PauseMenuAction {
        match self.mode {
            PauseMenuMode::Main => {
                let Some(opt) = self.selected_main_option() else {
                    return PauseMenuAction::None;
                };
                match opt {
                    PauseMenuOption::Resume => {
                        self.close();
                        PauseMenuAction::Resume
                    }
                    PauseMenuOption::RestartSong => {
                        self.close();
                        PauseMenuAction::RestartSong
                    }
                    PauseMenuOption::ChangeDifficulty => {
                        self.mode = PauseMenuMode::Difficulty;
                        self.selected = 0;
                        self.snap_item_positions();
                        PauseMenuAction::None
                    }
                    PauseMenuOption::TogglePractice => {
                        self.is_practice = !self.is_practice;
                        PauseMenuAction::TogglePractice
                    }
                    PauseMenuOption::ToggleBotplay => {
                        self.is_botplay = !self.is_botplay;
                        PauseMenuAction::ToggleBotplay
                    }
                    PauseMenuOption::ToggleDownscroll => {
                        self.is_downscroll = !self.is_downscroll;
                        PauseMenuAction::ToggleDownscroll
                    }
                    PauseMenuOption::Exit => {
                        self.close();
                        PauseMenuAction::Exit
                    }
                }
            }
            PauseMenuMode::Difficulty => {
                if self.selected >= self.difficulty_count {
                    self.mode = PauseMenuMode::Main;
                    self.selected = 0;
                    self.snap_item_positions();
                    PauseMenuAction::None
                } else {
                    let chosen = self.selected;
                    if chosen < self.difficulty_count {
                        self.difficulty.clear();
                        let _ = self.difficulty.write_str(self.difficulties[chosen].as_str());
                    }
                    self.close();
                    PauseMenuAction::ChangeDifficulty(chosen)
                }
            }
        }
    }

    fn snap_item_positions(&mut self) {
        let count = self.item_count().min(MAX_PAUSE_MENU_ITEMS);
        for i in 0..count {
            let rel = i as f32 - self.selected as f32;
            self.item_x[i] = rel * 26.0 + 90.0;
            self.item_y[i] = rel * 110.0 + VIRTUAL_HEIGHT * 0.48;
        }
    }

    /// Renders the dimmed overlay, scrolling option entries, and top-right metadata with
    /// zero heap allocations.
    pub fn draw(
        &self,
        batcher: &mut SpriteBatcher,
        white_tex: TextureHandle,
        font: &BitmapFont,
        font_tex: TextureHandle,
    ) {
        if !self.is_open {
            return;
        }

        // 1. Semi-transparent black overlay.
        batcher.push(
            white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                .with_color([0.0, 0.0, 0.0, self.bg_alpha]),
        );

        let is_small_font = font.line_height() <= 10.0;
        let entry_scale = if is_small_font { 4.0 } else { 2.0 };
        let meta_scale = if is_small_font { 2.0 } else { 1.1 };
        let mut line_buf = StackTextBuffer::<64>::new();

        // 2. Menu options.
        let count = self.item_count().min(MAX_PAUSE_MENU_ITEMS);
        for i in 0..count {
            let is_cur = i == self.selected;
            let alpha = if is_cur { 1.0 } else { 0.55 };
            let color = if is_cur {
                [1.0, 0.95, 0.45, alpha]
            } else {
                [1.0, 1.0, 1.0, alpha]
            };

            line_buf.clear();
            match self.mode {
                PauseMenuMode::Main => {
                    let opt = Self::MAIN_OPTIONS[i];
                    match opt {
                        PauseMenuOption::TogglePractice => {
                            let state_str = if self.is_practice { "ON" } else { "OFF" };
                            let _ = write!(line_buf, "Practice Mode: {state_str}");
                        }
                        PauseMenuOption::ToggleBotplay => {
                            let state_str = if self.is_botplay { "ON" } else { "OFF" };
                            let _ = write!(line_buf, "Botplay: {state_str}");
                        }
                        PauseMenuOption::ToggleDownscroll => {
                            let state_str = if self.is_downscroll { "ON" } else { "OFF" };
                            let _ = write!(line_buf, "Downscroll: {state_str}");
                        }
                        _ => {
                            let _ = line_buf.write_str(opt.label());
                        }
                    }
                }
                PauseMenuMode::Difficulty => {
                    if i < self.difficulty_count {
                        let _ = line_buf.write_str(self.difficulties[i].as_str());
                    } else {
                        let _ = line_buf.write_str("Back");
                    }
                }
            }

            let style = TextStyle {
                scale: entry_scale,
                color,
                align: TextAlign::Left,
                ..Default::default()
            };
            font.draw_bytes_outlined(
                batcher,
                font_tex,
                line_buf.as_bytes(),
                [self.item_x[i].round(), self.item_y[i].round()],
                &style,
                [0.0, 0.0, 0.0, alpha],
            );
        }

        // 3. Top-right metadata (`PauseSubState.buildMetadata`).
        let meta_x = VIRTUAL_WIDTH - 32.0;
        let meta_style = TextStyle {
            scale: meta_scale,
            color: [1.0, 1.0, 1.0, self.metadata_alpha],
            align: TextAlign::Right,
            ..Default::default()
        };

        font.draw_bytes_outlined(
            batcher,
            font_tex,
            self.song_name.as_bytes(),
            [meta_x, 22.0],
            &meta_style,
            [0.0, 0.0, 0.0, self.metadata_alpha],
        );

        line_buf.clear();
        let _ = write!(line_buf, "Difficulty: {}", self.difficulty.as_str());
        font.draw_bytes_outlined(
            batcher,
            font_tex,
            line_buf.as_bytes(),
            [meta_x, 56.0],
            &meta_style,
            [0.0, 0.0, 0.0, self.metadata_alpha],
        );

        line_buf.clear();
        let _ = write!(line_buf, "{} Blue Balls", self.death_count);
        font.draw_bytes_outlined(
            batcher,
            font_tex,
            line_buf.as_bytes(),
            [meta_x, 90.0],
            &meta_style,
            [0.0, 0.0, 0.0, self.metadata_alpha],
        );
    }
}

// ============================================================================
// Game Over Substate (`GameOverSubState.hx`)
// ============================================================================

/// Current phase of [`GameOverState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GameOverPhase {
    #[default]
    Inactive,
    /// Playing `firstDeath` anim + `fnf_loss_sfx`.
    FirstDeath,
    /// Playing `deathLoop` anim + `gameOver` looping music.
    DeathLoop,
    /// Confirmed retry; playing `deathConfirm` + `gameOverEnd` and fading to black.
    ConfirmRetry,
    /// Exiting back to menu.
    Exiting,
}

/// Audio cue emitted once on state transitions inside [`GameOverState::update`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameOverAudioCue {
    PlayLossSfx,
    StartGameOverLoop,
    PlayGameOverEnd,
    StopMusic,
}

/// High-level state transition requested by [`GameOverState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GameOverAction {
    #[default]
    None,
    RetrySong,
    ExitToMenu,
}

/// Per-frame return value from [`GameOverState::update`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GameOverEvents {
    pub audio_cue: Option<GameOverAudioCue>,
    pub action: GameOverAction,
}

/// Zero-allocation game-over substate (`GameOverSubState.hx`).
#[derive(Debug, Clone)]
pub struct GameOverState {
    pub phase: GameOverPhase,
    pub timer_s: f32,
    pub first_death_duration_s: f32,
    pub input_unlock_delay_s: f32,
    pub confirm_fade_duration_s: f32,
    pub can_input: bool,
    pub fade_alpha: f32,
    pub death_count: u32,
    pub is_pixel: bool,
    pub suffix: StackTextBuffer<32>,
}

impl Default for GameOverState {
    fn default() -> Self {
        Self::new()
    }
}

impl GameOverState {
    /// Creates an inactive [`GameOverState`] with V-Slice timing defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            phase: GameOverPhase::Inactive,
            timer_s: 0.0,
            first_death_duration_s: 2.0,
            input_unlock_delay_s: 1.0,
            confirm_fade_duration_s: 2.0,
            can_input: false,
            fade_alpha: 0.0,
            death_count: 0,
            is_pixel: false,
            suffix: StackTextBuffer::new(),
        }
    }

    /// Configures character-specific death audio suffix (e.g. `"-pixel"`, `"-pico"`).
    pub fn set_suffix(&mut self, suffix: &str, is_pixel: bool) {
        self.suffix.clear();
        let _ = self.suffix.write_str(suffix);
        self.is_pixel = is_pixel;
    }

    /// Enters game-over (`FirstDeath`) and emits [`GameOverAudioCue::PlayLossSfx`].
    pub fn trigger_death(&mut self) -> GameOverEvents {
        self.phase = GameOverPhase::FirstDeath;
        self.timer_s = 0.0;
        self.can_input = false;
        self.fade_alpha = 0.0;
        self.death_count = self.death_count.saturating_add(1);
        GameOverEvents {
            audio_cue: Some(GameOverAudioCue::PlayLossSfx),
            action: GameOverAction::None,
        }
    }

    /// Resets back to [`GameOverPhase::Inactive`] (e.g. when restarting song).
    pub fn reset(&mut self) {
        self.phase = GameOverPhase::Inactive;
        self.timer_s = 0.0;
        self.can_input = false;
        self.fade_alpha = 0.0;
    }

    /// Returns the V-Slice sound key for the loss SFX (`gameplay/gameover/fnf_loss_sfx<suffix>`).
    #[must_use]
    pub fn loss_sfx_key<'a>(&self, buf: &'a mut StackTextBuffer<64>) -> &'a str {
        buf.clear();
        let _ = write!(buf, "gameplay/gameover/fnf_loss_sfx{}", self.suffix.as_str());
        buf.as_str()
    }

    /// Returns the V-Slice music key for the loop (`gameplay/gameover/gameOver<suffix>`).
    #[must_use]
    pub fn loop_music_key<'a>(&self, buf: &'a mut StackTextBuffer<64>) -> &'a str {
        buf.clear();
        let _ = write!(buf, "gameplay/gameover/gameOver{}", self.suffix.as_str());
        buf.as_str()
    }

    /// Returns the V-Slice music key for retry confirm (`gameplay/gameover/gameOverEnd<suffix>`).
    #[must_use]
    pub fn end_music_key<'a>(&self, buf: &'a mut StackTextBuffer<64>) -> &'a str {
        buf.clear();
        let _ = write!(buf, "gameplay/gameover/gameOverEnd{}", self.suffix.as_str());
        buf.as_str()
    }

    /// Advances game-over timers, audio cues, and retry/exit handling with zero allocations.
    pub fn update(
        &mut self,
        elapsed_s: f32,
        accept_pressed: bool,
        back_pressed: bool,
    ) -> GameOverEvents {
        let mut events = GameOverEvents::default();
        if self.phase == GameOverPhase::Inactive || self.phase == GameOverPhase::Exiting {
            return events;
        }

        let dt = elapsed_s.max(0.0);
        self.timer_s += dt;

        match self.phase {
            GameOverPhase::FirstDeath => {
                if self.timer_s >= self.input_unlock_delay_s {
                    self.can_input = true;
                }

                if self.can_input && accept_pressed {
                    self.begin_confirm(&mut events);
                } else if back_pressed {
                    self.begin_exit(&mut events);
                } else if self.timer_s >= self.first_death_duration_s {
                    self.phase = GameOverPhase::DeathLoop;
                    self.can_input = true;
                    events.audio_cue = Some(GameOverAudioCue::StartGameOverLoop);
                }
            }
            GameOverPhase::DeathLoop => {
                self.can_input = true;
                if accept_pressed {
                    self.begin_confirm(&mut events);
                } else if back_pressed {
                    self.begin_exit(&mut events);
                }
            }
            GameOverPhase::ConfirmRetry => {
                let dur = self.confirm_fade_duration_s.max(0.1);
                let progress = (self.timer_s / dur).clamp(0.0, 1.0);
                self.fade_alpha = if self.is_pixel {
                    (progress * 10.0).floor() / 10.0
                } else {
                    progress
                };
                if self.timer_s >= dur {
                    self.reset();
                    events.action = GameOverAction::RetrySong;
                }
            }
            GameOverPhase::Inactive | GameOverPhase::Exiting => {}
        }

        events
    }

    fn begin_confirm(&mut self, events: &mut GameOverEvents) {
        self.phase = GameOverPhase::ConfirmRetry;
        self.timer_s = 0.0;
        self.can_input = false;
        self.fade_alpha = 0.0;
        events.audio_cue = Some(GameOverAudioCue::PlayGameOverEnd);
    }

    fn begin_exit(&mut self, events: &mut GameOverEvents) {
        self.phase = GameOverPhase::Exiting;
        self.death_count = 0;
        events.audio_cue = Some(GameOverAudioCue::StopMusic);
        events.action = GameOverAction::ExitToMenu;
    }

    /// Draws the game-over backdrop, status text, and retry/exit prompt with zero allocations.
    pub fn draw(
        &self,
        batcher: &mut SpriteBatcher,
        white_tex: TextureHandle,
        font: &BitmapFont,
        font_tex: TextureHandle,
    ) {
        if self.phase == GameOverPhase::Inactive {
            return;
        }

        if self.can_input && self.phase != GameOverPhase::ConfirmRetry {
            let is_small = font.line_height() <= 10.0;
            let sub_scale = if is_small { 2.0 } else { 1.05 };
            font.draw_bytes_outlined(
                batcher,
                font_tex,
                b"ENTER: Retry Song   |   ESC: Exit",
                [VIRTUAL_WIDTH * 0.5, VIRTUAL_HEIGHT - 44.0],
                &TextStyle {
                    scale: sub_scale,
                    color: [1.0, 1.0, 1.0, 0.85],
                    align: TextAlign::Center,
                    ..Default::default()
                },
                [0.0, 0.0, 0.0, 0.85],
            );
        }

        if self.fade_alpha > 0.0 {
            batcher.push(
                white_tex,
                BlendMode::Normal,
                FilterMode::Nearest,
                SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                    .with_color([0.0, 0.0, 0.0, self.fade_alpha.clamp(0.0, 1.0)]),
            );
        }
    }
}

// ============================================================================
// Results Summary Screen (`ResultState.hx`)
// ============================================================================

/// Display rank on the post-song results screen (`ScoringRank` in `Scoring.hx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResultsRank {
    Loss,
    Good,
    Great,
    Excellent,
    Perfect,
    PerfectGold,
}

impl ResultsRank {
    /// Computes [`ResultsRank`] from [`ScoreTallies`] using V-Slice `Scoring.calculateRank`.
    #[must_use]
    pub fn from_tallies(tallies: &ScoreTallies) -> Self {
        match calculate_rank(tallies) {
            Some(ScoringRank::PerfectGold) => Self::PerfectGold,
            Some(ScoringRank::Perfect) => Self::Perfect,
            Some(ScoringRank::Excellent) => Self::Excellent,
            Some(ScoringRank::Great) => Self::Great,
            Some(ScoringRank::Good) => Self::Good,
            Some(ScoringRank::Shit) | None => Self::Loss,
        }
    }

    /// Display label (`Loss`, `Good`, `Great`, `Excellent`, `Perfect`, `Perfect Gold`).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Loss => "Loss",
            Self::Good => "Good",
            Self::Great => "Great",
            Self::Excellent => "Excellent",
            Self::Perfect => "Perfect",
            Self::PerfectGold => "Perfect Gold",
        }
    }

    /// Accent color matching `ScoringRank.getRankingFreeplayColor`.
    #[must_use]
    pub const fn color(self) -> [f32; 4] {
        match self {
            Self::Loss => [0.376, 0.267, 1.0, 1.0],
            Self::Good => [0.937, 0.529, 0.392, 1.0],
            Self::Great => [0.918, 0.965, 1.0, 1.0],
            Self::Excellent => [0.992, 0.796, 0.259, 1.0],
            Self::Perfect => [1.0, 0.345, 0.706, 1.0],
            Self::PerfectGold => [1.0, 0.714, 0.098, 1.0],
        }
    }
}

/// Action emitted by [`ResultsSummaryState::update`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultsAction {
    None,
    Continue,
}

/// Computes weighted judgement accuracy percentage `[0.0, 100.0]` from [`ScoreTallies`].
///
/// Weights: `Sick = 1.0`, `Good = 0.75`, `Bad = 0.5`, `Shit = 0.25`, `Miss = 0.0`
/// divided by `total_notes` (or `hit + missed` if `total_notes == 0`).
#[must_use]
pub fn compute_accuracy_percent(tallies: &ScoreTallies) -> f64 {
    let judged_total = tallies
        .sick
        .saturating_add(tallies.good)
        .saturating_add(tallies.bad)
        .saturating_add(tallies.shit)
        .saturating_add(tallies.missed);
    let denom = tallies.total_notes.max(judged_total);
    if denom == 0 {
        return 0.0;
    }
    let weighted = f64::from(tallies.sick)
        + f64::from(tallies.good) * 0.75
        + f64::from(tallies.bad) * 0.50
        + f64::from(tallies.shit) * 0.25;
    ((weighted / f64::from(denom)) * 100.0).clamp(0.0, 100.0)
}

/// Zero-allocation post-song results screen (`ResultState.hx`).
#[derive(Debug, Clone)]
pub struct ResultsSummaryState {
    pub is_active: bool,
    pub song_name: StackTextBuffer<64>,
    pub difficulty: StackTextBuffer<32>,
    pub score: i64,
    pub tallies: ScoreTallies,
    pub rank: ResultsRank,
    pub clear_percent: f64,
    pub accuracy_percent: f64,
    pub is_new_highscore: bool,
    pub elapsed_s: f32,
}

impl Default for ResultsSummaryState {
    fn default() -> Self {
        Self::new()
    }
}

impl ResultsSummaryState {
    /// Creates an inactive [`ResultsSummaryState`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            is_active: false,
            song_name: StackTextBuffer::new(),
            difficulty: StackTextBuffer::new(),
            score: 0,
            tallies: ScoreTallies::default(),
            rank: ResultsRank::Loss,
            clear_percent: 0.0,
            accuracy_percent: 0.0,
            is_new_highscore: false,
            elapsed_s: 0.0,
            visuals: None,
        }
    }

    /// Populates and activates the results summary screen at song end.
    pub fn show(
        &mut self,
        song_name: &str,
        difficulty: &str,
        score: i64,
        tallies: ScoreTallies,
        is_new_highscore: bool,
    ) {
        self.is_active = true;
        self.song_name.clear();
        let _ = self.song_name.write_str(song_name);
        self.difficulty.clear();
        let _ = self.difficulty.write_str(difficulty);
        self.score = score;
        self.tallies = tallies;
        self.rank = ResultsRank::from_tallies(&tallies);
        self.clear_percent = tally_completion(&tallies) * 100.0;
        self.accuracy_percent = compute_accuracy_percent(&tallies);
        self.is_new_highscore = is_new_highscore;
        self.elapsed_s = 0.0;
    }

    /// Loads visual assets for the current result rank/character (`ResultsVisuals`).
    pub fn load_visuals(&mut self, assets: &AssetLocator, gpu: &mut GpuContext, is_pico: bool) {
        self.visuals = Some(Box::new(ResultsVisuals::load(
            assets,
            gpu,
            self.difficulty.as_str(),
            self.rank,
            is_pico,
        )));
    }

    /// Hides the results summary screen and releases visual textures.
    pub fn hide(&mut self) {
        self.is_active = false;
        self.visuals = None;
    }

    /// Advances display timer and checks confirm/back input with zero heap allocations.
    pub fn update(
        &mut self,
        elapsed_s: f32,
        accept_pressed: bool,
        back_pressed: bool,
    ) -> ResultsAction {
        if !self.is_active {
            return ResultsAction::None;
        }
        self.elapsed_s += elapsed_s.max(0.0);
        if let Some(vis) = &mut self.visuals {
            if let Some(atlas) = &vis.char_atlas {
                let total = atlas.total_frames().max(1);
                let frame = ((self.elapsed_s * atlas.frame_rate()).floor() as u32) % total;
                atlas.evaluate_frame(frame, &mut vis.char_parts);
            }
        }
        if self.elapsed_s >= 0.4 && (accept_pressed || back_pressed) {
            self.hide();
            return ResultsAction::Continue;
        }
        ResultsAction::None
    }

    /// Renders the post-song results screen using stack-formatted text with zero allocations.
    pub fn draw(
        &self,
        batcher: &mut SpriteBatcher,
        white_tex: TextureHandle,
        font: &BitmapFont,
        font_tex: TextureHandle,
    ) {
        if !self.is_active {
            return;
        }

        // Warm gold backdrop matching `ResultState.hx` (`0xFFFECC5C`).
        batcher.push(
            white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                .with_color([0.996, 0.780, 0.361, 1.0]),
        );

        if let Some(vis) = &self.visuals {
            if !vis.char_parts.is_empty() {
                let (b_orig, _) = vis.char_bounds;
                let base_x = 580.0 - b_orig[0];
                let base_y = 140.0 - b_orig[1];
                for part in &vis.char_parts {
                    let Some(&(tex, _)) = vis.char_textures.get(part.spritemap_index) else {
                        continue;
                    };
                    let (w, h) = if part.rotated {
                        (part.size[1], part.size[0])
                    } else {
                        (part.size[0], part.size[1])
                    };
                    let m = part.transform;
                    let flags = SPRITE_FLAG_AFFINE
                        | if part.rotated {
                            SPRITE_FLAG_ROTATED_90_CCW
                        } else {
                            0
                        };
                    let inst = SpriteInstance::new(
                        [base_x + m.translation.x, base_y + m.translation.y],
                        [m.matrix2.x_axis.x * w, m.matrix2.x_axis.y * w],
                    )
                    .with_origin([m.matrix2.y_axis.x * h, m.matrix2.y_axis.y * h])
                    .with_uv_rect([
                        part.uv_rect[0],
                        part.uv_rect[1],
                        part.uv_rect[0] + part.uv_rect[2],
                        part.uv_rect[1] + part.uv_rect[3],
                    ])
                    .with_color(part.color)
                    .with_flags(flags);
                    batcher.push(tex, BlendMode::Normal, FilterMode::Linear, inst);
                }
            }
            if let Some((bar_t, _)) = vis.top_bar_tex {
                batcher.push(
                    bar_t,
                    BlendMode::Normal,
                    FilterMode::Linear,
                    SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, 136.0]),
                );
            } else {
                batcher.push(
                    white_tex,
                    BlendMode::Normal,
                    FilterMode::Nearest,
                    SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, 136.0])
                        .with_color([0.05, 0.05, 0.07, 0.96]),
                );
            }
            if let Some((dt, dsz)) = vis.diff_tex {
                batcher.push(
                    dt,
                    BlendMode::Normal,
                    FilterMode::Linear,
                    SpriteInstance::new([56.0, 148.0], dsz),
                );
            }
            if let Some((rt, rsz)) = vis.rank_tex {
                batcher.push(
                    rt,
                    BlendMode::Normal,
                    FilterMode::Linear,
                    SpriteInstance::new([VIRTUAL_WIDTH - rsz[0] - 42.0, 18.0], rsz),
                );
            }
        } else {
            // Top black header bar (`ResultState.blackTopBar`).
            batcher.push(
                white_tex,
                BlendMode::Normal,
                FilterMode::Nearest,
                SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, 136.0])
                    .with_color([0.05, 0.05, 0.07, 0.96]),
            );
        }

        // Semi-transparent tally card.
        batcher.push(
            white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([64.0, 198.0], [540.0, 444.0])
                .with_color([0.06, 0.06, 0.09, 0.78]),
        );

        let is_small = font.line_height() <= 10.0;
        let title_scale = if is_small { 4.5 } else { 2.4 };
        let body_scale = if is_small { 2.5 } else { 1.3 };
        let rank_scale = if is_small { 5.0 } else { 2.8 };

        let mut buf = StackTextBuffer::<96>::new();

        // Header: Results + Song [Difficulty]
        font.draw_bytes_outlined(
            batcher,
            font_tex,
            b"RESULTS",
            [64.0, 26.0],
            &TextStyle {
                scale: title_scale,
                color: [1.0, 1.0, 1.0, 1.0],
                align: TextAlign::Left,
                ..Default::default()
            },
            [0.0, 0.0, 0.0, 1.0],
        );

        buf.clear();
        let _ = write!(
            buf,
            "{} [{}]",
            self.song_name.as_str(),
            self.difficulty.as_str()
        );
        font.draw_bytes_outlined(
            batcher,
            font_tex,
            buf.as_bytes(),
            [64.0, 82.0],
            &TextStyle {
                scale: body_scale,
                color: [0.95, 0.90, 0.55, 1.0],
                align: TextAlign::Left,
                ..Default::default()
            },
            [0.0, 0.0, 0.0, 1.0],
        );

        // Tallies column.
        let rows: [(&str, u32, [f32; 4]); 6] = [
            ("Sick", self.tallies.sick, [0.537, 0.898, 0.620, 1.0]),
            ("Good", self.tallies.good, [0.537, 0.788, 0.898, 1.0]),
            ("Bad", self.tallies.bad, [0.902, 0.812, 0.541, 1.0]),
            ("Shit", self.tallies.shit, [0.902, 0.549, 0.541, 1.0]),
            ("Missed", self.tallies.missed, [0.776, 0.541, 0.902, 1.0]),
            ("Max Combo", self.tallies.max_combo, [1.0, 1.0, 1.0, 1.0]),
        ];

        for (idx, (label, value, color)) in rows.iter().enumerate() {
            let y = 196.0 + idx as f32 * 48.0;
            buf.clear();
            let _ = write!(buf, "{label}: {value}");
            font.draw_bytes_outlined(
                batcher,
                font_tex,
                buf.as_bytes(),
                [96.0, y],
                &TextStyle {
                    scale: body_scale,
                    color: *color,
                    align: TextAlign::Left,
                    ..Default::default()
                },
                [0.0, 0.0, 0.0, 1.0],
            );
        }

        buf.clear();
        let _ = write!(buf, "Accuracy: {:.1}%", self.accuracy_percent);
        font.draw_bytes_outlined(
            batcher,
            font_tex,
            buf.as_bytes(),
            [96.0, 492.0],
            &TextStyle {
                scale: body_scale,
                color: [1.0, 1.0, 1.0, 1.0],
                align: TextAlign::Left,
                ..Default::default()
            },
            [0.0, 0.0, 0.0, 1.0],
        );

        buf.clear();
        let roll = ((self.elapsed_s / 1.4).clamp(0.0, 1.0) as f64) * self.clear_percent;
        let _ = write!(buf, "Clear: {:.0}%", roll.floor());
        font.draw_bytes_outlined(
            batcher,
            font_tex,
            buf.as_bytes(),
            [96.0, 540.0],
            &TextStyle {
                scale: body_scale,
                color: [1.0, 0.92, 0.45, 1.0],
                align: TextAlign::Left,
                ..Default::default()
            },
            [0.0, 0.0, 0.0, 1.0],
        );

        buf.clear();
        let _ = write!(buf, "Score: {}", self.score);
        font.draw_bytes_outlined(
            batcher,
            font_tex,
            buf.as_bytes(),
            [96.0, 588.0],
            &TextStyle {
                scale: body_scale,
                color: [1.0, 1.0, 1.0, 1.0],
                align: TextAlign::Left,
                ..Default::default()
            },
            [0.0, 0.0, 0.0, 1.0],
        );

        // Right-side Rank panel when animated Results character is unavailable.
        let has_char_anim = self
            .visuals
            .as_ref()
            .is_some_and(|v| !v.char_parts.is_empty());
        if !has_char_anim {
            batcher.push(
                white_tex,
                BlendMode::Normal,
                FilterMode::Nearest,
                SpriteInstance::new([748.0, 168.0], [468.0, 468.0])
                    .with_color([0.06, 0.06, 0.09, 0.82]),
            );
        }

        font.draw_bytes_outlined(
            batcher,
            font_tex,
            b"RANK",
            [982.0, 224.0],
            &TextStyle {
                scale: body_scale,
                color: [0.85, 0.85, 0.90, 1.0],
                align: TextAlign::Center,
                ..Default::default()
            },
            [0.0, 0.0, 0.0, 1.0],
        );

        font.draw_bytes_outlined(
            batcher,
            font_tex,
            self.rank.label().as_bytes(),
            [982.0, 316.0],
            &TextStyle {
                scale: rank_scale,
                color: self.rank.color(),
                align: TextAlign::Center,
                ..Default::default()
            },
            [0.0, 0.0, 0.0, 1.0],
        );

        if self.is_new_highscore {
            font.draw_bytes_outlined(
                batcher,
                font_tex,
                b"NEW HIGHSCORE!",
                [982.0, 420.0],
                &TextStyle {
                    scale: body_scale,
                    color: [1.0, 0.88, 0.25, 1.0],
                    align: TextAlign::Center,
                    ..Default::default()
                },
                [0.0, 0.0, 0.0, 1.0],
            );
        }

        font.draw_bytes_outlined(
            batcher,
            font_tex,
            b"Press ENTER or ESC to continue",
            [982.0, 560.0],
            &TextStyle {
                scale: body_scale * 0.85,
                color: [0.90, 0.90, 0.95, 1.0],
                align: TextAlign::Center,
                ..Default::default()
            },
            [0.0, 0.0, 0.0, 1.0],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_frame_selection_for_two_and_three_frame_strips() {
        // 2-frame strip (neutral 0, losing 1)
        assert_eq!(HealthIcon::select_strip_frame(0.10, 2), 1);
        assert_eq!(HealthIcon::select_strip_frame(0.199, 2), 1);
        assert_eq!(HealthIcon::select_strip_frame(0.20, 2), 0);
        assert_eq!(HealthIcon::select_strip_frame(0.50, 2), 0);
        assert_eq!(HealthIcon::select_strip_frame(0.95, 2), 0);

        // 3-frame strip (neutral 0, losing 1, winning 2)
        assert_eq!(HealthIcon::select_strip_frame(0.15, 3), 1);
        assert_eq!(HealthIcon::select_strip_frame(0.50, 3), 0);
        assert_eq!(HealthIcon::select_strip_frame(0.80, 3), 0);
        assert_eq!(HealthIcon::select_strip_frame(0.85, 3), 2);

        // 1-frame icon always stays on 0
        assert_eq!(HealthIcon::select_strip_frame(0.05, 1), 0);
        assert_eq!(HealthIcon::select_strip_frame(0.95, 1), 0);
    }

    #[test]
    fn health_bar_split_and_icon_positions_match_vslice() {
        let bar = HealthBarLayout::compute(601.0, 19.0, false, 1.0);
        // 50% health => split_x is exact center of bar (640.0)
        assert!((bar.split_x - 640.0).abs() < 1e-4);
        assert!((bar.bg_y - 648.0).abs() < 1e-4);

        // Player icon on right of split with 26px overlap
        let p_x = bar.icon_x(true, 150.0);
        assert!((p_x - (640.0 - 26.0)).abs() < 1e-4);

        // Opponent icon on left of split with 26px overlap
        let o_x = bar.icon_x(false, 150.0);
        assert!((o_x - (640.0 - (150.0 - 26.0))).abs() < 1e-4);

        // Y centered on healthBarBG
        let y = bar.icon_y_centered(150.0);
        assert!((y - (648.0 + (19.0 - 150.0) * 0.5)).abs() < 1e-4);

        // Downscroll moves health bar to 10% height (72.0)
        let down_bar = HealthBarLayout::compute(601.0, 19.0, true, 2.0);
        assert!((down_bar.bg_y - 72.0).abs() < 1e-4);
        assert!((down_bar.split_x - down_bar.bar_x).abs() < 1e-4);
    }

    #[test]
    fn health_icon_bop_decays_exponentially() {
        let mut icon = HealthIcon::placeholder("bf", true, false);
        let bar = HealthBarLayout::compute(601.0, 19.0, false, 1.0);
        icon.bop();
        assert!((icon.bop_scale_mult - 1.2).abs() < 1e-6);

        icon.update(0.1, 1.0, &bar);
        assert!(icon.bop_scale_mult < 1.2 && icon.bop_scale_mult > 1.0);

        for _ in 0..20 {
            icon.update(0.05, 1.0, &bar);
        }
        assert!((icon.bop_scale_mult - 1.0).abs() < 1e-3);
    }

    #[test]
    fn pause_menu_navigation_wraps_around_and_guards_first_frame() {
        let mut menu = PauseMenuState::new("Bopeebo", "normal");
        menu.open(false, false, 1);
        let count = menu.item_count();
        assert_eq!(count, 6);
        assert_eq!(menu.selected, 0);

        // Wrap upward from 0 -> last item
        menu.move_selection(-1);
        assert_eq!(menu.selected, count - 1);

        // Wrap downward from last -> 0
        menu.move_selection(1);
        assert_eq!(menu.selected, 0);

        // First frame ignores accept so opening key doesn't immediately resume
        let first = menu.update(
            0.016,
            PauseMenuInput {
                accept_pressed: true,
                ..Default::default()
            },
        );
        assert_eq!(first, PauseMenuAction::None);
        assert!(menu.is_open);

        // Next frame toggles botplay
        menu.move_selection(3);
        assert_eq!(menu.selected_main_option(), Some(PauseMenuOption::ToggleBotplay));
        let act = menu.update(
            0.016,
            PauseMenuInput {
                accept_pressed: true,
                ..Default::default()
            },
        );
        assert_eq!(act, PauseMenuAction::ToggleBotplay);
        assert!(menu.is_botplay);
    }

    #[test]
    fn game_over_transitions_and_cues() {
        let mut go = GameOverState::new();
        let start = go.trigger_death();
        assert_eq!(go.phase, GameOverPhase::FirstDeath);
        assert_eq!(start.audio_cue, Some(GameOverAudioCue::PlayLossSfx));

        // Before 1s unlock, accept is ignored
        let early = go.update(0.5, true, false);
        assert_eq!(early.action, GameOverAction::None);
        assert_eq!(go.phase, GameOverPhase::FirstDeath);

        // Crossing 2s transitions to DeathLoop and emits StartGameOverLoop
        let to_loop = go.update(1.6, false, false);
        assert_eq!(go.phase, GameOverPhase::DeathLoop);
        assert_eq!(to_loop.audio_cue, Some(GameOverAudioCue::StartGameOverLoop));

        // Accept starts confirm & gameOverEnd, then completes retry after fade
        let confirm = go.update(0.016, true, false);
        assert_eq!(go.phase, GameOverPhase::ConfirmRetry);
        assert_eq!(confirm.audio_cue, Some(GameOverAudioCue::PlayGameOverEnd));

        let done = go.update(2.1, false, false);
        assert_eq!(done.action, GameOverAction::RetrySong);
        assert_eq!(go.phase, GameOverPhase::Inactive);
    }

    #[test]
    fn results_rank_and_accuracy_calculation() {
        let perfect_gold = ScoreTallies {
            sick: 100,
            total_notes: 100,
            total_notes_hit: 100,
            ..Default::default()
        };
        assert_eq!(ResultsRank::from_tallies(&perfect_gold), ResultsRank::PerfectGold);
        assert!((compute_accuracy_percent(&perfect_gold) - 100.0).abs() < 1e-6);

        let perfect = ScoreTallies {
            sick: 95,
            good: 5,
            total_notes: 100,
            total_notes_hit: 100,
            ..Default::default()
        };
        assert_eq!(ResultsRank::from_tallies(&perfect), ResultsRank::Perfect);

        let excellent = ScoreTallies {
            sick: 80,
            good: 12,
            bad: 6,
            missed: 2,
            total_notes: 100,
            total_notes_hit: 98,
            ..Default::default()
        };
        assert_eq!(ResultsRank::from_tallies(&excellent), ResultsRank::Excellent);

        let great = ScoreTallies {
            sick: 70,
            good: 12,
            bad: 15,
            missed: 2,
            total_notes: 100,
            total_notes_hit: 97,
            ..Default::default()
        };
        assert_eq!(ResultsRank::from_tallies(&great), ResultsRank::Great);

        let good = ScoreTallies {
            sick: 50,
            good: 15,
            bad: 30,
            missed: 5,
            total_notes: 100,
            total_notes_hit: 95,
            ..Default::default()
        };
        assert_eq!(ResultsRank::from_tallies(&good), ResultsRank::Good);

        let loss = ScoreTallies {
            sick: 20,
            good: 10,
            missed: 70,
            total_notes: 100,
            total_notes_hit: 30,
            ..Default::default()
        };
        assert_eq!(ResultsRank::from_tallies(&loss), ResultsRank::Loss);
    }
}
