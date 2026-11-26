//! Runtime V-Slice stage loader, props, beat-synced `Bopper`s, and depth-sorted
//! rendering matching `funkin/play/stage/{Stage.hx,StageProp.hx,Bopper.hx}`.

use std::collections::HashMap;
use std::fs;

use fnfrs_core::constants::STEPS_PER_BEAT;
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::parse_hex_color;
use fnfrs_core::schema::stage::{StageData, StageDataCharacter, StageDataCharacters, StageDataProp};
use fnfrs_render::{
    BlendMode, FilterMode, GpuContext, SpriteBatcher, SpriteInstance, TextureHandle,
    SPRITE_FLAG_FLIP_X, SPRITE_FLAG_FLIP_Y,
};

use crate::character::{
    apply_camera_transform, build_loaded_anim, format_stack_join, load_animate_source,
    load_packer_source, load_sparrow_source, upload_png_path, AnimBank, AnimFrames,
    AtlasDrawParams, BaseCharacter, LoadedAnim,
};

/// Parses a `#RRGGBB`, `#AARRGGBB`, `#RGB`, or `0xAARRGGBB` color string into `[r, g, b, a]`.
#[must_use]
pub fn parse_stage_color(raw: &str) -> [f32; 4] {
    let trimmed = raw.trim();
    if let Some(hex) = trimmed.strip_prefix('#') {
        if hex.len() == 3 {
            if let Ok(rgb) = u32::from_str_radix(hex, 16) {
                let r = ((rgb >> 8) & 0xF) as f32 / 15.0;
                let g = ((rgb >> 4) & 0xF) as f32 / 15.0;
                let b = (rgb & 0xF) as f32 / 15.0;
                return [r, g, b, 1.0];
            }
        }
    }
    parse_hex_color(trimmed)
}

/// Parses a V-Slice stage prop blend string (`""`, `"normal"`, `"add"`, `"screen"`).
#[must_use]
pub fn parse_blend_mode(blend: &str) -> BlendMode {
    match blend.trim().to_ascii_lowercase().as_str() {
        "add" => BlendMode::Add,
        "screen" => BlendMode::Screen,
        _ => BlendMode::Normal,
    }
}

/// Beat/step-synchronized animated stage prop controller (`Bopper.hx`).
#[derive(Debug)]
pub struct Bopper {
    pub dance_every: f32,
    pub should_alternate: bool,
    pub has_danced: bool,
    pub starting_dance: bool,
    pub should_bop: bool,
    pub can_play_other_anims: bool,
    pub idle_suffix: String,
    pub global_offsets: [f32; 2],
    pub original_position: [f32; 2],

    bank: AnimBank,
    idle_anim: Option<usize>,
    dance_left_anim: Option<usize>,
    dance_right_anim: Option<usize>,
    last_dance_step: Option<i32>,
}

impl Bopper {
    pub(crate) fn from_bank(
        dance_every: f32,
        starting_dance: bool,
        original_position: [f32; 2],
        mut bank: AnimBank,
    ) -> Self {
        bank.finalize_links();
        let idle_anim = bank.anim_index.get("idle").copied();
        let dance_left_anim = bank.anim_index.get("danceLeft").copied();
        let dance_right_anim = bank.anim_index.get("danceRight").copied();
        let should_alternate = dance_left_anim.is_some();

        Self {
            dance_every,
            should_alternate,
            has_danced: starting_dance,
            starting_dance,
            should_bop: true,
            can_play_other_anims: true,
            idle_suffix: String::new(),
            global_offsets: [0.0, 0.0],
            original_position,
            bank,
            idle_anim,
            dance_left_anim,
            dance_right_anim,
            last_dance_step: None,
        }
    }

    /// Creates a headless [`Bopper`] with named animations for testing beat/step alternation.
    #[must_use]
    pub fn new_headless(dance_every: f32, anim_names: &[&str], starting_dance: bool) -> Self {
        let mut bank = AnimBank::new();
        for &name in anim_names {
            bank.anims.push(crate::character::make_loaded_anim_entry(
                name.to_owned(),
                [0.0, 0.0],
                false,
                false,
                false,
                24.0,
                AnimFrames::Sheet {
                    atlas_idx: 0,
                    frames: Vec::new(),
                },
            ));
        }
        Self::from_bank(dance_every, starting_dance, [0.0, 0.0], bank)
    }

