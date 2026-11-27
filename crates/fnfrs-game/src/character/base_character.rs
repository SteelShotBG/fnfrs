use std::fmt::Write as _;
use std::fs;

use fnfrs_core::constants::{KEY_COUNT, MS_PER_SEC, STEPS_PER_BEAT};
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::character::{CharacterData, CharacterRenderType, DeathData, HealthIconData};
use fnfrs_core::schema::stage::StageDataCharacter;
use fnfrs_render::{AtlasFrame, BlendMode, GpuContext, SpriteBatcher};

use super::atlas_bank::{
    build_loaded_anim, dir_upper, draw_atlas_frame, format_stack_join, load_animate_source,
    load_packer_source, load_sparrow_source, make_loaded_anim_entry, AnimBank, AnimFrames,
    AtlasDrawParams, LoadedAtlasSource, StackBuf,
};
use super::CharacterKind;
use crate::input::NoteDirection;
#[derive(Debug)]
pub struct BaseCharacter {
    pub id: String,
    pub name: String,
    pub kind: CharacterKind,
    pub render_type: CharacterRenderType,

    pub position: [f32; 2],
    pub original_position: [f32; 2],
    pub base_scale: f32,
    pub scale: f32,
    pub global_offsets: [f32; 2],
    pub data_camera_offsets: [f32; 2],
    pub stage_camera_offsets: [f32; 2],
    pub camera_focus_point: [f32; 2],
    pub scroll_factor: [f32; 2],
    pub z_index: i32,
    pub alpha: f32,
    pub visible: bool,
    pub color: [f32; 4],
    pub angle: f32,
    pub is_pixel: bool,
    pub adjust_color: Option<[f32; 4]>,
    pub flip_x: bool,
    pub data_flip_x: bool,
    pub dance_every: f32,
    pub data_dance_every: f32,
    pub sing_time: f32,
    pub hold_timer: f32,
    pub is_dead: bool,
    pub should_bop: bool,
    pub should_alternate: bool,
    pub has_danced: bool,
    pub can_play_other_anims: bool,
    pub is_bloody: bool,
    pub idle_suffix: String,
    pub starting_animation: String,
    pub health_icon: Option<HealthIconData>,
    pub death_data: Option<DeathData>,
    pub combo_note_counts: Vec<u32>,
    pub drop_note_counts: Vec<u32>,

    ref_unscaled_size: [f32; 2],
    hitbox_size: [f32; 2],
    bank: AnimBank,
    idle_anim: Option<usize>,
    dance_left_anim: Option<usize>,
    dance_right_anim: Option<usize>,
    sing_anims: [Option<usize>; KEY_COUNT],
    sing_miss_anims: [Option<usize>; KEY_COUNT],
    last_dance_step: Option<i32>,
}

impl BaseCharacter {
    /// Loads `data/characters/<id>.json` and builds a [`BaseCharacter`] via `assets` & `gpu`.
    pub fn load_by_id(
        id: &str,
        kind: CharacterKind,
        assets: &AssetLocator,
        gpu: &mut GpuContext,
    ) -> Option<Self> {
        let key = format!("characters/{id}");
        let path = assets.data_json(&key);
        let mut raw = fs::read_to_string(&path).ok()?;
        let trimmed = raw.trim_end();
        if let Some(pos) = trimmed.rfind('}') {
            raw.truncate(pos + 1);
        }
        let data: CharacterData = serde_json::from_str(&raw).ok()?;
        Some(Self::load_from_assets(id, kind, data, assets, gpu))
    }

