use crate::constants::{
    HEALTH_BAD_BONUS, HEALTH_GOOD_BONUS, HEALTH_SHIT_BONUS, HEALTH_SICK_BONUS,
    RANK_EXCELLENT_THRESHOLD, RANK_GOOD_THRESHOLD, RANK_GREAT_THRESHOLD, RANK_PERFECT_THRESHOLD,
};

/// Active scoring curve (`ScoringSystem` in `Scoring.hx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScoringSystem {
    Legacy,
    Week7,
    #[default]
    Pbot1,
}

/// Judgement rating for a note hit or miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Judgement {
    Sick,
    Good,
    Bad,
    Shit,
    Miss,
}

impl Judgement {
    /// Returns the V-Slice judgement string identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sick => "sick",
            Self::Good => "good",
            Self::Bad => "bad",
            Self::Shit => "shit",
            Self::Miss => "miss",
        }
    }

    /// Health change applied on hit/miss (`PlayState.goodNoteHit`).
    #[must_use]
    pub const fn health_bonus(self) -> f32 {
        match self {
            Self::Sick => HEALTH_SICK_BONUS,
            Self::Good => HEALTH_GOOD_BONUS,
            Self::Bad => HEALTH_BAD_BONUS,
            Self::Shit => HEALTH_SHIT_BONUS,
            Self::Miss => crate::constants::HEALTH_MISS_PENALTY,
        }
    }

    /// Whether this judgement breaks the player's combo (`Constants.JUDGEMENT_*_COMBO_BREAK`).
    #[must_use]
    pub const fn breaks_combo(self) -> bool {
        matches!(self, Self::Bad | Self::Shit | Self::Miss)
    }
}

/// End-of-song rank (`ScoringRank` in `Scoring.hx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScoringRank {
    Shit,
    Good,
    Great,
    Excellent,
    Perfect,
    PerfectGold,
}

/// Accumulated song judgement tallies (`SaveScoreTallyData`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScoreTallies {
    pub sick: u32,
    pub good: u32,
    pub bad: u32,
    pub shit: u32,
    pub missed: u32,
    pub combo: u32,
    pub max_combo: u32,
    pub total_notes_hit: u32,
    pub total_notes: u32,
}

// PBOT1 constants (`Scoring.hx` lines 99-160).
pub const PBOT1_MAX_SCORE: i32 = 500;
pub const PBOT1_SCORING_OFFSET: f64 = 54.99;
pub const PBOT1_SCORING_SLOPE: f64 = 0.080;
pub const PBOT1_MIN_SCORE: f64 = 9.0;
pub const PBOT1_MISS_SCORE: i32 = -100;
pub const PBOT1_PERFECT_THRESHOLD: f64 = 5.0;
pub const PBOT1_MISS_THRESHOLD: f64 = 160.0;
pub const PBOT1_SICK_THRESHOLD: f64 = 45.0;
pub const PBOT1_GOOD_THRESHOLD: f64 = 90.0;
pub const PBOT1_BAD_THRESHOLD: f64 = 135.0;
pub const PBOT1_SHIT_THRESHOLD: f64 = 160.0;

// Legacy & Week7 constants (`Scoring.hx` lines 209-312).
pub const LEGACY_HIT_WINDOW: f64 = (10.0 / 60.0) * 1000.0;
pub const LEGACY_SICK_THRESHOLD: f64 = 0.2;
pub const LEGACY_GOOD_THRESHOLD: f64 = 0.75;
pub const LEGACY_BAD_THRESHOLD: f64 = 0.9;
pub const LEGACY_MISS_SCORE: i32 = -10;

pub const WEEK7_SICK_THRESHOLD: f64 = 0.2;
pub const WEEK7_GOOD_THRESHOLD: f64 = 0.55;
pub const WEEK7_BAD_THRESHOLD: f64 = 0.8;
pub const WEEK7_MISS_SCORE: i32 = -10;

pub const STEP_SICK_SCORE: i32 = 350;
pub const STEP_GOOD_SCORE: i32 = 200;
pub const STEP_BAD_SCORE: i32 = 100;
pub const STEP_SHIT_SCORE: i32 = 50;

