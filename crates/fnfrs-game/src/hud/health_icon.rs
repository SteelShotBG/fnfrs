use std::fs;

use fnfrs_core::constants::{
    DEFAULT_HEALTH_ICON, HEALTH_ICON_ANGLE_HALF_LIFE, HEALTH_ICON_BOP_SCALE, HEALTH_ICON_SIZE,
    HEALTH_MAX, STEPS_PER_BEAT, VIRTUAL_HEIGHT, VIRTUAL_WIDTH,
};
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::character::HealthIconData;
use fnfrs_render::{
    AtlasFrame, BlendMode, FilterMode, GpuContext, SparrowAtlas, SpriteBatcher, SpriteInstance,
    TextureHandle, SPRITE_FLAG_FLIP_X, SPRITE_FLAG_FLIP_Y, SPRITE_FLAG_PIXEL_SNAP,
    SPRITE_FLAG_ROTATED_90_CCW,
};
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