    /// Builds a [`BaseCharacter`] from parsed [`CharacterData`], loading all primary
    /// and secondary (Multi-Sparrow / Multi-Animate) atlases into `gpu`.
    pub fn load_from_assets(
        id: &str,
        kind: CharacterKind,
        data: CharacterData,
        assets: &AssetLocator,
        gpu: &mut GpuContext,
    ) -> Self {
        let mut bank = AnimBank::new();
        let mut path_to_atlas: HashMap<String, usize> = HashMap::new();

        // Load primary atlas according to `data.render_type`.
        let primary = match data.render_type {
            CharacterRenderType::Packer => load_packer_source(assets, gpu, &data.asset_path),
            CharacterRenderType::AnimateAtlas | CharacterRenderType::MultiAnimateAtlas => {
                load_animate_source(
                    assets,
                    gpu,
                    &data.asset_path,
                    data.atlas_settings.as_ref(),
                )
            }
            CharacterRenderType::Sparrow
            | CharacterRenderType::MultiSparrow
            | CharacterRenderType::Custom => load_sparrow_source(assets, gpu, &data.asset_path),
        };

        let primary_idx = if let Some(src) = primary {
            let idx = bank.atlases.len();
            bank.atlases.push(src);
            path_to_atlas.insert(data.asset_path.clone(), idx);
            Some(idx)
        } else {
            None
        };

        // Load secondary atlases for MultiSparrow / MultiAnimateAtlas (or per-anim assetPath).
        for anim in &data.animations {
            let Some(sub_path) = anim.asset_path.as_deref().filter(|s| !s.is_empty()) else {
                continue;
            };
            if path_to_atlas.contains_key(sub_path) {
                continue;
            }
            let use_animate = anim.render_type.eq_ignore_ascii_case("animateatlas")
                || (data.render_type == CharacterRenderType::MultiAnimateAtlas
                    && !anim.render_type.eq_ignore_ascii_case("sparrow")
                    && !anim.render_type.eq_ignore_ascii_case("packer"));

            let sub_src = if use_animate {
                load_animate_source(
                    assets,
                    gpu,
                    sub_path,
                    anim.atlas_settings
                        .as_ref()
                        .or(data.atlas_settings.as_ref()),
                )
            } else if anim.render_type.eq_ignore_ascii_case("packer") {
                load_packer_source(assets, gpu, sub_path)
            } else {
                load_sparrow_source(assets, gpu, sub_path)
            };

            if let Some(src) = sub_src {
                let idx = bank.atlases.len();
                bank.atlases.push(src);
                path_to_atlas.insert(sub_path.to_owned(), idx);
            }
        }

        // Build animations, falling back across loaded atlases for MultiSparrow sheets.
        for anim in &data.animations {
            if let Some(sub_path) = anim.asset_path.as_deref().filter(|s| !s.is_empty()) {
                if let Some(&idx) = path_to_atlas.get(sub_path) {
                    if let Some(loaded) = build_loaded_anim(anim, &bank.atlases, idx) {
                        bank.anims.push(loaded);
                    }
                }
                continue;
            }

            if let Some(p_idx) = primary_idx {
                if let Some(mut loaded) = build_loaded_anim(anim, &bank.atlases, p_idx) {
                    if loaded.frames.frame_count() == 0 && bank.atlases.len() > 1 {
                        for alt_idx in 0..bank.atlases.len() {
                            if alt_idx == p_idx {
                                continue;
                            }
                            if let Some(alt_loaded) =
                                build_loaded_anim(anim, &bank.atlases, alt_idx)
                            {
                                if alt_loaded.frames.frame_count() > 0 {
                                    loaded = alt_loaded;
                                    break;
                                }
                            }
                        }
                    }
                    bank.anims.push(loaded);
                }
            }
        }

        Self::finish_init(id, kind, data, bank)
    }

    /// Creates a headless [`BaseCharacter`] from [`CharacterData`] without GPU textures
    /// (for tests and simulation without a window).
    #[must_use]
    pub fn from_data_headless(id: &str, kind: CharacterKind, data: CharacterData) -> Self {
        let mut bank = AnimBank::new();
        for anim in &data.animations {
            let frames = AnimFrames::Sheet {
                atlas_idx: 0,
                frames: vec![AtlasFrame {
                    name: anim.prefix.clone().unwrap_or_else(|| anim.name.clone()),
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 150,
                    frame_x: 0.0,
                    frame_y: 0.0,
                    frame_width: 100.0,
                    frame_height: 150.0,
                    rotated: false,
                    flip_x: false,
                    flip_y: false,
                }],
            };
            bank.anims.push(make_loaded_anim_entry(
                anim.name.clone(),
                anim.offsets,
                anim.looped,
                anim.flip_x,
                anim.flip_y,
                anim.frame_rate.max(1) as f32,
                frames,
            ));
        }
        Self::finish_init(id, kind, data, bank)
    }

