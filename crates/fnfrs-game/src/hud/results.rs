use std::fmt::Write as _;
use std::fs;

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_core::scoring::{calculate_rank, tally_completion, ScoreTallies, ScoringRank};
use fnfrs_render::{
    AnimateAtlas, BitmapFont, BlendMode, EvaluatedPart, FilterMode, GpuContext, SpriteBatcher,
    SpriteInstance, StackTextBuffer, TextAlign, TextStyle, TextureHandle, SPRITE_FLAG_AFFINE,
    SPRITE_FLAG_ROTATED_90_CCW,
};
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

#[derive(Debug, Clone)]
struct ResultsVisuals {
    char_atlas: Option<AnimateAtlas>,
    char_textures: Vec<(TextureHandle, [f32; 2])>,
    char_bounds: ([f32; 2], [f32; 2]),
    char_parts: Vec<EvaluatedPart>,
    top_bar_tex: Option<(TextureHandle, [f32; 2])>,
    diff_tex: Option<(TextureHandle, [f32; 2])>,
    rank_tex: Option<(TextureHandle, [f32; 2])>,
}

impl ResultsVisuals {
    fn load(
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        difficulty: &str,
        rank: ResultsRank,
        is_pico: bool,
    ) -> Self {
        let top_bar_tex = crate::character::upload_png_path(
            gpu,
            Some("results/topBar"),
            &assets.image("resultScreen/topBar"),
        );

        let diff_lower = difficulty.to_ascii_lowercase();
        let diff_key = format!("resultScreen/diff_{diff_lower}");
        let diff_tex =
            crate::character::upload_png_path(gpu, Some(&diff_key), &assets.image(&diff_key));

        let rank_slug = match rank {
            ResultsRank::Loss => "loss",
            ResultsRank::Good => "good",
            ResultsRank::Great => "great",
            ResultsRank::Excellent => "excellent",
            ResultsRank::Perfect => "perfect",
            ResultsRank::PerfectGold => "perfectGold",
        };
        let rank_key = format!("resultScreen/rankText/rankText{rank_slug}");
        let rank_tex =
            crate::character::upload_png_path(gpu, Some(&rank_key), &assets.image(&rank_key));

        let char_dir_key = if is_pico {
            match rank {
                ResultsRank::Loss => "resultScreen/results-pico/resultsSHIT",
                ResultsRank::Good => "resultScreen/results-pico/resultsGOOD",
                ResultsRank::Great => "resultScreen/results-pico/resultsGREAT",
                ResultsRank::Excellent => "resultScreen/results-pico/resultsEXCELLENT",
                ResultsRank::Perfect | ResultsRank::PerfectGold => {
                    "resultScreen/results-pico/resultsPERFECT"
                }
            }
        } else {
            match rank {
                ResultsRank::Loss => "resultScreen/results-bf/resultsSHIT",
                ResultsRank::Good => "resultScreen/results-bf/resultsGOOD",
                ResultsRank::Great => "resultScreen/results-bf/resultsGREAT",
                ResultsRank::Excellent => "resultScreen/results-bf/resultsEXCELLENT",
                ResultsRank::Perfect | ResultsRank::PerfectGold => {
                    "resultScreen/results-bf/resultsPERFECT"
                }
            }
        };

        let mut char_atlas = None;
        let mut char_textures = Vec::new();
        let mut char_bounds = ([0.0, 0.0], [0.0, 0.0]);
        let mut char_parts = Vec::new();

        let dir = assets.animate_atlas_dir(char_dir_key);
        if let Ok(anim_json) = fs::read_to_string(dir.join("Animation.json")) {
            let mut sm_entries: Vec<(u32, std::path::PathBuf)> = Vec::new();
            if let Ok(rd) = fs::read_dir(&dir) {
                for entry in rd.flatten() {
                    let p = entry.path();
                    if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                        if name.starts_with("spritemap") && name.ends_with(".json") {
                            let num = name["spritemap".len()..name.len() - ".json".len()]
                                .parse::<u32>()
                                .unwrap_or(0);
                            sm_entries.push((num, p));
                        }
                    }
                }
            }
            sm_entries.sort_by_key(|(n, _)| *n);
            let mut sm_strings = Vec::with_capacity(sm_entries.len());
            for (_, p) in &sm_entries {
                if let Ok(s) = fs::read_to_string(p) {
                    sm_strings.push(s);
                }
            }
            if sm_strings.len() == sm_entries.len() && !sm_strings.is_empty() {
                let sm_refs: Vec<&str> = sm_strings.iter().map(String::as_str).collect();
                if let Ok(atlas) = AnimateAtlas::from_json_strs(&anim_json, &sm_refs) {
                    let mut textures = Vec::with_capacity(atlas.spritemaps().len());
                    let mut all_ok = true;
                    for sm in atlas.spritemaps() {
                        if let Some(tex) = crate::character::upload_png_path(
                            gpu,
                            Some(&sm.image),
                            &dir.join(&sm.image),
                        ) {
                            textures.push(tex);
                        } else {
                            all_ok = false;
                            break;
                        }
                    }
                    if all_ok {
                        char_bounds = atlas.full_timeline_bounds();
                        char_parts = Vec::with_capacity(64);
                        atlas.evaluate_frame(0, &mut char_parts);
                        char_textures = textures;
                        char_atlas = Some(atlas);
                    }
                }
            }
        }

        Self {
            char_atlas,
            char_textures,
            char_bounds,
            char_parts,
            top_bar_tex,
            diff_tex,
            rank_tex,
        }
    }
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
    visuals: Option<Box<ResultsVisuals>>,
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
