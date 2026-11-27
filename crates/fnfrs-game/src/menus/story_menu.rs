//! V-Slice Story Mode Menu (`StoryMenuState.hx`, `Level.hx`, `LevelTitle.hx`, `LevelProp.hx`).

use std::fmt::Write as _;
use std::fs;

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::save::SaveData;
use fnfrs_core::schema::level::{LevelData, LevelPropData};
use fnfrs_core::schema::song::SongMetadata;
use fnfrs_render::{
    AtlasFrame, BitmapFont, BlendMode, FilterMode, GpuContext, SpriteBatcher, SpriteInstance,
    StackTextBuffer, TextAlign, TextStyle, TextureHandle,
};

use crate::camera::{smooth_lerp_precision, FlxEase};
use crate::stage::parse_stage_color;

use super::{
    draw_menu_sparrow_frame, level_sort_key, load_menu_image, load_menu_sparrow, MenuCommand,
    MenuInputState,
};

const STORY_DIFFS: [&str; 3] = ["easy", "normal", "hard"];
const INTRO_BPM: f32 = 102.0;
const BANNER_Y: f32 = 56.0;
const BANNER_HEIGHT: f32 = 400.0;
const SELECTED_TITLE_Y: f32 = 480.0;
const TITLE_GAP: f32 = 20.0;
const MIN_ABOVE_STEP: f32 = 125.0;
const INITIAL_TITLE_Y: f32 = BANNER_Y + BANNER_HEIGHT + 10.0;
const CONFIRM_HOLD_S: f32 = 1.0;
const CONFIRM_FADE_S: f32 = 0.2;

#[derive(Debug)]
struct StoryPropSprite {
    tex: Option<(TextureHandle, [f32; 2])>,
    idle_frames: Vec<AtlasFrame>,
    idle_offset: [f32; 2],
    confirm_frames: Vec<AtlasFrame>,
    confirm_offset: [f32; 2],
    dance_left_frames: Vec<AtlasFrame>,
    dance_left_offset: [f32; 2],
    dance_right_frames: Vec<AtlasFrame>,
    dance_right_offset: [f32; 2],
    has_dance_lr: bool,
    dance_every: f32,
    offsets: [f32; 2],
    scale: f32,
    alpha: f32,
    is_pixel: bool,
    flip_x: bool,
    flip_y: bool,
    fps: f32,
}

#[derive(Debug)]
struct StoryWeekEntry {
    id: String,
    id_upper: String,
    name: String,
    songs: Vec<String>,
    song_names: Vec<String>,
    difficulties: Vec<String>,
    is_simple_bg: bool,
    bg_color: [f32; 4],
    bg_asset: String,
    title_tex: Option<(TextureHandle, [f32; 2])>,
    y: f32,
    target_y: f32,
    prop_specs: Vec<LevelPropData>,
    props: Vec<StoryPropSprite>,
}

#[derive(Debug)]
struct BgColorTween {
    from: [f32; 4],
    to: [f32; 4],
    elapsed_s: f32,
    duration_s: f32,
}

#[derive(Debug)]
struct BgImageFade {
    old_tex: (TextureHandle, [f32; 2]),
    elapsed_s: f32,
    duration_s: f32,
}

#[derive(Debug)]
pub struct StoryMenuScreenState {
    white_tex: TextureHandle,
    weeks: Vec<StoryWeekEntry>,
    selected_week: usize,
    loaded_props_week: Option<usize>,
    available_diffs: Vec<String>,
    current_difficulty: String,

    active_bg_color: [f32; 4],
    bg_color_tween: Option<BgColorTween>,
    active_bg_tex: Option<(TextureHandle, [f32; 2])>,
    old_bg_fade: Option<BgImageFade>,

    high_score: i64,
    high_score_lerp: f64,
    saved_level_scores: Vec<(String, String, i64)>,

    arrows_tex: Option<(TextureHandle, [f32; 2])>,
    left_idle: Option<AtlasFrame>,
    left_press: Option<AtlasFrame>,
    right_idle: Option<AtlasFrame>,
    right_press: Option<AtlasFrame>,
    diff_textures: [Option<(TextureHandle, [f32; 2])>; 3],
    diff_tween_s: f32,

    anim_time_s: f32,
    beat_timer_s: f32,
    beat_count: u32,
    confirm_elapsed_s: f32,
    confirming: bool,
    confirm_timer_s: f32,
}

fn collect_prop_frames(
    atlas: &fnfrs_render::SparrowAtlas,
    prefix: &str,
    indices: &[usize],
) -> Vec<AtlasFrame> {
    let matched: Vec<AtlasFrame> = atlas.frames_by_prefix(prefix).cloned().collect();
    if indices.is_empty() {
        return matched;
    }
    indices
        .iter()
        .filter_map(|&idx| matched.get(idx).cloned())
        .collect()
}