/// Computes the score awarded for a note hit at `ms_timing` offset.
#[must_use]
pub fn score_note(ms_timing: f64, system: ScoringSystem) -> i32 {
    let abs_timing = ms_timing.abs();
    match system {
        ScoringSystem::Pbot1 => {
            if abs_timing > PBOT1_MISS_THRESHOLD {
                PBOT1_MISS_SCORE
            } else if abs_timing < PBOT1_PERFECT_THRESHOLD {
                PBOT1_MAX_SCORE
            } else {
                let exp_arg = -PBOT1_SCORING_SLOPE * (abs_timing - PBOT1_SCORING_OFFSET);
                let factor = 1.0 - (1.0 / (1.0 + exp_arg.exp()));
                (f64::from(PBOT1_MAX_SCORE) * factor + PBOT1_MIN_SCORE) as i32
            }
        }
        ScoringSystem::Week7 => {
            if abs_timing < LEGACY_HIT_WINDOW * WEEK7_SICK_THRESHOLD {
                STEP_SICK_SCORE
            } else if abs_timing < LEGACY_HIT_WINDOW * WEEK7_GOOD_THRESHOLD {
                STEP_GOOD_SCORE
            } else if abs_timing < LEGACY_HIT_WINDOW * WEEK7_BAD_THRESHOLD {
                STEP_BAD_SCORE
            } else if abs_timing < LEGACY_HIT_WINDOW {
                STEP_SHIT_SCORE
            } else {
                0
            }
        }
        ScoringSystem::Legacy => {
            if abs_timing < LEGACY_HIT_WINDOW * LEGACY_SICK_THRESHOLD {
                STEP_SICK_SCORE
            } else if abs_timing < LEGACY_HIT_WINDOW * LEGACY_GOOD_THRESHOLD {
                STEP_GOOD_SCORE
            } else if abs_timing < LEGACY_HIT_WINDOW * LEGACY_BAD_THRESHOLD {
                STEP_BAD_SCORE
            } else if abs_timing < LEGACY_HIT_WINDOW {
                STEP_SHIT_SCORE
            } else {
                0
            }
        }
    }
}

/// Computes the [`Judgement`] for a note hit at `ms_timing` offset.
#[must_use]
pub fn judge_note(ms_timing: f64, system: ScoringSystem) -> Judgement {
    let abs_timing = ms_timing.abs();
    match system {
        ScoringSystem::Pbot1 => {
            if abs_timing <= PBOT1_SICK_THRESHOLD {
                Judgement::Sick
            } else if abs_timing <= PBOT1_GOOD_THRESHOLD {
                Judgement::Good
            } else if abs_timing <= PBOT1_BAD_THRESHOLD {
                Judgement::Bad
            } else if abs_timing <= PBOT1_SHIT_THRESHOLD {
                Judgement::Shit
            } else {
                Judgement::Miss
            }
        }
        ScoringSystem::Week7 => {
            if abs_timing <= LEGACY_HIT_WINDOW * WEEK7_SICK_THRESHOLD {
                Judgement::Sick
            } else if abs_timing <= LEGACY_HIT_WINDOW * WEEK7_GOOD_THRESHOLD {
                Judgement::Good
            } else if abs_timing <= LEGACY_HIT_WINDOW * WEEK7_BAD_THRESHOLD {
                Judgement::Bad
            } else if abs_timing <= LEGACY_HIT_WINDOW {
                Judgement::Shit
            } else {
                Judgement::Miss
            }
        }
        ScoringSystem::Legacy => {
            if abs_timing <= LEGACY_HIT_WINDOW * LEGACY_SICK_THRESHOLD {
                Judgement::Sick
            } else if abs_timing <= LEGACY_HIT_WINDOW * LEGACY_GOOD_THRESHOLD {
                Judgement::Good
            } else if abs_timing <= LEGACY_HIT_WINDOW * LEGACY_BAD_THRESHOLD {
                Judgement::Bad
            } else if abs_timing <= LEGACY_HIT_WINDOW {
                Judgement::Shit
            } else {
                Judgement::Miss
            }
        }
    }
}

/// Miss score penalty for the given [`ScoringSystem`].
#[must_use]
pub const fn get_miss_score(system: ScoringSystem) -> i32 {
    match system {
        ScoringSystem::Pbot1 => PBOT1_MISS_SCORE,
        ScoringSystem::Week7 => WEEK7_MISS_SCORE,
        ScoringSystem::Legacy => LEGACY_MISS_SCORE,
    }
}

/// Computes normalized completion ratio `[0.0, 1.0]` (`Scoring.tallyCompletion`).
#[must_use]
pub fn tally_completion(tallies: &ScoreTallies) -> f64 {
    if tallies.total_notes == 0 {
        return 0.0;
    }
    let numerator = i64::from(tallies.sick) + i64::from(tallies.good) - i64::from(tallies.missed);
    (numerator as f64 / f64::from(tallies.total_notes)).clamp(0.0, 1.0)
}