    #[inline]
    #[must_use]
    pub fn current_animation(&self) -> &str {
        self.bank.current_name()
    }

    #[inline]
    #[must_use]
    pub fn current_anim_offsets(&self) -> [f32; 2] {
        self.bank.current_offsets()
    }

    #[inline]
    #[must_use]
    pub fn has_animation(&self, name: &str) -> bool {
        self.bank.anim_index.contains_key(name)
    }

    /// Plays `name` with `Bopper.correctAnimationName` `-suffix` fallback.
    pub fn play_animation(&mut self, name: &str, restart: bool) {
        if !self.can_play_other_anims && !(self.current_animation() == name && restart) {
            return;
        }
        if let Some(idx) = self.bank.resolve_with_fallback(name) {
            self.bank.play_index(idx, restart);
        }
    }

    /// Triggers `danceLeft`/`danceRight` (if alternating) or `idle` (`Bopper.dance`).
    pub fn dance(&mut self, force: bool) {
        if self.should_alternate {
            let play_right = self.has_danced;
            let played = if self.idle_suffix.is_empty() {
                let target = if play_right {
                    self.dance_right_anim
                } else {
                    self.dance_left_anim
                };
                if let Some(idx) = target {
                    self.bank.play_index(idx, force);
                    true
                } else {
                    false
                }
            } else {
                let mut buf = [0u8; 96];
                let base = if play_right { "danceRight" } else { "danceLeft" };
                if let Some(name) = format_stack_join(&mut buf, base, &self.idle_suffix) {
                    if let Some(idx) = self.bank.resolve_with_fallback(name) {
                        self.bank.play_index(idx, force);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            };
            if played {
                self.has_danced = !self.has_danced;
            }
        } else if self.idle_suffix.is_empty() {
            if let Some(idx) = self.idle_anim {
                self.bank.play_index(idx, force);
            }
        } else {
            let mut buf = [0u8; 96];
            if let Some(name) = format_stack_join(&mut buf, "idle", &self.idle_suffix) {
                if let Some(idx) = self.bank.resolve_with_fallback(name) {
                    self.bank.play_index(idx, force);
                }
            }
        }
    }

    /// Step callback supporting `dance_every` down to `0.25` beats (`Bopper.onStepHit`).
    pub fn on_step_hit(&mut self, step: i32) {
        if self.dance_every <= 0.0 || step < 0 {
            return;
        }
        let steps_per_dance = (self.dance_every * STEPS_PER_BEAT as f32).round() as i32;
        if steps_per_dance <= 0 {
            return;
        }
        if (step as u32).is_multiple_of(steps_per_dance as u32) && self.last_dance_step != Some(step) {
            self.last_dance_step = Some(step);
            self.dance(self.should_bop);
        }
    }

    /// Beat callback for integer or sub-beat `dance_every` cadences.
    pub fn on_beat_hit(&mut self, beat: i32) {
        if self.dance_every <= 0.0 || beat < 0 {
            return;
        }
        let step = beat * (STEPS_PER_BEAT as i32);
        let steps_per_dance = (self.dance_every * STEPS_PER_BEAT as f32).round() as i32;
        if steps_per_dance <= 0 {
            return;
        }
        let should_dance = if self.dance_every < 1.0 {
            true
        } else {
            (step as u32).is_multiple_of(steps_per_dance as u32)
        };
        if should_dance && self.last_dance_step != Some(step) {
            self.last_dance_step = Some(step);
            self.dance(self.should_bop);
        }
    }

    /// Zero-allocation animation frame update.
    pub fn update(&mut self, elapsed_s: f32) {
        if self.bank.update(elapsed_s) && !self.can_play_other_anims {
            self.can_play_other_anims = true;
        }
    }
}

#[derive(Debug)]
enum PropVisual {
    SolidRect {
        texture: TextureHandle,
        size: [f32; 2],
        color: [f32; 4],
    },
    StaticImage {
        texture: TextureHandle,
        tex_size: [f32; 2],
    },
    Animated,
    Empty,
}

/// A single prop placed in a [`Stage`] (`StageProp.hx`).
#[derive(Debug)]
pub struct StageProp {
    pub name: Option<String>,
    pub position: [f32; 2],
    pub original_position: [f32; 2],
    pub scroll_factor: [f32; 2],
    pub scale: [f32; 2],
    pub z_index: i32,
    pub alpha: f32,
    pub angle: f32,
    pub flip_x: bool,
    pub flip_y: bool,
    pub blend: BlendMode,
    pub color: [f32; 4],
    pub is_pixel: bool,
    pub is_hud: bool,
    pub visible: bool,
    pub initial_dance_every: f32,
    pub bopper: Option<Bopper>,
    visual: PropVisual,
}

impl StageProp {
    /// Creates a `#RRGGBB` solid rectangle prop (`Stage.hx` `makeSolidColor`).
    #[must_use]
    pub fn solid_rect(
        name: Option<String>,
        white_texture: TextureHandle,
        position: [f32; 2],
        size: [f32; 2],
        color_hex: &str,
        z_index: i32,
    ) -> Self {
        Self {
            name,
            position,
            original_position: position,
            scroll_factor: [1.0, 1.0],
            scale: [1.0, 1.0],
            z_index,
            alpha: 1.0,
            angle: 0.0,
            flip_x: false,
            flip_y: false,
            blend: BlendMode::Normal,
            color: parse_stage_color(color_hex),
            is_pixel: false,
            is_hud: false,
            visible: true,
            initial_dance_every: 0.0,
            bopper: None,
            visual: PropVisual::SolidRect {
                texture: white_texture,
                size,
                color: parse_stage_color(color_hex),
            },
        }
    }

    /// Loads one [`StageDataProp`] from disk via `assets` and `gpu`.
    #[must_use]
    pub fn from_data(
        data: &StageDataProp,
        assets: &AssetLocator,
        gpu: &mut GpuContext,
    ) -> Self {
        let is_solid = data.asset_path.trim().starts_with('#');
        let is_animated = !data.animations.is_empty();
        let scale_xy = data.scale.as_xy();
        let blend = parse_blend_mode(&data.blend);
        let prop_color = if is_solid {
            parse_stage_color(&data.asset_path)
        } else {
            parse_stage_color(&data.color)
        };

        let mut bopper: Option<Bopper> = None;
        let visual = if is_animated {
            let mut bank = AnimBank::new();
            let src = if data.anim_type.eq_ignore_ascii_case("packer") {
                load_packer_source(assets, gpu, &data.asset_path)
            } else if data.anim_type.eq_ignore_ascii_case("animateatlas") {
                load_animate_source(assets, gpu, &data.asset_path, data.atlas_settings.as_ref())
            } else {
                load_sparrow_source(assets, gpu, &data.asset_path)
            };

            if let Some(loaded_src) = src {
                bank.atlases.push(loaded_src);
                for anim in &data.animations {
                    if let Some(loaded) = build_loaded_anim(anim, &bank.atlases, 0) {
                        bank.anims.push(loaded);
                    }
                }
            }

            let starting_dance = data
                .starting_animation
                .as_deref()
                .is_some_and(|s| s.starts_with("danceLeft"));
            let mut bop = Bopper::from_bank(data.dance_every, starting_dance, data.position, bank);
            if let Some(start_anim) = data.starting_animation.as_deref() {
                bop.play_animation(start_anim, true);
            }
            bopper = Some(bop);
            PropVisual::Animated
        } else if is_solid {
            PropVisual::SolidRect {
                texture: gpu.white_texture(),
                size: scale_xy,
                color: prop_color,
            }
        } else if let Some((texture, tex_size)) =
            upload_png_path(gpu, Some(&data.asset_path), &assets.image(&data.asset_path))
        {
            if data.dance_every != 0.0 {
                bopper = Some(Bopper::from_bank(
                    data.dance_every,
                    false,
                    data.position,
                    AnimBank::new(),
                ));
            }
            PropVisual::StaticImage { texture, tex_size }
        } else {
            PropVisual::Empty
        };

        Self {
            name: data.name.clone(),
            position: data.position,
            original_position: data.position,
            scroll_factor: data.scroll,
            scale: if is_solid { [1.0, 1.0] } else { scale_xy },
            z_index: data.z_index,
            alpha: data.alpha,
            angle: data.angle,
            flip_x: data.flip_x,
            flip_y: data.flip_y,
            blend,
            color: prop_color,
            is_pixel: data.is_pixel,
            is_hud: false,
            visible: true,
            initial_dance_every: data.dance_every,
            bopper,
            visual,
        }
    }

    /// Creates a headless [`StageProp`] without GPU textures (for unit tests).
    #[must_use]
    pub fn from_data_headless(data: &StageDataProp) -> Self {
        let is_solid = data.asset_path.trim().starts_with('#');
        let is_animated = !data.animations.is_empty();
        let scale_xy = data.scale.as_xy();
        let prop_color = if is_solid {
            parse_stage_color(&data.asset_path)
        } else {
            parse_stage_color(&data.color)
        };

        let mut bopper: Option<Bopper> = None;
        let visual = if is_animated || data.dance_every != 0.0 {
            let mut bank = AnimBank::new();
            for anim in &data.animations {
                bank.anims.push(LoadedAnim {
                    name: anim.name.clone(),
                    offsets: anim.offsets,
                    looped: anim.looped,
                    flip_x: anim.flip_x,
                    flip_y: anim.flip_y,
                    frame_rate: anim.frame_rate.max(1) as f32,
                    frames: AnimFrames::Sheet {
                        atlas_idx: 0,
                        frames: Vec::new(),
                    },
                    hold_anim: None,
                    end_anim: None,
                    is_sing: false,
                    is_miss: false,
                    is_hold_suffix: false,
                    is_end_suffix: false,
                    is_idle_or_dance: anim.name.starts_with("idle")
                        || anim.name.starts_with("dance"),
                    is_combo_or_drop: false,
                });
            }
            let starting_dance = data
                .starting_animation
                .as_deref()
                .is_some_and(|s| s.starts_with("danceLeft"));
            let mut bop = Bopper::from_bank(data.dance_every, starting_dance, data.position, bank);
            if let Some(start_anim) = data.starting_animation.as_deref() {
                bop.play_animation(start_anim, true);
            }
            bopper = Some(bop);
            PropVisual::Animated
        } else if is_solid {
            PropVisual::SolidRect {
                texture: TextureHandle(0),
                size: scale_xy,
                color: prop_color,
            }
        } else {
            PropVisual::Empty
        };

        Self {
            name: data.name.clone(),
            position: data.position,
            original_position: data.position,
            scroll_factor: data.scroll,
            scale: if is_solid { [1.0, 1.0] } else { scale_xy },
            z_index: data.z_index,
            alpha: data.alpha,
            angle: data.angle,
            flip_x: data.flip_x,
            flip_y: data.flip_y,
            blend: parse_blend_mode(&data.blend),
            color: prop_color,
            is_pixel: data.is_pixel,
            is_hud: false,
            visible: true,
            initial_dance_every: data.dance_every,
            bopper,
            visual,
        }
    }

    /// Updates prop animation state (if a `Bopper`).
    pub fn update(&mut self, elapsed_s: f32) {
        if let Some(bop) = &mut self.bopper {
            bop.update(elapsed_s);
        }
    }

    /// Triggers step-based bopper dance (`Bopper.onStepHit`).
    pub fn on_step_hit(&mut self, step: i32) {
        if let Some(bop) = &mut self.bopper {
            bop.on_step_hit(step);
        }
    }

    /// Triggers beat-based bopper dance.
    pub fn on_beat_hit(&mut self, beat: i32) {
        if let Some(bop) = &mut self.bopper {
            bop.on_beat_hit(beat);
        }
    }

    /// Queues this prop's sprite instance(s) into `batcher` with camera parallax & zoom.
    pub fn draw(
        &self,
        batcher: &mut SpriteBatcher,
        cam_pos: [f32; 2],
        cam_zoom: f32,
        hud_zoom: f32,
    ) {
        if !self.visible || self.alpha <= 0.0 {
            return;
        }
        let (eff_cam_pos, eff_zoom, eff_scroll) = if self.is_hud {
            ([0.0, 0.0], hud_zoom, [0.0, 0.0])
        } else {
            (cam_pos, cam_zoom, self.scroll_factor)
        };

        match &self.visual {
            PropVisual::SolidRect {
                texture,
                size,
                color,
            } => {
                let world_w = size[0] * self.scale[0];
                let world_h = size[1] * self.scale[1];
                let (screen_pos, screen_size) = apply_camera_transform(
                    self.position,
                    [world_w, world_h],
                    eff_scroll,
                    eff_cam_pos,
                    eff_zoom,
                );
                let tint = [color[0], color[1], color[2], color[3] * self.alpha];
                let origin = [screen_size[0] * 0.5, screen_size[1] * 0.5];
                let inst = SpriteInstance::new(screen_pos, screen_size)
                    .with_color(tint)
                    .with_rotation(self.angle.to_radians(), origin);
                batcher.push(*texture, self.blend, FilterMode::Nearest, inst);
            }
            PropVisual::StaticImage { texture, tex_size } => {
                let world_w = tex_size[0] * self.scale[0];
                let world_h = tex_size[1] * self.scale[1];
                let (screen_pos, screen_size) = apply_camera_transform(
                    self.position,
                    [world_w, world_h],
                    eff_scroll,
                    eff_cam_pos,
                    eff_zoom,
                );
                let mut flags = 0u32;
                if self.flip_x {
                    flags |= SPRITE_FLAG_FLIP_X;
                }
                if self.flip_y {
                    flags |= SPRITE_FLAG_FLIP_Y;
                }
                let filter = if self.is_pixel {
                    FilterMode::Nearest
                } else {
                    FilterMode::Linear
                };
                let tint = [
                    self.color[0],
                    self.color[1],
                    self.color[2],
                    self.color[3] * self.alpha,
                ];
                let origin = [screen_size[0] * 0.5, screen_size[1] * 0.5];
                let inst = SpriteInstance::new(screen_pos, screen_size)
                    .with_color(tint)
                    .with_rotation(self.angle.to_radians(), origin)
                    .with_flags(flags);
                batcher.push(*texture, self.blend, filter, inst);
            }
            PropVisual::Animated => {
                let Some(bop) = &self.bopper else {
                    return;
                };
                let params = AtlasDrawParams {
                    pos: self.position,
                    scale: self.scale,
                    anim_offsets: bop.current_anim_offsets(),
                    global_offsets: bop.global_offsets,
                    scroll_factor: eff_scroll,
                    cam_pos: eff_cam_pos,
                    cam_zoom: eff_zoom,
                    angle_deg: self.angle,
                    flip_x: self.flip_x,
                    flip_y: self.flip_y,
                    color: self.color,
                    alpha: self.alpha,
                    blend: self.blend,
                    is_pixel: self.is_pixel,
                    ref_unscaled_size: None,
                };
                bop.bank.draw_current(batcher, &params);
            }
            PropVisual::Empty => {}
        }
    }
}

/// Loaded V-Slice stage containing depth-sorted props, boppers, and placements (`Stage.hx`).
#[derive(Debug)]
pub struct Stage {
    pub id: String,
    pub name: String,
    pub directory: String,
    pub camera_zoom: f32,
    pub props: Vec<StageProp>,
    pub placements: StageDataCharacters,

    named_props: HashMap<String, usize>,
    bopper_indices: Vec<usize>,
}

impl Stage {
    /// Loads `data/stages/<stage_id>.json` and builds the [`Stage`] using `assets` & `gpu`.
    pub fn load_by_id(
        stage_id: &str,
        assets: &AssetLocator,
        gpu: &mut GpuContext,
    ) -> Option<Self> {
        let key = format!("stages/{stage_id}");
        let path = assets.data_json(&key);
        let raw = fs::read_to_string(&path).ok()?;
        let data: StageData = serde_json::from_str(&raw).ok()?;
        Some(Self::load_from_assets(stage_id, data, assets, gpu))
    }

    /// Builds a [`Stage`] from [`StageData`] across `stage.directory`, `shared`, and `preload`.
    pub fn load_from_assets(
        id: &str,
        data: StageData,
        assets: &AssetLocator,
        gpu: &mut GpuContext,
    ) -> Self {
        let mut scoped_assets = assets.clone();
        if !data.directory.is_empty() {
            scoped_assets.set_current_level(Some(&data.directory));
        }

        let mut props = Vec::with_capacity(data.props.len());
        for prop_data in &data.props {
            props.push(StageProp::from_data(prop_data, &scoped_assets, gpu));
        }

        let mut stage = Self {
            id: id.to_owned(),
            name: data.name,
            directory: data.directory,
            camera_zoom: data.camera_zoom,
            props,
            placements: data.characters,
            named_props: HashMap::new(),
            bopper_indices: Vec::new(),
        };
        stage.refresh();
        stage
    }

    /// Builds a headless [`Stage`] from [`StageData`] without GPU textures (for unit tests).
    #[must_use]
    pub fn from_data_headless(id: &str, data: StageData) -> Self {
        let mut props = Vec::with_capacity(data.props.len());
        for prop_data in &data.props {
            props.push(StageProp::from_data_headless(prop_data));
        }

        let mut stage = Self {
            id: id.to_owned(),
            name: data.name,
            directory: data.directory,
            camera_zoom: data.camera_zoom,
            props,
            placements: data.characters,
            named_props: HashMap::new(),
            bopper_indices: Vec::new(),
        };
        stage.refresh();
        stage
    }

    /// Sorts props ascending by `z_index` (preserving relative order on ties) and rebuilds
    /// named-prop and bopper lookup tables (`Stage.refresh`).
    pub fn refresh(&mut self) {
        self.props.sort_by_key(|p| p.z_index);
        self.named_props.clear();
        self.bopper_indices.clear();

        for (idx, prop) in self.props.iter().enumerate() {
            if let Some(name) = prop.name.as_deref() {
                self.named_props.insert(name.to_owned(), idx);
            }
            if prop.bopper.is_some() {
                self.bopper_indices.push(idx);
            }
        }
    }

    /// Adds a prop and re-sorts the stage by `z_index`.
    pub fn add_prop(&mut self, prop: StageProp) {
        self.props.push(prop);
        self.refresh();
    }

    /// Looks up a named prop (`Stage.getNamedProp`).
    #[inline]
    #[must_use]
    pub fn get_named_prop(&self, name: &str) -> Option<&StageProp> {
        self.named_props.get(name).and_then(|&i| self.props.get(i))
    }

    /// Mutable lookup for a named prop.
    #[inline]
    #[must_use]
    pub fn get_named_prop_mut(&mut self, name: &str) -> Option<&mut StageProp> {
        self.named_props
            .get(name)
            .copied()
            .and_then(|i| self.props.get_mut(i))
    }

    /// Boyfriend placement metadata (`position`, `zIndex`, `cameraOffsets`, `scale`).
    #[inline]
    #[must_use]
    pub fn bf_placement(&self) -> &StageDataCharacter {
        &self.placements.bf
    }

    /// Opponent/Dad placement metadata (`position`, `zIndex`, `cameraOffsets`, `scale`).
    #[inline]
    #[must_use]
    pub fn dad_placement(&self) -> &StageDataCharacter {
        &self.placements.dad
    }

    /// Girlfriend placement metadata (`position`, `zIndex`, `cameraOffsets`, `scale`).
    #[inline]
    #[must_use]
    pub fn gf_placement(&self) -> &StageDataCharacter {
        &self.placements.gf
    }

    /// Resets named props and character placements (`Stage.resetStage`).
    pub fn reset_stage(
        &mut self,
        bf: Option<&mut BaseCharacter>,
        dad: Option<&mut BaseCharacter>,
        gf: Option<&mut BaseCharacter>,
    ) {
        if let Some(c) = bf {
            c.place_on_stage(&self.placements.bf);
        }
        if let Some(c) = dad {
            c.place_on_stage(&self.placements.dad);
        }
        if let Some(c) = gf {
            c.place_on_stage(&self.placements.gf);
        }

        for prop in &mut self.props {
            if prop.name.is_some() {
                prop.position = prop.original_position;
                if let Some(bop) = &mut prop.bopper {
                    bop.dance_every = prop.initial_dance_every;
                    bop.has_danced = bop.starting_dance;
                    bop.last_dance_step = None;
                }
            }
        }
    }

    /// Zero-allocation per-frame stage prop update.
    pub fn update(&mut self, elapsed_s: f32) {
        for &idx in &self.bopper_indices {
            if let Some(prop) = self.props.get_mut(idx) {
                prop.update(elapsed_s);
            }
        }
    }

    /// Zero-allocation step callback for all stage boppers.
    pub fn on_step_hit(&mut self, step: i32) {
        for &idx in &self.bopper_indices {
            if let Some(prop) = self.props.get_mut(idx) {
                prop.on_step_hit(step);
            }
        }
    }

    /// Zero-allocation beat callback for all stage boppers.
    pub fn on_beat_hit(&mut self, beat: i32) {
        for &idx in &self.bopper_indices {
            if let Some(prop) = self.props.get_mut(idx) {
                prop.on_beat_hit(beat);
            }
        }
    }

    /// Draws all stage props sorted by `z_index` with zero heap allocations.
    pub fn draw(
        &self,
        batcher: &mut SpriteBatcher,
        cam_pos: [f32; 2],
        cam_zoom: f32,
        hud_zoom: f32,
    ) {
        for prop in &self.props {
            prop.draw(batcher, cam_pos, cam_zoom, hud_zoom);
        }
    }

    /// Draws stage props and characters interleaved in ascending `z_index` order
    /// with zero heap allocations.
    pub fn draw_with_characters(
        &self,
        batcher: &mut SpriteBatcher,
        cam_pos: [f32; 2],
        cam_zoom: f32,
        hud_zoom: f32,
        chars: &[&BaseCharacter],
    ) {
        const MAX_INLINE_CHARS: usize = 16;
        let char_count = chars.len().min(MAX_INLINE_CHARS);
        let mut order = [(i32::MAX, usize::MAX); MAX_INLINE_CHARS];
        for (i, ch) in chars.iter().take(char_count).enumerate() {
            order[i] = (ch.z_index, i);
        }
        // Stable insertion sort on small stack slice (zero heap allocation).
        for i in 1..char_count {
            let key = order[i];
            let mut j = i;
            while j > 0 && order[j - 1].0 > key.0 {
                order[j] = order[j - 1];
                j -= 1;
            }
            order[j] = key;
        }

        let mut p_idx = 0usize;
        let mut c_idx = 0usize;
        while p_idx < self.props.len() || c_idx < char_count {
            let char_z = (c_idx < char_count).then(|| order[c_idx]);
            let draw_prop = match (self.props.get(p_idx), char_z) {
                (Some(p), Some((cz, _))) => p.z_index <= cz,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => break,
            };
            if draw_prop {
                self.props[p_idx].draw(batcher, cam_pos, cam_zoom, hud_zoom);
                p_idx += 1;
            } else {
                let ch = chars[order[c_idx].1];
                ch.draw(batcher, cam_pos, cam_zoom);
                c_idx += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fnfrs_core::schema::character::AnimationData;
    use fnfrs_core::schema::stage::{ScaleSpec, StageData};

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn simple_prop(name: Option<&str>, z_index: i32, asset_path: &str) -> StageDataProp {
        StageDataProp {
            name: name.map(str::to_owned),
            asset_path: asset_path.to_owned(),
            position: [0.0, 0.0],
            z_index,
            is_pixel: false,
            flip_x: false,
            flip_y: false,
            scale: ScaleSpec::Scalar(1.0),
            alpha: 1.0,
            dance_every: 0.0,
            scroll: [1.0, 1.0],
            animations: Vec::new(),
            starting_animation: None,
            anim_type: "sparrow".to_owned(),
            angle: 0.0,
            blend: String::new(),
            color: "#FFFFFF".to_owned(),
            atlas_settings: None,
        }
    }

    #[test]
    fn sorts_stage_props_by_z_index_and_preserves_ties() {
        let mut data = StageData {
            version: "1.0.0".to_owned(),
            name: "Test Stage".to_owned(),
            props: vec![
                simple_prop(Some("fg"), 300, "#FFFFFF"),
                simple_prop(Some("mid_a"), 100, "#112233"),
                simple_prop(Some("bg"), 10, "#000000"),
                simple_prop(Some("mid_b"), 100, "#445566"),
            ],
            characters: StageDataCharacters::default(),
            camera_zoom: 0.9,
            directory: "shared".to_owned(),
        };
        data.characters.bf.position = [950.0, 880.0];
        data.characters.bf.z_index = 200;

        let stage = Stage::from_data_headless("testStage", data);
        let names: Vec<&str> = stage
            .props
            .iter()
            .filter_map(|p| p.name.as_deref())
            .collect();
        assert_eq!(names, vec!["bg", "mid_a", "mid_b", "fg"]);
        assert_eq!(stage.get_named_prop("mid_b").map(|p| p.z_index), Some(100));
        assert!(approx_eq(stage.bf_placement().position[0], 950.0));
        assert_eq!(stage.bf_placement().z_index, 200);
    }

    #[test]
    fn bopper_dance_every_toggles_and_respects_cadence() {
        let mut bop_alt = Bopper::new_headless(2.0, &["danceLeft", "danceRight"], false);
        assert!(bop_alt.should_alternate);

        bop_alt.on_beat_hit(0);
        assert_eq!(bop_alt.current_animation(), "danceLeft");

        // Beat 1 should not trigger when dance_every == 2.0.
        bop_alt.on_beat_hit(1);
        assert_eq!(bop_alt.current_animation(), "danceLeft");

        // Beat 2 toggles to danceRight.
        bop_alt.on_beat_hit(2);
        assert_eq!(bop_alt.current_animation(), "danceRight");

        // Sub-beat step dancing (`dance_every = 0.5` -> every 2 steps).
        let mut fast = Bopper::new_headless(0.5, &["danceLeft", "danceRight"], true);
        fast.on_step_hit(0);
        assert_eq!(fast.current_animation(), "danceRight");
        fast.on_step_hit(1);
        assert_eq!(fast.current_animation(), "danceRight");
        fast.on_step_hit(2);
        assert_eq!(fast.current_animation(), "danceLeft");
    }

    #[test]
    fn parses_solid_colors_and_blend_modes() {
        let c6 = parse_stage_color("#8E9191");
        assert!(approx_eq(c6[0], 0x8E as f32 / 255.0));
        assert!(approx_eq(c6[1], 0x91 as f32 / 255.0));
        assert!(approx_eq(c6[2], 0x91 as f32 / 255.0));
        assert!(approx_eq(c6[3], 1.0));

        let c3 = parse_stage_color("#F08");
        assert!(approx_eq(c3[0], 1.0));
        assert!(approx_eq(c3[1], 0.0));
        assert!(approx_eq(c3[2], 8.0 / 15.0));
        assert!(approx_eq(c3[3], 1.0));

        let c8 = parse_stage_color("#80FF0000");
        assert!(approx_eq(c8[0], 1.0));
        assert!(approx_eq(c8[1], 0.0));
        assert!(approx_eq(c8[2], 0.0));
        assert!(approx_eq(c8[3], 0x80 as f32 / 255.0));

        assert_eq!(parse_blend_mode("add"), BlendMode::Add);
        assert_eq!(parse_blend_mode("SCREEN"), BlendMode::Screen);
        assert_eq!(parse_blend_mode(""), BlendMode::Normal);

        let mut solid_prop = simple_prop(Some("sky"), 0, "#FF8000");
        solid_prop.scale = ScaleSpec::Pair([4000.0, 3000.0]);
        solid_prop.scroll = [0.0, 0.0];
        let parsed = StageProp::from_data_headless(&solid_prop);
        assert!(approx_eq(parsed.color[0], 1.0));
        assert!(approx_eq(parsed.color[1], 0x80 as f32 / 255.0));
        assert!(approx_eq(parsed.color[2], 0.0));

        // Animated stage prop with startingAnimation = danceLeft toggles to danceRight.
        let mut anim_prop = simple_prop(Some("speakers"), 50, "gfDance");
        anim_prop.dance_every = 1.0;
        anim_prop.starting_animation = Some("danceLeft".to_owned());
        anim_prop.animations = vec![
            AnimationData {
                name: "danceLeft".to_owned(),
                prefix: Some("GF Dancing Beat".to_owned()),
                asset_path: None,
                offsets: [0.0, 0.0],
                looped: false,
                flip_x: false,
                flip_y: false,
                frame_rate: 24,
                frame_indices: Vec::new(),
                anim_type: "framelabel".to_owned(),
                render_type: String::new(),
                atlas_settings: None,
            },
            AnimationData {
                name: "danceRight".to_owned(),
                prefix: Some("GF Dancing Beat".to_owned()),
                asset_path: None,
                offsets: [0.0, 0.0],
                looped: false,
                flip_x: false,
                flip_y: false,
                frame_rate: 24,
                frame_indices: Vec::new(),
                anim_type: "framelabel".to_owned(),
                render_type: String::new(),
                atlas_settings: None,
            },
        ];
        let mut prop = StageProp::from_data_headless(&anim_prop);
        assert_eq!(
            prop.bopper.as_ref().map(Bopper::current_animation),
            Some("danceLeft")
        );
        prop.on_beat_hit(0);
        assert_eq!(
            prop.bopper.as_ref().map(Bopper::current_animation),
            Some("danceRight")
        );
    }
}