fn load_week_props(
    specs: &[LevelPropData],
    assets: &AssetLocator,
    gpu: &mut GpuContext,
) -> Vec<StoryPropSprite> {
    let mut props = Vec::with_capacity(specs.len());
    for prop in specs {
        let loaded = load_menu_sparrow(assets, gpu, &prop.asset_path);
        let idle_anim = prop
            .animations
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case("idle"));
        let confirm_anim = prop
            .animations
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case("confirm"));
        let dl_anim = prop
            .animations
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case("danceLeft"));
        let dr_anim = prop
            .animations
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case("danceRight"));
        let start_anim = if !prop.starting_animation.is_empty() {
            prop.animations
                .iter()
                .find(|a| a.name.eq_ignore_ascii_case(&prop.starting_animation))
        } else {
            None
        };
        let fallback_anim = start_anim.or_else(|| prop.animations.first());

        let ref_anim = dl_anim.or(idle_anim).or(fallback_anim);
        let fps = ref_anim.map_or(24.0, |a| a.frame_rate.max(1) as f32);

        let (tex, idle_frames, confirm_frames, dance_left_frames, dance_right_frames, has_dance_lr) =
            match loaded {
                Some((t, sz, atlas)) => {
                    let idle_src = idle_anim.or(fallback_anim);
                    let idle_pfx = idle_src.and_then(|a| a.prefix.as_deref()).unwrap_or("idle");
                    let idle_indices = idle_src.map_or(&[][..], |a| a.frame_indices.as_slice());
                    let mut idle_f = collect_prop_frames(&atlas, idle_pfx, idle_indices);
                    if idle_f.is_empty() {
                        idle_f = atlas.frames().to_vec();
                    }

                    let conf_f = confirm_anim
                        .and_then(|a| a.prefix.as_deref())
                        .map(|pfx| {
                            collect_prop_frames(
                                &atlas,
                                pfx,
                                confirm_anim.map_or(&[][..], |a| a.frame_indices.as_slice()),
                            )
                        })
                        .unwrap_or_default();

                    let dl_f = dl_anim
                        .map(|a| {
                            let pfx = a.prefix.as_deref().unwrap_or("idle");
                            collect_prop_frames(&atlas, pfx, &a.frame_indices)
                        })
                        .unwrap_or_default();
                    let dr_f = dr_anim
                        .map(|a| {
                            let pfx = a.prefix.as_deref().unwrap_or("idle");
                            collect_prop_frames(&atlas, pfx, &a.frame_indices)
                        })
                        .unwrap_or_default();
                    let has_dlr = !dl_f.is_empty() && !dr_f.is_empty();
                    (Some((t, sz)), idle_f, conf_f, dl_f, dr_f, has_dlr)
                }
                None => (None, Vec::new(), Vec::new(), Vec::new(), Vec::new(), false),
            };
        let eff_scale = prop.scale * if prop.is_pixel { 6.0 } else { 1.0 };
        props.push(StoryPropSprite {
            tex,
            idle_frames,
            idle_offset: idle_anim.or(fallback_anim).map_or([0.0, 0.0], |a| a.offsets),
            confirm_frames,
            confirm_offset: confirm_anim.map_or([0.0, 0.0], |a| a.offsets),
            dance_left_frames,
            dance_left_offset: dl_anim.map_or([0.0, 0.0], |a| a.offsets),
            dance_right_frames,
            dance_right_offset: dr_anim.map_or([0.0, 0.0], |a| a.offsets),
            has_dance_lr,
            dance_every: prop.dance_every,
            offsets: prop.offsets,
            scale: eff_scale,
            alpha: prop.alpha,
            is_pixel: prop.is_pixel,
            flip_x: prop.flip_x,
            flip_y: prop.flip_y,
            fps,
        });
    }
    props
}

fn read_song_meta(assets: &AssetLocator, song_id: &str, var: Option<&str>) -> Option<SongMetadata> {
    let path = assets.song_metadata(song_id, var);
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str::<SongMetadata>(&raw).ok()
}

