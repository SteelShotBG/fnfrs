//! V-Slice Story Mode Menu (`StoryMenuState.hx`).

use std::fs;

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::schema::level::LevelData;
use fnfrs_render::{
    AtlasFrame, BitmapFont, BlendMode, FilterMode, GpuContext, SpriteBatcher, SpriteInstance,
    TextAlign, TextStyle, TextureHandle,
};

use crate::stage::parse_stage_color;

use super::{
    draw_menu_sparrow_frame, level_sort_key, load_menu_image, load_menu_sparrow, MenuCommand,
    MenuInputState,
};

const STORY_DIFFS: [&str; 3] = ["easy", "normal", "hard"];
const STORY_DIFFS_UPPER: [&str; 3] = ["EASY", "NORMAL", "HARD"];
const INTRO_BPM: f32 = 102.0;

#[derive(Debug)]
struct StoryPropSprite {
    tex: Option<(TextureHandle, [f32; 2])>,
    idle_frames: Vec<AtlasFrame>,
    confirm_frames: Vec<AtlasFrame>,
    offsets: [f32; 2],
    scale: f32,
    alpha: f32,
    is_pixel: bool,
    flip_x: bool,
    fps: f32,
}

#[derive(Debug)]
struct StoryWeekEntry {
    id_upper: String,
    hdr_label: String,
    name: String,
    songs: Vec<String>,
    tracks_display: String,
    bg_color: [f32; 4],
    title_tex: Option<(TextureHandle, [f32; 2])>,
    props: Vec<StoryPropSprite>,
}

#[derive(Debug)]
pub struct StoryMenuScreenState {
    white_tex: TextureHandle,
    weeks: Vec<StoryWeekEntry>,
    selected_week: usize,
    diff_idx: usize,
    scroll_y: f32,

    arrows_tex: Option<(TextureHandle, [f32; 2])>,
    left_idle: Option<AtlasFrame>,
    left_press: Option<AtlasFrame>,
    right_idle: Option<AtlasFrame>,
    right_press: Option<AtlasFrame>,
    diff_textures: [Option<(TextureHandle, [f32; 2])>; 3],

    anim_time_s: f32,
    beat_timer_s: f32,
    confirming: bool,
    confirm_timer_s: f32,
}

impl StoryMenuScreenState {
    #[must_use]
    pub fn load(assets: &AssetLocator, gpu: &mut GpuContext) -> Self {
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
            let mut props = Vec::with_capacity(data.props.len());
            for prop in &data.props {
                let loaded = load_menu_sparrow(assets, gpu, &prop.asset_path);
                let idle_anim = prop
                    .animations
                    .iter()
                    .find(|a| a.name.eq_ignore_ascii_case("idle"));
                let confirm_anim = prop
                    .animations
                    .iter()
                    .find(|a| a.name.eq_ignore_ascii_case("confirm"));
                let fps = idle_anim.map_or(24.0, |a| a.frame_rate.max(1.0));
                let (tex, idle_frames, confirm_frames) = match loaded {
                    Some((t, sz, atlas)) => {
                        let idle_pfx = idle_anim
                            .and_then(|a| a.prefix.as_deref())
                            .unwrap_or("idle");
                        let conf_pfx = confirm_anim
                            .and_then(|a| a.prefix.as_deref())
                            .unwrap_or("confirm");
                        (
                            Some((t, sz)),
                            atlas.frames_by_prefix(idle_pfx).cloned().collect(),
                            atlas.frames_by_prefix(conf_pfx).cloned().collect(),
                        )
                    }
                    None => (None, Vec::new(), Vec::new()),
                };
                props.push(StoryPropSprite {
                    tex,
                    idle_frames,
                    confirm_frames,
                    offsets: prop.offsets,
                    scale: prop.scale,
                    alpha: prop.alpha,
                    is_pixel: prop.is_pixel,
                    flip_x: prop.flip_x,
                    fps,
                });
            }
            let tracks_display = data.songs.join("\n").to_ascii_uppercase();
            let id_upper = id.to_ascii_uppercase();
            let hdr_label = format!("WEEK: {id_upper}");
            weeks.push(StoryWeekEntry {
                id_upper,
                hdr_label,
                name: data.name,
                songs: data.songs,
                tracks_display,
                bg_color: parse_stage_color(&data.background),
                title_tex,
                props,
            });
        }

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