    /// Creates a synthetic [`BaseCharacter`] with explicit hitbox size and animation list
    /// for unit testing positioning, offsets, dancing, and sing timers.
    #[must_use]
    pub fn new_synthetic(
        id: &str,
        kind: CharacterKind,
        unscaled_frame_size: [f32; 2],
        data: CharacterData,
    ) -> Self {
        let mut ch = Self::from_data_headless(id, kind, data);
        ch.ref_unscaled_size = unscaled_frame_size;
        ch.hitbox_size = [
            unscaled_frame_size[0] * ch.scale,
            unscaled_frame_size[1] * ch.scale,
        ];
        ch.reset_camera_focus_point();
        ch
    }

    fn finish_init(
        id: &str,
        kind: CharacterKind,
        data: CharacterData,
        mut bank: AnimBank,
    ) -> Self {
        bank.finalize_links();

        let idle_anim = bank.anim_index.get("idle").copied();
        let dance_left_anim = bank.anim_index.get("danceLeft").copied();
        let dance_right_anim = bank.anim_index.get("danceRight").copied();
        let should_alternate = dance_left_anim.is_some();

        let mut sing_anims = [None; KEY_COUNT];
        let mut sing_miss_anims = [None; KEY_COUNT];
        for dir in NoteDirection::ALL {
            let d = dir_upper(dir);
            let mut s_buf = [0u8; 32];
            if let Some(key) = format_stack_join(&mut s_buf, "sing", d) {
                sing_anims[dir.as_index()] = bank.anim_index.get(key).copied();
            }
            let mut m_buf = [0u8; 32];
            let mut sb = StackBuf::new(&mut m_buf);
            if write!(sb, "sing{d}miss").is_ok() {
                sing_miss_anims[dir.as_index()] = bank.anim_index.get(sb.as_str()).copied();
            }
        }

        let mut combo_note_counts = Vec::new();
        let mut drop_note_counts = Vec::new();
        for anim in &bank.anims {
            if let Some(rest) = anim.name.strip_prefix("combo") {
                if let Ok(n) = rest.parse::<u32>() {
                    combo_note_counts.push(n);
                }
            } else if let Some(rest) = anim.name.strip_prefix("drop") {
                if let Ok(n) = rest.parse::<u32>() {
                    drop_note_counts.push(n);
                }
            }
        }
        combo_note_counts.sort_unstable();
        drop_note_counts.sort_unstable();

        // Pick reference frame dimensions from idle/danceLeft/starting animation.
        let ref_anim_idx = if should_alternate {
            dance_left_anim
        } else {
            idle_anim
        }
        .or_else(|| bank.anim_index.get(&data.starting_animation).copied())
        .or(if bank.anims.is_empty() { None } else { Some(0) });

        let ref_unscaled_size = ref_anim_idx
            .and_then(|idx| bank.anims[idx].frames.first_source_size(&bank.atlases))
            .or_else(|| {
                bank.atlases
                    .first()
                    .and_then(LoadedAtlasSource::fallback_hitbox_size)
            })
            .unwrap_or([100.0, 150.0]);

        let base_scale = data.scale;
        let hitbox_size = [
            ref_unscaled_size[0] * base_scale,
            ref_unscaled_size[1] * base_scale,
        ];

        let flip_x = match kind {
            CharacterKind::Bf => !data.flip_x,
            CharacterKind::Dad | CharacterKind::Gf | CharacterKind::Other => data.flip_x,
        };

        let mut ch = Self {
            id: id.to_owned(),
            name: data.name,
            kind,
            render_type: data.render_type,
            position: [0.0, 0.0],
            original_position: [0.0, 0.0],
            base_scale,
            scale: base_scale,
            global_offsets: data.offsets,
            data_camera_offsets: data.camera_offsets,
            stage_camera_offsets: [0.0, 0.0],
            camera_focus_point: [0.0, 0.0],
            scroll_factor: [1.0, 1.0],
            z_index: 0,
            alpha: 1.0,
            visible: true,
            color: [1.0, 1.0, 1.0, 1.0],
            angle: 0.0,
            is_pixel: data.is_pixel,
            adjust_color: None,
            flip_x,
            data_flip_x: data.flip_x,
            dance_every: data.dance_every,
            data_dance_every: data.dance_every,
            sing_time: data.sing_time,
            hold_timer: 0.0,
            is_dead: false,
            should_bop: false,
            should_alternate,
            has_danced: false,
            can_play_other_anims: true,
            is_bloody: false,
            idle_suffix: String::new(),
            starting_animation: data.starting_animation,
            health_icon: data.health_icon,
            death_data: data.death,
            combo_note_counts,
            drop_note_counts,
            ref_unscaled_size,
            hitbox_size,
            bank,
            idle_anim,
            dance_left_anim,
            dance_right_anim,
            sing_anims,
            sing_miss_anims,
            last_dance_step: None,
        };

        ch.dance(true);
        if ch.bank.current_anim.is_none() && !ch.starting_animation.is_empty() {
            let start_name = ch.starting_animation.clone();
            ch.play_animation(&start_name, true, false);
        }
        ch.reset_camera_focus_point();
        ch
    }