fn resolve_week_songs_and_diffs(
    assets: &AssetLocator,
    songs: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut song_names = Vec::with_capacity(songs.len());
    let mut per_song_diffs: Vec<Vec<String>> = Vec::with_capacity(songs.len());

    for song_id in songs {
        let base = read_song_meta(assets, song_id, None);
        let erect = read_song_meta(assets, song_id, Some("erect"));
        let name = base
            .as_ref()
            .or(erect.as_ref())
            .map(|m| m.song_name.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or(song_id.as_str())
            .to_owned();
        song_names.push(name);

        let mut supported = Vec::new();
        for d in STORY_DIFFS {
            let has_in_base = base
                .as_ref()
                .is_some_and(|m| m.play_data.difficulties.iter().any(|x| x.eq_ignore_ascii_case(d)));
            let has_in_erect = erect
                .as_ref()
                .is_some_and(|m| m.play_data.difficulties.iter().any(|x| x.eq_ignore_ascii_case(d)));
            if has_in_base || has_in_erect {
                supported.push(d.to_owned());
            }
        }
        per_song_diffs.push(supported);
    }

    let mut common_diffs = Vec::new();
    for d in STORY_DIFFS {
        if !per_song_diffs.is_empty()
            && per_song_diffs
                .iter()
                .all(|list| list.iter().any(|x| x == d))
        {
            common_diffs.push(d.to_owned());
        }
    }
    if common_diffs.is_empty() {
        common_diffs.push("normal".to_owned());
    }
    (song_names, common_diffs)
}

fn write_formatted_commas(buf: &mut StackTextBuffer<48>, val: i64) {
    let neg = val < 0;
    let abs_u = val.unsigned_abs();
    let mut digits = [0u8; 24];
    let mut n = 0usize;
    if abs_u == 0 {
        digits[0] = b'0';
        n = 1;
    } else {
        let mut v = abs_u;
        while v > 0 {
            digits[n] = b'0' + (v % 10) as u8;
            n += 1;
            v /= 10;
        }
    }
    if neg {
        let _ = buf.write_char('-');
    }
    for i in (0..n).rev() {
        let _ = buf.write_char(digits[i] as char);
        if i > 0 && i % 3 == 0 {
            let _ = buf.write_char(',');
        }
    }
}

fn diff_texture_index(diff: &str) -> usize {
    match diff.to_ascii_lowercase().as_str() {
        "easy" => 0,
        "hard" => 2,
        _ => 1,
    }
}

impl StoryMenuScreenState {
    #[must_use]
    pub fn load(
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        save: &SaveData,
        remembered_level_id: Option<&str>,
        remembered_difficulty: &str,
    ) -> Self {
        let levels_dir = assets.root().join("data").join("levels");
        let mut level_files: Vec<(String, std::path::PathBuf)> = fs::read_dir(&levels_dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) != Some("json") {
                    return None;
                }
                let id = p.file_stem()?.to_str()?.to_owned();
                Some((id, p))
            })
            .collect();
        level_files.sort_by_key(|(id, _)| level_sort_key(id));

        let mut weeks = Vec::with_capacity(level_files.len());
        for (id, path) in level_files {
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(data) = serde_json::from_str::<LevelData>(&raw) else {
                continue;
            };
            if !data.visible || data.songs.is_empty() {
                continue;
            }
            let title_tex = load_menu_image(assets, gpu, &data.title_asset);
            let (song_names, difficulties) = resolve_week_songs_and_diffs(assets, &data.songs);
            let is_simple_bg = data.background.trim().starts_with('#');
            let bg_color = if is_simple_bg {
                parse_stage_color(&data.background)
            } else {
                parse_stage_color("#F9CF51")
            };
            weeks.push(StoryWeekEntry {
                id: id.to_ascii_lowercase(),
                id_upper: id.to_ascii_uppercase(),
                name: data.name,
                songs: data.songs,
                song_names,
                difficulties,
                is_simple_bg,
                bg_color,
                bg_asset: data.background,
                title_tex,
                y: INITIAL_TITLE_Y,
                target_y: INITIAL_TITLE_Y,
                prop_specs: data.props,
                props: Vec::new(),
            });
        }

        let selected_week = remembered_level_id
            .and_then(|rem| weeks.iter().position(|w| w.id.eq_ignore_ascii_case(rem)))
            .unwrap_or(0);

        let loaded_props_week = if let Some(sel) = weeks.get_mut(selected_week) {
            sel.props = load_week_props(&sel.prop_specs, assets, gpu);
            Some(selected_week)
        } else {
            None
        };

        let arrows_loaded = load_menu_sparrow(assets, gpu, "storymenu/ui/arrows");
        let (arrows_tex, left_idle, left_press, right_idle, right_press) = match arrows_loaded {
            Some((t, sz, atlas)) => (
                Some((t, sz)),
                atlas.frames_by_prefix("leftIdle0").next().cloned(),
                atlas.frames_by_prefix("leftConfirm0").next().cloned(),
                atlas.frames_by_prefix("rightIdle0").next().cloned(),
                atlas.frames_by_prefix("rightConfirm0").next().cloned(),
            ),
            None => (None, None, None, None, None),
        };

        let diff_textures = [
            load_menu_image(assets, gpu, "storymenu/difficulties/easy"),
            load_menu_image(assets, gpu, "storymenu/difficulties/normal"),
            load_menu_image(assets, gpu, "storymenu/difficulties/hard"),
        ];

        let (initial_color, initial_bg_tex) = if let Some(w) = weeks.get(selected_week) {
            if w.is_simple_bg {
                (w.bg_color, None)
            } else {
                (w.bg_color, load_menu_image(assets, gpu, &w.bg_asset))
            }
        } else {
            (parse_stage_color("#F9CF51"), None)
        };

        let mut state = Self {
            white_tex: gpu.white_texture(),
            weeks,
            selected_week,
            loaded_props_week,
            available_diffs: Vec::new(),
            current_difficulty: remembered_difficulty.to_ascii_lowercase(),
            active_bg_color: initial_color,
            bg_color_tween: None,
            active_bg_tex: initial_bg_tex,
            old_bg_fade: None,
            high_score: 0,
            high_score_lerp: 12_345_678.0,
            saved_level_scores: Vec::new(),
            arrows_tex,
            left_idle,
            left_press,
            right_idle,
            right_press,
            diff_textures,
            diff_tween_s: 0.07,
            anim_time_s: 0.0,
            beat_timer_s: 0.0,
            beat_count: 0,
            confirm_elapsed_s: 0.0,
            confirming: false,
            confirm_timer_s: 0.0,
        };
        state.sync_from_save(save);
        state.reposition_titles();
        state.sync_difficulty_for_week(false);
        state.refresh_high_score();
        state
    }

    pub fn sync_from_save(&mut self, save: &SaveData) {
        self.saved_level_scores.clear();
        for w in &self.weeks {
            for d in &w.difficulties {
                let key = SaveData::level_key(&w.id, d);
                if let Some(entry) = save.scores.levels.get(&key) {
                    self.saved_level_scores
                        .push((w.id.clone(), d.clone(), entry.score));
                }
            }
        }
        self.refresh_high_score();
    }

    /// Ensures only the currently selected week has its heavy prop textures uploaded on GPU.
    pub fn sync_active_week(&mut self, assets: &AssetLocator, gpu: &mut GpuContext) {
        if self.loaded_props_week == Some(self.selected_week) {
            return;
        }
        let prev_idx = self.loaded_props_week;
        if let Some(p_idx) = prev_idx {
            if let Some(prev) = self.weeks.get_mut(p_idx) {
                for prop in &mut prev.props {
                    if let Some((tex, _)) = prop.tex.take() {
                        gpu.unload_texture(tex);
                    }
                }
                prev.props.clear();
            }
        }
        if let Some(curr) = self.weeks.get_mut(self.selected_week) {
            curr.props = load_week_props(&curr.prop_specs, assets, gpu);
            self.loaded_props_week = Some(self.selected_week);

            // Trigger background color or complex background crossfade (`updateBackground`).
            let prev_simple = prev_idx
                .and_then(|i| self.weeks.get(i))
                .is_none_or(|p| p.is_simple_bg);
            if curr.is_simple_bg && prev_simple {
                self.old_bg_fade = None;
                self.active_bg_tex = None;
                if self.active_bg_color != curr.bg_color {
                    self.bg_color_tween = Some(BgColorTween {
                        from: self.active_bg_color,
                        to: curr.bg_color,
                        elapsed_s: 0.0,
                        duration_s: 0.9,
                    });
                }
            } else {
                self.bg_color_tween = None;
                self.active_bg_color = curr.bg_color;
                if let Some(old) = self.active_bg_tex.take() {
                    self.old_bg_fade = Some(BgImageFade {
                        old_tex: old,
                        elapsed_s: 0.0,
                        duration_s: 0.6,
                    });
                }
                if !curr.is_simple_bg {
                    self.active_bg_tex = load_menu_image(assets, gpu, &curr.bg_asset);
                }
            }
        }
    }

    pub fn reset_for_enter(&mut self, save: &SaveData) {
        self.confirming = false;
        self.confirm_timer_s = 0.0;
        self.confirm_elapsed_s = 0.0;
        self.high_score_lerp = 12_345_678.0;
        for w in &mut self.weeks {
            w.y = INITIAL_TITLE_Y;
        }
        self.reposition_titles();
        self.sync_from_save(save);
    }

    /// Returns the active week level ID (e.g. `"week1"`), if any.
    #[must_use]
    pub fn selected_week_id(&self) -> Option<&str> {
        self.weeks.get(self.selected_week).map(|w| w.id.as_str())
    }

    /// Returns the active selected difficulty ID (e.g. `"normal"`).
    #[must_use]
    pub fn selected_difficulty(&self) -> &str {
        self.current_difficulty.as_str()
    }

    fn title_height(&self, idx: usize) -> f32 {
        self.weeks
            .get(idx)
            .and_then(|w| w.title_tex)
            .map_or(88.0, |(_, sz)| sz[1])
    }

    fn reposition_titles(&mut self) {
        if self.weeks.is_empty() {
            return;
        }
        let cur = self.selected_week.min(self.weeks.len() - 1);
        self.weeks[cur].target_y = SELECTED_TITLE_Y;

        for step in 0..cur {
            let idx = cur - 1 - step;
            let next_y = self.weeks[idx + 1].target_y;
            let h = self.title_height(idx);
            self.weeks[idx].target_y = next_y - (h + TITLE_GAP).max(MIN_ABOVE_STEP);
        }

        for idx in (cur + 1)..self.weeks.len() {
            let prev_y = self.weeks[idx - 1].target_y;
            let prev_h = self.title_height(idx - 1);
            self.weeks[idx].target_y = prev_y + (prev_h + TITLE_GAP);
        }
    }

    fn sync_difficulty_for_week(&mut self, trigger_tween: bool) {
        let Some(w) = self.weeks.get(self.selected_week) else {
            return;
        };
        self.available_diffs = w.difficulties.clone();
        if self.available_diffs.is_empty() {
            self.available_diffs.push("normal".to_owned());
        }
        if !self
            .available_diffs
            .iter()
            .any(|d| d.eq_ignore_ascii_case(&self.current_difficulty))
        {
            self.current_difficulty = self.available_diffs[0].clone();
        }
        if trigger_tween {
            self.diff_tween_s = 0.0;
        }
    }

    fn refresh_high_score(&mut self) {
        let Some(week_id) = self.weeks.get(self.selected_week).map(|w| &w.id) else {
            self.high_score = 0;
            return;
        };
        self.high_score = self
            .saved_level_scores
            .iter()
            .find(|(id, diff, _)| {
                id.eq_ignore_ascii_case(week_id)
                    && diff.eq_ignore_ascii_case(&self.current_difficulty)
            })
            .map_or(0, |(_, _, s)| *s);
    }

    pub fn update(&mut self, elapsed_s: f32, input: MenuInputState) -> MenuCommand {
        let dt = elapsed_s.clamp(0.0, 0.1);
        self.anim_time_s += dt;
        self.beat_timer_s += dt;
        let beat_dur = 60.0 / INTRO_BPM;
        if self.beat_timer_s >= beat_dur {
            self.beat_timer_s -= beat_dur;
            self.beat_count = self.beat_count.wrapping_add(1);
        }

        if self.diff_tween_s < 0.07 {
            self.diff_tween_s = (self.diff_tween_s + dt).min(0.07);
        }

        // Smooth background color tween (`FlxEase.quartOut`, 0.9s).
        if let Some(tw) = &mut self.bg_color_tween {
            tw.elapsed_s += dt;
            let raw_t = (tw.elapsed_s / tw.duration_s).clamp(0.0, 1.0);
            let eased = FlxEase::QuartOut.eval(raw_t);
            for i in 0..4 {
                self.active_bg_color[i] = tw.from[i] + (tw.to[i] - tw.from[i]) * eased;
            }
            if raw_t >= 1.0 {
                self.active_bg_color = tw.to;
                self.bg_color_tween = None;
            }
        }

        if let Some(fade) = &mut self.old_bg_fade {
            fade.elapsed_s += dt;
            if fade.elapsed_s >= fade.duration_s {
                self.old_bg_fade = None;
            }
        }

        // High score ticker lerp (`smoothLerpPrecision(..., 0.307)` + snap within 1).
        let target_hs = self.high_score as f64;
        self.high_score_lerp =
            smooth_lerp_precision(self.high_score_lerp, target_hs, dt, 0.307, 0.01);
        if (self.high_score_lerp - target_hs).abs() <= 1.0 {
            self.high_score_lerp = target_hs;
        }

        // Per-item vertical slide (`LevelTitle.hx`: smoothLerpPrecision(..., 0.451)).
        for w in &mut self.weeks {
            w.y = smooth_lerp_precision(w.y as f64, w.target_y as f64, dt, 0.451, 0.01) as f32;
        }

        if self.confirming {
            self.confirm_timer_s += dt;
            self.confirm_elapsed_s += dt;
            if self.confirm_timer_s >= CONFIRM_HOLD_S + CONFIRM_FADE_S {
                self.confirming = false;
                if let Some(week) = self.weeks.get(self.selected_week) {
                    return MenuCommand::StartStoryWeek {
                        level_id: week.id.clone(),
                        level_title: week.name.clone(),
                        songs: week.songs.clone(),
                        difficulty: self.current_difficulty.clone(),
                    };
                }
            }
            return MenuCommand::None;
        }

        if input.back_pressed {
            return MenuCommand::EnterMainMenu;
        }

        if input.up_pressed && !self.weeks.is_empty() {
            let prev = self.selected_week;
            self.selected_week = (self.selected_week + self.weeks.len() - 1) % self.weeks.len();
            self.reposition_titles();
            self.sync_difficulty_for_week(false);
            self.refresh_high_score();
            if self.selected_week != prev {
                return MenuCommand::PlaySfxVol("scrollMenu", 0.4);
            }
        }
        if input.down_pressed && !self.weeks.is_empty() {
            let prev = self.selected_week;
            self.selected_week = (self.selected_week + 1) % self.weeks.len();
            self.reposition_titles();
            self.sync_difficulty_for_week(false);
            self.refresh_high_score();
            if self.selected_week != prev {
                return MenuCommand::PlaySfxVol("scrollMenu", 0.4);
            }
        }
        if input.left_pressed && !self.available_diffs.is_empty() {
            let idx = self
                .available_diffs
                .iter()
                .position(|d| d.eq_ignore_ascii_case(&self.current_difficulty))
                .unwrap_or(0);
            let next_idx = (idx + self.available_diffs.len() - 1) % self.available_diffs.len();
            if next_idx != idx {
                self.current_difficulty = self.available_diffs[next_idx].clone();
                self.diff_tween_s = 0.0;
                self.refresh_high_score();
                return MenuCommand::PlaySfxVol("scrollMenu", 0.4);
            }
        }
        if input.right_pressed && !self.available_diffs.is_empty() {
            let idx = self
                .available_diffs
                .iter()
                .position(|d| d.eq_ignore_ascii_case(&self.current_difficulty))
                .unwrap_or(0);
            let next_idx = (idx + 1) % self.available_diffs.len();
            if next_idx != idx {
                self.current_difficulty = self.available_diffs[next_idx].clone();
                self.diff_tween_s = 0.0;
                self.refresh_high_score();
                return MenuCommand::PlaySfxVol("scrollMenu", 0.4);
            }
        }
        if input.accept_pressed && !self.weeks.is_empty() {
            self.confirming = true;
            self.confirm_timer_s = 0.0;
            self.confirm_elapsed_s = 0.0;
            return MenuCommand::PlaySfxVol("confirmMenu", 1.0);
        }

        MenuCommand::None
    }

    pub fn draw(
        &self,
        batcher: &mut SpriteBatcher,
        font: &BitmapFont,
        font_tex: TextureHandle,
        input: MenuInputState,
    ) {
        // 1. Fullscreen black base background.
        batcher.push(
            self.white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                .with_color([0.0, 0.0, 0.0, 1.0]),
        );

        let Some(active_week) = self.weeks.get(self.selected_week) else {
            font.draw_str(
                batcher,
                font_tex,
                "NO VISIBLE STORY LEVELS FOUND",
                VIRTUAL_WIDTH * 0.5,
                VIRTUAL_HEIGHT * 0.5,
                &TextStyle {
                    scale: 1.5,
                    color: [1.0, 1.0, 1.0, 1.0],
                    align: TextAlign::Center,
                    ..Default::default()
                },
            );
            return;
        };

        // 2. Week titles drawn BEFORE top stage banner (`zIndex = 15 < 100`), so sliding up tucks naturally behind `y = 456`.
        for (idx, week) in self.weeks.iter().enumerate() {
            if week.y > VIRTUAL_HEIGHT + 20.0 {
                continue;
            }
            let h = self.title_height(idx);
            if week.y + h < BANNER_Y + BANNER_HEIGHT - 160.0 {
                continue;
            }
            let is_sel = idx == self.selected_week;
            let blink =
                is_sel && self.confirming && (((self.confirm_timer_s * 20.0) as i32) & 1 == 1);
            let alpha = if is_sel { 1.0 } else { 0.6 };
            let color = if blink {
                [0.2, 1.0, 1.0, 1.0]
            } else {
                [1.0, 1.0, 1.0, alpha]
            };
            if let Some((tex, sz)) = week.title_tex {
                batcher.push(
                    tex,
                    BlendMode::Normal,
                    FilterMode::Linear,
                    SpriteInstance::new([(VIRTUAL_WIDTH - sz[0]) * 0.5, week.y], sz)
                        .with_color(color),
                );
            } else {
                font.draw_str(
                    batcher,
                    font_tex,
                    &week.id_upper,
                    VIRTUAL_WIDTH * 0.5,
                    week.y + 20.0,
                    &TextStyle {
                        scale: 1.6,
                        color,
                        align: TextAlign::Center,
                        ..Default::default()
                    },
                );
            }
        }

        // 3. Opaque black cover over `[0, 0..456]` behind stage banner (`StoryMenuState.hx:161`).
        batcher.push(
            self.white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, BANNER_Y + BANNER_HEIGHT])
                .with_color([0.0, 0.0, 0.0, 1.0]),
        );

        // 4. Stage banner background (`y = 56, h = 400`).
        if let Some((tex, sz)) = self.active_bg_tex {
            let new_alpha = self
                .old_bg_fade
                .as_ref()
                .map_or(1.0, |f| (f.elapsed_s / f.duration_s).clamp(0.0, 1.0));
            if let Some(fade) = &self.old_bg_fade {
                let old_a = (1.0 - new_alpha).clamp(0.0, 1.0);
                batcher.push(
                    fade.old_tex.0,
                    BlendMode::Normal,
                    FilterMode::Linear,
                    SpriteInstance::new([0.0, BANNER_Y], fade.old_tex.1)
                        .with_color([1.0, 1.0, 1.0, old_a]),
                );
            }
            batcher.push(
                tex,
                BlendMode::Normal,
                FilterMode::Linear,
                SpriteInstance::new([0.0, BANNER_Y], sz).with_color([1.0, 1.0, 1.0, new_alpha]),
            );
        } else {
            batcher.push(
                self.white_tex,
                BlendMode::Normal,
                FilterMode::Nearest,
                SpriteInstance::new([0.0, BANNER_Y], [VIRTUAL_WIDTH, BANNER_HEIGHT])
                    .with_color(self.active_bg_color),
            );
            if let Some(fade) = &self.old_bg_fade {
                let old_a = (1.0 - fade.elapsed_s / fade.duration_s).clamp(0.0, 1.0);
                batcher.push(
                    fade.old_tex.0,
                    BlendMode::Normal,
                    FilterMode::Linear,
                    SpriteInstance::new([0.0, BANNER_Y], fade.old_tex.1)
                        .with_color([1.0, 1.0, 1.0, old_a]),
                );
            }
        }

        // 5. Week Props (`LevelProp.hx:79-80`: `x = offsets[0] + width*0.25*idx, y = offsets[1]`).
        for (idx, prop) in active_week.props.iter().enumerate() {
            let Some((tex, sz)) = prop.tex else {
                continue;
            };
            let (frames, anim_offset, loop_mode) =
                if self.confirming && !prop.confirm_frames.is_empty() {
                    (&prop.confirm_frames, prop.confirm_offset, false)
                } else if prop.has_dance_lr {
                    let dance_period = prop.dance_every.max(1.0).round() as u32;
                    let step = (self.beat_count / dance_period.max(1)) & 1;
                    if step == 0 {
                        (&prop.dance_left_frames, prop.dance_left_offset, false)
                    } else {
                        (&prop.dance_right_frames, prop.dance_right_offset, false)
                    }
                } else {
                    (
                        &prop.idle_frames,
                        prop.idle_offset,
                        prop.dance_every <= 0.0,
                    )
                };
            if frames.is_empty() {
                continue;
            }
            let f_idx = if self.confirming && !prop.confirm_frames.is_empty() {
                ((self.confirm_elapsed_s * prop.fps).floor() as usize).min(frames.len() - 1)
            } else if loop_mode {
                ((self.anim_time_s * prop.fps).floor() as usize) % frames.len()
            } else {
                let beat_dur = 60.0 / INTRO_BPM;
                let mult = prop.dance_every.max(1.0);
                let cycle_dur = beat_dur * mult;
                let phase_s =
                    ((self.beat_count as f32 * beat_dur + self.beat_timer_s) % cycle_dur).max(0.0);
                ((phase_s * prop.fps).floor() as usize).min(frames.len() - 1)
            };
            if let Some(frame) = frames.get(f_idx) {
                let pos_x = (VIRTUAL_WIDTH * 0.25) * (idx as f32) + prop.offsets[0]
                    - anim_offset[0] * prop.scale;
                let pos_y = prop.offsets[1] - anim_offset[1] * prop.scale;
                draw_menu_sparrow_frame(
                    batcher,
                    tex,
                    sz,
                    frame,
                    [pos_x, pos_y],
                    [prop.scale, prop.scale],
                    prop.flip_x,
                    [1.0, 1.0, 1.0, prop.alpha],
                    [0.0, 0.0, 0.0],
                    if prop.is_pixel {
                        FilterMode::Nearest
                    } else {
                        FilterMode::Linear
                    },
                );
            }
        }

        // 6. Top bar HUD (`LEVEL SCORE: ...` left, level title right, `VCR OSD Mono` size 32).
        let mut score_buf = StackTextBuffer::<48>::new();
        let _ = score_buf.write_str("LEVEL SCORE: ");
        write_formatted_commas(&mut score_buf, self.high_score_lerp.round() as i64);
        font.draw_str(
            batcher,
            font_tex,
            score_buf.as_str(),
            10.0,
            10.0,
            &TextStyle {
                scale: 1.85,
                color: [1.0, 1.0, 1.0, 1.0],
                align: TextAlign::Left,
                ..Default::default()
            },
        );

        font.draw_str(
            batcher,
            font_tex,
            &active_week.name,
            VIRTUAL_WIDTH - 10.0,
            10.0,
            &TextStyle {
                scale: 1.85,
                color: [1.0, 1.0, 1.0, 0.7],
                align: TextAlign::Right,
                ..Default::default()
            },
        );

        // 7. Left-side TRACKS header + song display names (`#E55777`, `y = 500`).
        let tracks_center_x = VIRTUAL_WIDTH * 0.17;
        font.draw_str(
            batcher,
            font_tex,
            "TRACKS",
            tracks_center_x,
            500.0,
            &TextStyle {
                scale: 1.8,
                color: [0.898, 0.341, 0.467, 1.0],
                align: TextAlign::Center,
                ..Default::default()
            },
        );
        for (line_idx, song_name) in active_week.song_names.iter().enumerate() {
            font.draw_str(
                batcher,
                font_tex,
                song_name,
                tracks_center_x,
                564.0 + (line_idx as f32) * 32.0,
                &TextStyle {
                    scale: 1.75,
                    color: [0.898, 0.341, 0.467, 1.0],
                    align: TextAlign::Center,
                    ..Default::default()
                },
            );
        }

        // 8. Right-side difficulty arrows & sprite (`StoryMenuState.hx:192-210, 259-291`).
        let left_arrow_x = VIRTUAL_WIDTH - 410.0;
        let right_arrow_x = VIRTUAL_WIDTH - 47.0;
        let arrow_y = 480.0;
        let show_arrows = self.available_diffs.len() > 1;
        if show_arrows {
            if let Some((tex, sz)) = self.arrows_tex {
                let l_frame = if input.left_held {
                    self.left_press.as_ref().or(self.left_idle.as_ref())
                } else {
                    self.left_idle.as_ref()
                };
                if let Some(lf) = l_frame {
                    draw_menu_sparrow_frame(
                        batcher,
                        tex,
                        sz,
                        lf,
                        [left_arrow_x, arrow_y],
                        [1.0, 1.0],
                        false,
                        [1.0, 1.0, 1.0, 1.0],
                        [0.0, 0.0, 0.0],
                        FilterMode::Linear,
                    );
                }
                let r_frame = if input.right_held {
                    self.right_press.as_ref().or(self.right_idle.as_ref())
                } else {
                    self.right_idle.as_ref()
                };
                if let Some(rf) = r_frame {
                    draw_menu_sparrow_frame(
                        batcher,
                        tex,
                        sz,
                        rf,
                        [right_arrow_x, arrow_y],
                        [1.0, 1.0],
                        false,
                        [1.0, 1.0, 1.0, 1.0],
                        [0.0, 0.0, 0.0],
                        FilterMode::Linear,
                    );
                }
            }
        }

        let left_w = self.left_idle.as_ref().map_or(48.0, |f| f.source_size()[0]);
        let normal_size = self.diff_textures[1].map_or([307.0, 66.0], |(_, s)| s);
        let diff_idx = diff_texture_index(&self.current_difficulty);
        let tween_t = (self.diff_tween_s / 0.07).clamp(0.0, 1.0);
        if let Some((dtex, dsz)) = self.diff_textures[diff_idx] {
            let dx = left_arrow_x + left_w + 10.0 + (normal_size[0] - dsz[0]) * 0.5;
            let start_y = arrow_y - 15.0;
            let target_y = arrow_y + 10.0 - (dsz[1] - normal_size[1]) * 0.5;
            let dy = start_y + (target_y - start_y) * tween_t;
            batcher.push(
                dtex,
                BlendMode::Normal,
                FilterMode::Linear,
                SpriteInstance::new([dx, dy], dsz).with_color([1.0, 1.0, 1.0, tween_t]),
            );
        } else {
            font.draw_str(
                batcher,
                font_tex,
                &self.current_difficulty.to_ascii_uppercase(),
                left_arrow_x + left_w + 10.0 + normal_size[0] * 0.5,
                arrow_y + 24.0,
                &TextStyle {
                    scale: 1.8,
                    color: [1.0, 1.0, 1.0, tween_t],
                    align: TextAlign::Center,
                    ..Default::default()
                },
            );
        }

        // 9. Camera fade to black during the final 0.2s of week confirm (`StoryMenuState.hx:619`).
        if self.confirming && self.confirm_timer_s > CONFIRM_HOLD_S {
            let fade_a = ((self.confirm_timer_s - CONFIRM_HOLD_S) / CONFIRM_FADE_S).clamp(0.0, 1.0);
            batcher.push(
                self.white_tex,
                BlendMode::Normal,
                FilterMode::Nearest,
                SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                    .with_color([0.0, 0.0, 0.0, fade_a]),
            );
        }
    }
}