        Self {
            white_tex: gpu.white_texture(),
            weeks,
            selected_week: 0,
            diff_idx: 1,
            scroll_y: 0.0,
            arrows_tex,
            left_idle,
            left_press,
            right_idle,
            right_press,
            diff_textures,
            anim_time_s: 0.0,
            beat_timer_s: 0.0,
            confirming: false,
            confirm_timer_s: 0.0,
        }
    }

    pub fn reset_for_enter(&mut self) {
        self.confirming = false;
        self.confirm_timer_s = 0.0;
        self.scroll_y = self.selected_week as f32 * 120.0;
    }

    pub fn update(&mut self, elapsed_s: f32, input: MenuInputState) -> MenuCommand {
        let dt = elapsed_s.clamp(0.0, 0.1);
        self.anim_time_s += dt;
        self.beat_timer_s += dt;
        let beat_dur = 60.0 / INTRO_BPM;
        if self.beat_timer_s >= beat_dur {
            self.beat_timer_s -= beat_dur;
        }

        let target_scroll = self.selected_week as f32 * 120.0;
        let lerp_t = 1.0 - (1.0 - 0.17_f32).powf(dt * 60.0);
        self.scroll_y += (target_scroll - self.scroll_y) * lerp_t;

        if self.confirming {
            self.confirm_timer_s += dt;
            if self.confirm_timer_s >= 0.85 {
                self.confirming = false;
                if let Some(week) = self.weeks.get(self.selected_week) {
                    return MenuCommand::StartStoryWeek {
                        songs: week.songs.clone(),
                        difficulty: STORY_DIFFS[self.diff_idx].to_owned(),
                    };
                }
            }
            return MenuCommand::None;
        }

        if input.back_pressed {
            return MenuCommand::EnterMainMenu;
        }

        if input.up_pressed && !self.weeks.is_empty() {
            self.selected_week = (self.selected_week + self.weeks.len() - 1) % self.weeks.len();
            return MenuCommand::PlaySfx("scrollMenu");
        }
        if input.down_pressed && !self.weeks.is_empty() {
            self.selected_week = (self.selected_week + 1) % self.weeks.len();
            return MenuCommand::PlaySfx("scrollMenu");
        }
        if input.left_pressed {
            self.diff_idx = (self.diff_idx + STORY_DIFFS.len() - 1) % STORY_DIFFS.len();
            return MenuCommand::PlaySfx("scrollMenu");
        }
        if input.right_pressed {
            self.diff_idx = (self.diff_idx + 1) % STORY_DIFFS.len();
            return MenuCommand::PlaySfx("scrollMenu");
        }
        if input.accept_pressed && !self.weeks.is_empty() {
            self.confirming = true;
            self.confirm_timer_s = 0.0;
            return MenuCommand::PlaySfx("confirmMenu");
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
        // Black base background.
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
                    scale: 1.1,
                    color: [1.0, 1.0, 1.0, 1.0],
                    align: TextAlign::Center,
                    ..Default::default()
                },
            );
            return;
        };

        // Colored stage banner behind props (`y = 56, h = 400`).
        batcher.push(
            self.white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([0.0, 56.0], [VIRTUAL_WIDTH, 400.0]).with_color(active_week.bg_color),
        );

        // Week props inside banner (`storymenu/props/*`).
        let prop_count = active_week.props.len().max(1);
        for (idx, prop) in active_week.props.iter().enumerate() {
            let Some((tex, sz)) = prop.tex else {
                continue;
            };
            let frames = if self.confirming && !prop.confirm_frames.is_empty() {
                &prop.confirm_frames
            } else {
                &prop.idle_frames
            };
            if frames.is_empty() {
                continue;
            }
            let f_idx = ((self.anim_time_s * prop.fps).floor() as usize) % frames.len();
            if let Some(frame) = frames.get(f_idx) {
                let src = frame.source_size();
                let spacing = 360.0;
                let group_origin_x = VIRTUAL_WIDTH * 0.5 - ((prop_count - 1) as f32) * spacing * 0.5;
                let center_x = group_origin_x + (idx as f32) * spacing + prop.offsets[0] * 0.35;
                let pos_x = center_x - src[0] * prop.scale * 0.5;
                let pos_y = 56.0 + (400.0 - src[1] * prop.scale) * 0.5 + prop.offsets[1] * 0.25;
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

        // Top black band over prop top overhang (`y = 0..56`).
        batcher.push(
            self.white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, 56.0])
                .with_color([0.0, 0.0, 0.0, 1.0]),
        );

        // Bottom black band containing level titles, tracks, and difficulty selector (`y = 456..720`).
        batcher.push(
            self.white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([0.0, 456.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT - 456.0])
                .with_color([0.0, 0.0, 0.0, 1.0]),
        );

        // Scrolling level title banners centered horizontally (`y = 480 + index*120 - scroll_y`).
        for (idx, week) in self.weeks.iter().enumerate() {
            let y = 480.0 + (idx as f32) * 120.0 - self.scroll_y;
            if y < 440.0 || y > VIRTUAL_HEIGHT + 20.0 {
                continue;
            }
            let is_sel = idx == self.selected_week;
            let blink = is_sel && self.confirming && (((self.confirm_timer_s * 16.0) as i32) & 1 == 1);
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
                    SpriteInstance::new([(VIRTUAL_WIDTH - sz[0]) * 0.5, y], sz).with_color(color),
                );
            } else {
                font.draw_str(
                    batcher,
                    font_tex,
                    &week.id_upper,
                    VIRTUAL_WIDTH * 0.5,
                    y + 20.0,
                    &TextStyle {
                        scale: 1.2,
                        color,
                        align: TextAlign::Center,
                        ..Default::default()
                    },
                );
            }
        }

        // Top HUD text: week name (right) & label (left).
        font.draw_str(
            batcher,
            font_tex,
            &active_week.hdr_label,
            16.0,
            12.0,
            &TextStyle {
                scale: 0.9,
                color: [1.0, 1.0, 1.0, 1.0],
                align: TextAlign::Left,
                ..Default::default()
            },
        );
        font.draw_str(
            batcher,
            font_tex,
            &active_week.name,
            VIRTUAL_WIDTH - 16.0,
            12.0,
            &TextStyle {
                scale: 0.9,
                color: [1.0, 1.0, 1.0, 0.72],
                align: TextAlign::Right,
                ..Default::default()
            },
        );

        // Left side TRACKS header + song list in `#E55777`.
        font.draw_str(
            batcher,
            font_tex,
            "TRACKS",
            200.0,
            488.0,
            &TextStyle {
                scale: 0.95,
                color: [0.898, 0.341, 0.467, 1.0],
                align: TextAlign::Center,
                ..Default::default()
            },
        );
        for (line_idx, song) in active_week.tracks_display.lines().enumerate() {
            font.draw_str(
                batcher,
                font_tex,
                song,
                200.0,
                536.0 + (line_idx as f32) * 30.0,
                &TextStyle {
                    scale: 0.8,
                    color: [0.898, 0.341, 0.467, 1.0],
                    align: TextAlign::Center,
                    ..Default::default()
                },
            );
        }

        // Right side difficulty arrows & graphic (`storymenu/difficulties/*`).
        let left_arrow_x = VIRTUAL_WIDTH - 410.0;
        let right_arrow_x = VIRTUAL_WIDTH - 80.0;
        let arrow_y = 480.0;
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
        if let Some((dtex, dsz)) = self.diff_textures[self.diff_idx] {
            let dx = left_arrow_x + 56.0 + (270.0 - dsz[0]) * 0.5;
            let dy = arrow_y + (85.0 - dsz[1]) * 0.5;
            batcher.push(
                dtex,
                BlendMode::Normal,
                FilterMode::Linear,
                SpriteInstance::new([dx, dy], dsz),
            );
        } else {
            font.draw_str(
                batcher,
                font_tex,
                STORY_DIFFS_UPPER[self.diff_idx],
                left_arrow_x + 190.0,
                arrow_y + 26.0,
                &TextStyle {
                    scale: 1.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                    align: TextAlign::Center,
                    ..Default::default()
                },
            );
        }
    }
}