    /// Offset from top-left `position` to ground origin (horizontal center, vertical bottom).
    #[inline]
    #[must_use]
    pub fn character_origin(&self) -> [f32; 2] {
        [self.hitbox_size[0] * 0.5, self.hitbox_size[1]]
    }

    /// Scaled hitbox dimensions `[width, height]`.
    #[inline]
    #[must_use]
    pub fn hitbox_size(&self) -> [f32; 2] {
        self.hitbox_size
    }

    /// World position of the character's feet (`position + character_origin`).
    #[inline]
    #[must_use]
    pub fn feet_position(&self) -> [f32; 2] {
        let origin = self.character_origin();
        [self.position[0] + origin[0], self.position[1] + origin[1]]
    }

    /// Places the character so its feet rest at `feet` (applying `global_offsets`).
    pub fn set_feet_position(&mut self, feet: [f32; 2]) {
        let origin = self.character_origin();
        self.position = [
            feet[0] - origin[0] + self.global_offsets[0],
            feet[1] - origin[1] + self.global_offsets[1],
        ];
        self.original_position = self.position;
        self.reset_camera_focus_point();
    }

    /// Updates `scale` while keeping feet anchored (`BaseCharacter.setScale`).
    pub fn set_scale(&mut self, scale: f32) {
        let feet = self.feet_position();
        self.scale = scale;
        self.hitbox_size = [
            self.ref_unscaled_size[0] * scale,
            self.ref_unscaled_size[1] * scale,
        ];
        let origin = self.character_origin();
        self.position = [
            feet[0] - origin[0] + self.global_offsets[0],
            feet[1] - origin[1] + self.global_offsets[1],
        ];
    }

    /// Positions and configures this character from stage placement data (`Stage.addCharacter`).
    pub fn place_on_stage(&mut self, placement: &StageDataCharacter) {
        self.flip_x = match self.kind {
            CharacterKind::Bf => !self.data_flip_x,
            CharacterKind::Dad | CharacterKind::Gf | CharacterKind::Other => self.data_flip_x,
        };
        self.reset_character(false);

        self.z_index = placement.z_index;
        let origin = self.character_origin();
        self.position = [
            placement.position[0] - origin[0],
            placement.position[1] - origin[1],
        ];
        let final_scale = self.base_scale * placement.scale;
        self.set_scale(final_scale);
        self.original_position = self.position;

        self.stage_camera_offsets = placement.camera_offsets;
        self.scroll_factor = placement.scroll;
        self.alpha = placement.alpha;
        self.angle = placement.angle;
        self.reset_camera_focus_point();
    }

    /// Resets position, dance cadence, idle pose, and optionally `camera_focus_point`.
    pub fn reset_character(&mut self, reset_camera: bool) {
        self.position = self.original_position;
        self.dance_every = self.data_dance_every;
        self.hold_timer = 0.0;
        self.has_danced = false;
        self.can_play_other_anims = true;
        self.is_bloody = false;
        self.last_dance_step = None;
        self.dance(true);
        if reset_camera {
            self.reset_camera_focus_point();
        }
    }