/// Calculates the final song rank (`Scoring.calculateRank`).
#[must_use]
pub fn calculate_rank(tallies: &ScoreTallies) -> Option<ScoringRank> {
    if tallies.total_notes == 0 {
        return None;
    }
    if tallies.sick == tallies.total_notes {
        return Some(ScoringRank::PerfectGold);
    }

    let completion = tally_completion(tallies);
    if (completion - RANK_PERFECT_THRESHOLD).abs() < f64::EPSILON {
        Some(ScoringRank::Perfect)
    } else if completion >= RANK_EXCELLENT_THRESHOLD {
        Some(ScoringRank::Excellent)
    } else if completion >= RANK_GREAT_THRESHOLD {
        Some(ScoringRank::Great)
    } else if completion >= RANK_GOOD_THRESHOLD {
        Some(ScoringRank::Good)
    } else {
        Some(ScoringRank::Shit)
    }
}

/// Zero-allocation stack-buffer score formatter for the HUD (`"Score: 1,234,560"`).
#[must_use]
pub fn format_score_into(buf: &mut [u8; 32], score: i64, botplay: bool) -> &str {
    if botplay {
        let msg = b"Bot Play Enabled";
        buf[..msg.len()].copy_from_slice(msg);
        return std::str::from_utf8(&buf[..msg.len()]).unwrap_or("Bot Play Enabled");
    }

    let prefix = b"Score: ";
    let mut pos = prefix.len();
    buf[..pos].copy_from_slice(prefix);

    let negative = score < 0;
    let mut val = score.unsigned_abs();
    if negative && pos < buf.len() {
        buf[pos] = b'-';
        pos += 1;
    }

    let mut digits_rev = [0_u8; 24];
    let mut d_len = 0_usize;
    if val == 0 {
        digits_rev[0] = b'0';
        d_len = 1;
    } else {
        let mut digit_count = 0_usize;
        while val > 0 && d_len < digits_rev.len() {
            if digit_count > 0 && digit_count % 3 == 0 {
                digits_rev[d_len] = b',';
                d_len += 1;
            }
            digits_rev[d_len] = b'0' + (val % 10) as u8;
            d_len += 1;
            digit_count += 1;
            val /= 10;
        }
    }

    for i in (0..d_len).rev() {
        if pos >= buf.len() {
            break;
        }
        buf[pos] = digits_rev[i];
        pos += 1;
    }

    std::str::from_utf8(&buf[..pos]).unwrap_or("Score: 0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pbot1_curve_and_judgements() {
        assert_eq!(score_note(0.0, ScoringSystem::Pbot1), 500);
        assert_eq!(score_note(4.9, ScoringSystem::Pbot1), 500);
        assert_eq!(score_note(-4.9, ScoringSystem::Pbot1), 500);
        assert_eq!(score_note(160.1, ScoringSystem::Pbot1), -100);

        // At offset 54.99ms, sigmoid factor == 0.5 => floor(500 * 0.5 + 9) == 259
        assert_eq!(score_note(54.99, ScoringSystem::Pbot1), 259);

        assert_eq!(judge_note(45.0, ScoringSystem::Pbot1), Judgement::Sick);
        assert_eq!(judge_note(90.0, ScoringSystem::Pbot1), Judgement::Good);
        assert_eq!(judge_note(135.0, ScoringSystem::Pbot1), Judgement::Bad);
        assert_eq!(judge_note(160.0, ScoringSystem::Pbot1), Judgement::Shit);
        assert_eq!(judge_note(160.1, ScoringSystem::Pbot1), Judgement::Miss);
    }

    #[test]
    fn rank_and_zero_alloc_score_format() {
        let perfect_gold = ScoreTallies {
            sick: 100,
            total_notes: 100,
            ..Default::default()
        };
        assert_eq!(calculate_rank(&perfect_gold), Some(ScoringRank::PerfectGold));

        let mut buf = [0_u8; 32];
        assert_eq!(format_score_into(&mut buf, 0, false), "Score: 0");
        assert_eq!(format_score_into(&mut buf, 1234560, false), "Score: 1,234,560");
        assert_eq!(format_score_into(&mut buf, -100, false), "Score: -100");
        assert_eq!(format_score_into(&mut buf, 999, true), "Bot Play Enabled");
    }
}