    /// Recomputes `camera_focus_point` from `original_position`, hitbox center, and offsets.
    pub fn reset_camera_focus_point(&mut self) {
        let cx = self.original_position[0] + self.hitbox_size[0] * 0.5;
        let cy = self.original_position[1] + self.hitbox_size[1] * 0.5;
        self.camera_focus_point = [
            cx + self.data_camera_offsets[0] + self.stage_camera_offsets[0],
            cy + self.data_camera_offsets[1] + self.stage_camera_offsets[1],
        ];
    }

    /// Returns the world point the game camera focuses on for this character.
    #[inline]
    #[must_use]
    pub fn get_camera_focus_point(&self) -> [f32; 2] {
        self.camera_focus_point
    }

    /// Returns `(current_anim_index, current_frame_index, current_anim_offsets)` for trail effects.
    #[inline]
    #[must_use]
    pub fn active_frame_snapshot(&self) -> Option<(usize, usize, [f32; 2])> {
        let idx = self.bank.current_anim?;
        let anim = self.bank.anims.get(idx)?;
        Some((idx, self.bank.current_frame, anim.offsets))
    }

    /// Draws an explicit `(anim_idx, frame_idx)` snapshot at `pos` with `alpha` (e.g. `FunkTrail`).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_snapshot_ghost(
        &self,
        batcher: &mut SpriteBatcher,
        cam_pos: [f32; 2],
        cam_zoom: f32,
        anim_idx: usize,
        frame_idx: usize,
        pos: [f32; 2],
        anim_offsets: [f32; 2],
        flip_x: bool,
        alpha: f32,
    ) {
        let Some(anim) = self.bank.anims.get(anim_idx) else {
            return;
        };
        let AnimFrames::Sheet { atlas_idx, frames } = &anim.frames else {
            return;
        };
        if frames.is_empty() {
            return;
        }
        let frame = &frames[frame_idx.min(frames.len() - 1)];
        let Some(atlas) = self.bank.atlases.get(*atlas_idx) else {
            return;
        };
        let (texture, tex_size) = match atlas {
            LoadedAtlasSource::Sparrow {
                texture, tex_size, ..
            }
            | LoadedAtlasSource::Packer {
                texture, tex_size, ..
            } => (*texture, *tex_size),
            LoadedAtlasSource::Animate { .. } => return,
        };
        let params = AtlasDrawParams {
            pos,
            scale: [self.scale, self.scale],
            anim_offsets,
            global_offsets: self.global_offsets,
            scroll_factor: self.scroll_factor,
            cam_pos,
            cam_zoom,
            angle_deg: self.angle,
            flip_x,
            flip_y: false,
            color: [1.0, 1.0, 1.0, 1.0],
            alpha,
            blend: BlendMode::Normal,
            is_pixel: self.is_pixel,
            adjust_color: self.adjust_color,
            wiggle: None,
            ref_unscaled_size: Some(self.ref_unscaled_size),
        };
        draw_atlas_frame(
            batcher,
            texture,
            tex_size,
            frame,
            &params,
            anim.flip_x,
            anim.flip_y,
        );
    }

    /// Returns the active animation name, or `""` if none.
    #[inline]
    #[must_use]
    pub fn current_animation(&self) -> &str {
        self.bank.current_name()
    }

    /// Returns the active animation's `[x, y]` offset.
    #[inline]
    #[must_use]
    pub fn current_anim_offsets(&self) -> [f32; 2] {
        self.bank.current_offsets()
    }

    /// Returns `true` if the active animation has reached its final frame.
    #[inline]
    #[must_use]
    pub fn is_animation_finished(&self) -> bool {
        self.bank.finished
    }

    /// Returns `true` when currently playing a `sing*` (non-`-end`) animation.
    #[inline]
    #[must_use]
    pub fn is_singing(&self) -> bool {
        self.bank
            .current_anim_meta()
            .is_some_and(|anim| anim.is_sing)
    }

    /// Returns `true` if `name` is present in this character's animation table.
    #[inline]
    #[must_use]
    pub fn has_animation(&self, name: &str) -> bool {
        self.bank.anim_index.contains_key(name)
    }

    /// Computes the draw-space world top-left position for the current frame after
    /// `anim_offsets`, `global_offsets`, and `scale` (`Bopper.getScreenPosition`).
    #[inline]
    #[must_use]
    pub fn effective_draw_origin(&self) -> [f32; 2] {
        let anim_off = self.current_anim_offsets();
        [
            self.position[0] - (anim_off[0] - self.global_offsets[0]) * self.scale,
            self.position[1] - (anim_off[1] - self.global_offsets[1]) * self.scale,
        ]
    }

    /// Sets the idle suffix (e.g. `"-alt"`) and immediately re-dances (`Bopper.set_idleSuffix`).
    pub fn set_idle_suffix(&mut self, suffix: &str) {
        self.idle_suffix.clear();
        self.idle_suffix.push_str(suffix);
        self.dance(false);
    }

    /// Plays an animation by name with `Bopper.correctAnimationName` `-suffix` fallback
    /// and zero heap allocations. Also mirrors `tankman-bloody.hxc` (`redheadsAnim` -> `-bloody`).
    pub fn play_animation(&mut self, name: &str, restart: bool, ignore_other: bool) {
        if !self.can_play_other_anims {
            let same_restart = self.current_animation() == name && restart;
            if !same_restart
                && !name.starts_with("sing")
                && !name.starts_with("shoot")
                && !name.starts_with("cheer")
            {
                return;
            }
        }
        let resolved = if self.is_bloody {
            let mut buf = [0u8; 96];
            format_stack_join(&mut buf, name, "-bloody")
                .and_then(|bloody| self.bank.resolve_with_fallback(bloody))
                .or_else(|| self.bank.resolve_with_fallback(name))
        } else {
            self.bank.resolve_with_fallback(name)
        };
        if let Some(idx) = resolved {
            self.play_anim_by_index(idx, restart, ignore_other);
        }
        if name == "redheadsAnim" {
            self.is_bloody = true;
        }
    }

    fn play_anim_by_index(&mut self, idx: usize, restart: bool, ignore_other: bool) {
        self.bank.play_index(idx, restart);
        if ignore_other {
            self.can_play_other_anims = false;
        }
    }

    /// Plays `sing{DIR}[miss][-{suffix}]` with zero heap allocations.
    pub fn play_sing_animation(&mut self, dir: NoteDirection, is_miss: bool, suffix: &str) {
        if suffix.is_empty() && !self.is_bloody {
            let pre = if is_miss {
                self.sing_miss_anims[dir.as_index()]
            } else {
                self.sing_anims[dir.as_index()]
            };
            if let Some(idx) = pre {
                // `sing` is in `ignoreExclusionPref` so it overrides `canPlayOtherAnims`.
                self.play_anim_by_index(idx, true, false);
                self.hold_timer = 0.0;
                return;
            }
        }

        let mut buf = [0u8; 96];
        let mut sb = StackBuf::new(&mut buf);
        let d = dir_upper(dir);
        let miss_part = if is_miss { "miss" } else { "" };
        let res = match (suffix.is_empty(), self.is_bloody) {
            (true, false) => write!(sb, "sing{d}{miss_part}"),
            (false, false) => write!(sb, "sing{d}{miss_part}-{suffix}"),
            (true, true) => write!(sb, "sing{d}{miss_part}-bloody"),
            (false, true) => write!(sb, "sing{d}{miss_part}-{suffix}-bloody"),
        };
        if res.is_ok() {
            let target = sb.as_str();
            if let Some(idx) = self.bank.resolve_with_fallback(target) {
                self.play_anim_by_index(idx, true, false);
                self.hold_timer = 0.0;
                return;
            }
        }
        if self.is_bloody {
            let mut fallback_buf = [0u8; 96];
            let mut fb = StackBuf::new(&mut fallback_buf);
            let fb_res = if suffix.is_empty() {
                write!(fb, "sing{d}{miss_part}")
            } else {
                write!(fb, "sing{d}{miss_part}-{suffix}")
            };
            if fb_res.is_ok() {
                if let Some(idx) = self.bank.resolve_with_fallback(fb.as_str()) {
                    self.play_anim_by_index(idx, true, false);
                    self.hold_timer = 0.0;
                }
            }
        }
    }

    /// Plays `combo{count}` if registered for GF-style characters.
    pub fn play_combo_animation(&mut self, combo_count: u32) {
        let mut buf = [0u8; 32];
        let mut sb = StackBuf::new(&mut buf);
        if write!(sb, "combo{combo_count}").is_ok() {
            if let Some(&idx) = self.bank.anim_index.get(sb.as_str()) {
                self.play_anim_by_index(idx, true, true);
            }
        }
    }

    /// Plays the highest matching `drop{threshold}` animation when dropping a combo >= threshold.
    pub fn play_combo_drop_animation(&mut self, combo_count: u32) {
        let mut chosen: Option<u32> = None;
        for &count in &self.drop_note_counts {
            if combo_count >= count {
                chosen = Some(count);
            }
        }
        if let Some(threshold) = chosen {
            let mut buf = [0u8; 32];
            let mut sb = StackBuf::new(&mut buf);
            if write!(sb, "drop{threshold}").is_ok() {
                if let Some(&idx) = self.bank.anim_index.get(sb.as_str()) {
                    self.play_anim_by_index(idx, true, true);
                }
            }
        }
    }

    /// Plays `danceLeft`/`danceRight` (when alternating) or `idle` (`BaseCharacter.dance`).
    pub fn dance(&mut self, force: bool) {
        if self.is_dead {
            return;
        }
        if !force {
            if self.is_singing() {
                return;
            }
            if let Some(meta) = self.bank.current_anim_meta() {
                if !meta.is_idle_or_dance && !self.bank.finished {
                    return;
                }
            }
        }

        if self.should_alternate {
            let play_right = self.has_danced;
            let played = if self.idle_suffix.is_empty() {
                let target = if play_right {
                    self.dance_right_anim
                } else {
                    self.dance_left_anim
                };
                if let Some(idx) = target {
                    self.play_anim_by_index(idx, force, false);
                    true
                } else {
                    false
                }
            } else {
                let mut buf = [0u8; 96];
                let base = if play_right { "danceRight" } else { "danceLeft" };
                if let Some(name) = format_stack_join(&mut buf, base, &self.idle_suffix) {
                    if let Some(idx) = self.bank.resolve_with_fallback(name) {
                        self.play_anim_by_index(idx, force, false);
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
        } else if self.idle_suffix.is_empty() && !self.is_bloody {
            if let Some(idx) = self.idle_anim {
                self.play_anim_by_index(idx, force, false);
            }
        } else {
            let mut buf = [0u8; 96];
            let mut sb = StackBuf::new(&mut buf);
            let res = match (self.idle_suffix.is_empty(), self.is_bloody) {
                (true, false) => write!(sb, "idle"),
                (false, false) => write!(sb, "idle{}", self.idle_suffix),
                (true, true) => write!(sb, "idle-bloody"),
                (false, true) => write!(sb, "idle{}-bloody", self.idle_suffix),
            };
            let resolved = if res.is_ok() {
                self.bank.resolve_with_fallback(sb.as_str())
            } else {
                None
            };
            if let Some(idx) = resolved.or(self.idle_anim) {
                self.play_anim_by_index(idx, force, false);
            }
        }
    }

    /// Resets `hold_timer` (e.g. when player presses a note key).
    #[inline]
    pub fn reset_hold_timer(&mut self) {
        self.hold_timer = 0.0;
    }

    /// Beat callback matching `BaseCharacter` / `Bopper` dance cadence.
    pub fn on_beat_hit(&mut self, beat: i32, is_holding_note: bool) {
        if self.dance_every <= 0.0 {
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
            step.rem_euclid(steps_per_dance) == 0
        };
        if should_dance && self.last_dance_step != Some(step) {
            self.last_dance_step = Some(step);
            if self.kind == CharacterKind::Bf && is_holding_note && self.is_singing() {
                return;
            }
            self.dance(self.should_bop);
        }
    }

    /// Step callback supporting fractional `dance_every` down to `0.25` (`Bopper.onStepHit`).
    pub fn on_step_hit(&mut self, step: i32, is_holding_note: bool) {
        if self.dance_every <= 0.0 {
            return;
        }
        let steps_per_dance = (self.dance_every * STEPS_PER_BEAT as f32).round() as i32;
        if steps_per_dance <= 0 {
            return;
        }
        if step.rem_euclid(steps_per_dance) == 0 && self.last_dance_step != Some(step) {
            self.last_dance_step = Some(step);
            if self.kind == CharacterKind::Bf && is_holding_note && self.is_singing() {
                return;
            }
            self.dance(self.should_bop);
        }
    }

    /// Zero-allocation per-frame character update (`BaseCharacter.onUpdate`).
    ///
    /// - Advances current animation frames.
    /// - Transitions `anim` -> `anim-hold` when finished (if available).
    /// - Reverts `sing*` animations to `sing*-end` or `dance(true)` once
    ///   `hold_timer > sing_time * step_length_s` (doubled on miss) when not holding.
    pub fn update(&mut self, elapsed_s: f32, beat_length_ms: f64, is_holding: bool) {
        let just_finished = self.bank.update(elapsed_s);
        if just_finished {
            if !self.can_play_other_anims {
                self.can_play_other_anims = true;
            }
            let should_redance = self.bank.current_anim_meta().is_some_and(|meta| {
                (meta.is_end_suffix && !meta.is_idle_or_dance) || meta.is_combo_or_drop
            });
            if should_redance {
                self.dance(true);
            }
        }

        if self.is_dead {
            return;
        }

        // Transition finished non-`-hold` animations into their `-hold` loop if defined.
        if self.bank.finished {
            let hold_idx = self
                .bank
                .current_anim_meta()
                .filter(|meta| !meta.is_hold_suffix)
                .and_then(|meta| meta.hold_anim);
            if let Some(idx) = hold_idx {
                self.play_anim_by_index(idx, true, false);
            }
        }

        // Sing hold timer revert (`BaseCharacter.hx` lines 426-469).
        let sing_info = self
            .bank
            .current_anim_meta()
            .filter(|meta| meta.is_sing)
            .map(|meta| (meta.is_miss, meta.end_anim));

        if let Some((is_miss, end_anim)) = sing_info {
            self.hold_timer += elapsed_s.max(0.0);
            let step_ms = beat_length_ms.max(1.0) / f64::from(STEPS_PER_BEAT);
            let mut sing_time_sec = self.sing_time * (step_ms / MS_PER_SEC) as f32;
            if is_miss {
                sing_time_sec *= 2.0;
            }
            let should_stop = match self.kind {
                CharacterKind::Bf => !is_holding,
                CharacterKind::Dad | CharacterKind::Gf | CharacterKind::Other => true,
            };
            if self.hold_timer > sing_time_sec && should_stop {
                self.hold_timer = 0.0;
                if let Some(end_idx) = end_anim {
                    self.play_anim_by_index(end_idx, false, false);
                } else {
                    self.dance(true);
                }
            }
        } else {
            self.hold_timer = 0.0;
        }
    }

    /// Zero-allocation character draw into `batcher` using camera scroll & zoom.
    pub fn draw(&self, batcher: &mut SpriteBatcher, cam_pos: [f32; 2], cam_zoom: f32) {
        if !self.visible || self.alpha <= 0.0 {
            return;
        }
        let params = AtlasDrawParams {
            pos: self.position,
            scale: [self.scale, self.scale],
            anim_offsets: self.current_anim_offsets(),
            global_offsets: self.global_offsets,
            scroll_factor: self.scroll_factor,
            cam_pos,
            cam_zoom,
            angle_deg: self.angle,
            flip_x: self.flip_x,
            flip_y: false,
            color: self.color,
            alpha: self.alpha,
            blend: BlendMode::Normal,
            is_pixel: self.is_pixel,
            adjust_color: self.adjust_color,
            wiggle: None,
            ref_unscaled_size: Some(self.ref_unscaled_size),
        };
        self.bank.draw_current(batcher, &params);
    }
}

