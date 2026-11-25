use crate::constants::{
    DEFAULT_BPM, DEFAULT_TIME_SIGNATURE_DEN, DEFAULT_TIME_SIGNATURE_NUM, MS_PER_SEC, SECS_PER_MIN,
    STEPS_PER_BEAT,
};
use crate::schema::song::SongTimeChange;

/// Step/beat/measure transitions emitted by [`Conductor::update`] without heap allocations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConductorEvents {
    pub step_hit: bool,
    pub beat_hit: bool,
    pub measure_hit: bool,
}

/// Musical timing state mirroring `funkin/Conductor.hx`.
#[derive(Debug, Clone)]
pub struct Conductor {
    time_changes: Vec<SongTimeChange>,
    current_time_change_idx: Option<usize>,
    bpm_override: Option<f64>,

    pub song_position_ms: f64,
    pub song_position_delta_ms: f64,

    pub current_measure: i32,
    pub current_beat: i32,
    pub current_step: i32,

    pub current_measure_time: f64,
    pub current_beat_time: f64,
    pub current_step_time: f64,

    /// Chart-level instrumental offset (ms).
    pub instrumental_offset_ms: f64,
    /// Audio codec delay compensation (e.g. `MP3_DELAY_MS`, or `0.0` for OGG).
    pub format_offset_ms: f64,
    /// User-configured hardware input offset (ms).
    pub global_offset_ms: f64,
}

impl Default for Conductor {
    fn default() -> Self {
        Self::new()
    }
}

impl Conductor {
    /// Creates a new [`Conductor`] initialized at 100 BPM 4/4.
    #[must_use]
    pub fn new() -> Self {
        Self {
            time_changes: Vec::new(),
            current_time_change_idx: None,
            bpm_override: None,
            song_position_ms: 0.0,
            song_position_delta_ms: 0.0,
            current_measure: 0,
            current_beat: 0,
            current_step: 0,
            current_measure_time: 0.0,
            current_beat_time: 0.0,
            current_step_time: 0.0,
            instrumental_offset_ms: 0.0,
            format_offset_ms: 0.0,
            global_offset_ms: 0.0,
        }
    }

    /// Returns the combined offset (`instrumental + format + global`) in milliseconds.
    #[inline]
    #[must_use]
    pub fn combined_offset_ms(&self) -> f64 {
        self.instrumental_offset_ms + self.format_offset_ms + self.global_offset_ms
    }

    /// Forces a fixed BPM override, or passes `None` to resume using `time_changes`.
    pub fn force_bpm(&mut self, bpm: Option<f64>) {
        self.bpm_override = bpm;
    }

    /// Returns the active [`SongTimeChange`] at the current song position, if any.
    #[must_use]
    pub fn current_time_change(&self) -> Option<&SongTimeChange> {
        self.current_time_change_idx
            .and_then(|idx| self.time_changes.get(idx))
    }

    /// Current beats per minute.
    #[must_use]
    pub fn bpm(&self) -> f64 {
        if let Some(forced) = self.bpm_override {
            return forced;
        }
        self.current_time_change().map_or(DEFAULT_BPM, |tc| tc.bpm)
    }

    /// Starting BPM of the song.
    #[must_use]
    pub fn starting_bpm(&self) -> f64 {
        if let Some(forced) = self.bpm_override {
            return forced;
        }
        self.time_changes.first().map_or(DEFAULT_BPM, |tc| tc.bpm)
    }

    /// Current time signature numerator (e.g. `4` in `4/4`).
    #[must_use]
    pub fn time_signature_numerator(&self) -> u32 {
        self.current_time_change()
            .map_or(DEFAULT_TIME_SIGNATURE_NUM, |tc| tc.time_signature_num)
    }

    /// Current time signature denominator (e.g. `4` in `4/4`).
    #[must_use]
    pub fn time_signature_denominator(&self) -> u32 {
        self.current_time_change()
            .map_or(DEFAULT_TIME_SIGNATURE_DEN, |tc| tc.time_signature_den)
    }

    /// Duration of one beat in milliseconds.
    #[must_use]
    pub fn beat_length_ms(&self) -> f64 {
        beat_length_for(self.bpm(), self.time_signature_denominator())
    }

    /// Duration of one step in milliseconds.
    #[must_use]
    pub fn step_length_ms(&self) -> f64 {
        self.beat_length_ms() / f64::from(STEPS_PER_BEAT)
    }

    /// Duration of one measure in milliseconds.
    #[must_use]
    pub fn measure_length_ms(&self) -> f64 {
        self.beat_length_ms() * f64::from(self.time_signature_numerator())
    }

    /// Number of steps in the current measure.
    #[must_use]
    pub fn steps_per_measure(&self) -> u32 {
        self.time_signature_numerator() * STEPS_PER_BEAT
    }

    /// Maps and sorts `SongTimeChange` entries, computing cumulative `beat_time` values.
    pub fn map_time_changes(&mut self, input: &[SongTimeChange]) {
        self.time_changes.clear();
        self.time_changes.extend(input.iter().cloned());
        self.time_changes
            .sort_by(|a, b| a.time_stamp.total_cmp(&b.time_stamp));

        let mut prev_bpm = DEFAULT_BPM;
        for i in 0..self.time_changes.len() {
            if self.time_changes[i].time_stamp < 0.0 {
                self.time_changes[i].time_stamp = 0.0;
            }
            if self.time_changes[i].bpm <= 0.0 {
                self.time_changes[i].bpm = prev_bpm;
            } else {
                prev_bpm = self.time_changes[i].bpm;
            }

            if self.time_changes[i].time_stamp <= 0.0 || i == 0 {
                self.time_changes[i].beat_time = 0.0;
            } else {
                let prev = &self.time_changes[i - 1];
                let delta_ms = self.time_changes[i].time_stamp - prev.time_stamp;
                let beats = prev.beat_time
                    + (delta_ms * prev.bpm / SECS_PER_MIN / MS_PER_SEC
                        * (f64::from(prev.time_signature_den) / 4.0));
                self.time_changes[i].beat_time = round_decimal(beats, 4);
            }
        }

        let pos = self.song_position_ms;
        let _ = self.update(pos, false);
    }

    /// Updates `song_position_ms`, active time change, and step/beat/measure counters.
    /// Zero heap allocations.
    pub fn update(&mut self, raw_song_pos_ms: f64, apply_offsets: bool) -> ConductorEvents {
        let song_pos = if apply_offsets {
            raw_song_pos_ms + self.combined_offset_ms()
        } else {
            raw_song_pos_ms
        };
        self.song_position_ms = song_pos;

        let old_measure = self.current_measure;
        let old_beat = self.current_beat;
        let old_step = self.current_step;

        if self.time_changes.is_empty() {
            self.current_time_change_idx = None;
        } else {
            let mut active_idx = 0;
            if song_pos > 0.0 {
                for (i, tc) in self.time_changes.iter().enumerate() {
                    if song_pos >= tc.time_stamp {
                        active_idx = i;
                    } else {
                        break;
                    }
                }
            }
            self.current_time_change_idx = Some(active_idx);
        }

        if let Some(tc) = self.current_time_change().cloned() {
            if song_pos > 0.0 {
                let step_len = self.step_length_ms();
                self.current_step_time = round_decimal(
                    (tc.beat_time * f64::from(STEPS_PER_BEAT))
                        + (song_pos - tc.time_stamp) / step_len,
                    6,
                );
                self.current_beat_time = self.current_step_time / f64::from(STEPS_PER_BEAT);
                self.current_measure_time = self.get_time_in_measures(song_pos);
                self.current_step = self.current_step_time.floor() as i32;
                self.current_beat = self.current_beat_time.floor() as i32;
                self.current_measure = self.current_measure_time.floor() as i32;
            } else {
                self.update_constant_bpm(song_pos);
            }
        } else {
            self.update_constant_bpm(song_pos);
        }

        ConductorEvents {
            step_hit: self.current_step != old_step,
            beat_hit: self.current_beat != old_beat,
            measure_hit: self.current_measure != old_measure,
        }
    }

    fn update_constant_bpm(&mut self, song_pos: f64) {
        let step_len = self.step_length_ms();
        let steps_per_meas = f64::from(self.steps_per_measure());
        self.current_step_time = round_decimal(song_pos / step_len, 4);
        self.current_beat_time = self.current_step_time / f64::from(STEPS_PER_BEAT);
        self.current_measure_time = self.current_step_time / steps_per_meas;
        self.current_step = self.current_step_time.floor() as i32;
        self.current_beat = self.current_beat_time.floor() as i32;
        self.current_measure = self.current_measure_time.floor() as i32;
    }

    /// Returns `song_position_ms + song_position_delta_ms` for smooth high-FPS note scrolling.
    #[inline]
    #[must_use]
    pub fn time_with_delta_ms(&self) -> f64 {
        self.song_position_ms + self.song_position_delta_ms
    }

    /// Converts a timestamp in milliseconds to fractional steps.
    #[must_use]
    pub fn get_time_in_steps(&self, ms: f64) -> f64 {
        if self.time_changes.is_empty() {
            return (ms / self.step_length_ms()).floor();
        }
        let ms = ms.max(0.0);
        let mut result_step = 0.0;
        let mut last_tc = &self.time_changes[0];

        for tc in &self.time_changes {
            if ms >= tc.time_stamp {
                result_step += (tc.beat_time - last_tc.beat_time) * f64::from(STEPS_PER_BEAT);
                last_tc = tc;
            } else {
                break;
            }
        }

        let last_step_len =
            beat_length_for(last_tc.bpm, last_tc.time_signature_den) / f64::from(STEPS_PER_BEAT);
        result_step + (ms - last_tc.time_stamp) / last_step_len
    }

    /// Converts a step time to milliseconds.
    #[must_use]
    pub fn get_step_time_in_ms(&self, step_time: f64) -> f64 {
        if self.time_changes.is_empty() {
            return step_time * self.step_length_ms();
        }
        let step_time = step_time.max(0.0);
        let mut result_ms = 0.0;
        let mut last_tc = &self.time_changes[0];

        for tc in &self.time_changes {
            let tc_step = tc.beat_time * f64::from(STEPS_PER_BEAT);
            if step_time >= tc_step {
                result_ms += tc.time_stamp - last_tc.time_stamp;
                last_tc = tc;
            } else {
                break;
            }
        }

        let last_step_len =
            beat_length_for(last_tc.bpm, last_tc.time_signature_den) / f64::from(STEPS_PER_BEAT);
        result_ms + (step_time - last_tc.beat_time * f64::from(STEPS_PER_BEAT)) * last_step_len
    }

    /// Converts a beat time to milliseconds.
    #[must_use]
    pub fn get_beat_time_in_ms(&self, beat_time: f64) -> f64 {
        if self.time_changes.is_empty() {
            return beat_time * self.beat_length_ms();
        }
        let mut last_tc = &self.time_changes[0];
        let mut result_ms = 0.0;

        for tc in &self.time_changes {
            if beat_time >= tc.beat_time {
                last_tc = tc;
                result_ms = tc.time_stamp;
            } else {
                break;
            }
        }

        let last_beat_len = beat_length_for(last_tc.bpm, last_tc.time_signature_den);
        result_ms + (beat_time - last_tc.beat_time) * last_beat_len
    }

    /// Converts a timestamp in milliseconds to fractional measures.
    #[must_use]
    pub fn get_time_in_measures(&self, ms: f64) -> f64 {
        if self.time_changes.is_empty() {
            return ms / self.measure_length_ms();
        }
        let ms = ms.max(0.0);
        let mut result_measures = 0.0;
        let mut last_tc = &self.time_changes[0];

        for tc in &self.time_changes {
            if ms >= tc.time_stamp {
                let meas_len = beat_length_for(last_tc.bpm, last_tc.time_signature_den)
                    * f64::from(last_tc.time_signature_num);
                result_measures += (tc.time_stamp - last_tc.time_stamp) / meas_len;
                last_tc = tc;
            } else {
                break;
            }
        }

        let remaining_meas_len = beat_length_for(last_tc.bpm, last_tc.time_signature_den)
            * f64::from(last_tc.time_signature_num);
        result_measures + (ms - last_tc.time_stamp) / remaining_meas_len
    }
}

#[inline]
fn beat_length_for(bpm: f64, time_sig_den: u32) -> f64 {
    ((SECS_PER_MIN / bpm) * MS_PER_SEC) * (4.0 / f64::from(time_sig_den.max(1)))
}

#[inline]
fn round_decimal(value: f64, precision: i32) -> f64 {
    let mult = 10_f64.powi(precision);
    (value * mult).round() / mult
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_bpm_beats_and_steps() {
        let mut conductor = Conductor::new();
        conductor.map_time_changes(&[SongTimeChange {
            time_stamp: 0.0,
            bpm: 120.0,
            ..Default::default()
        }]);

        assert!((conductor.beat_length_ms() - 500.0).abs() < 1e-6);
        assert!((conductor.step_length_ms() - 125.0).abs() < 1e-6);

        let ev = conductor.update(1000.0, false);
        assert!(ev.beat_hit);
        assert!(ev.step_hit);
        assert_eq!(conductor.current_beat, 2);
        assert_eq!(conductor.current_step, 8);
    }

    #[test]
    fn multi_bpm_time_changes_match_haxe() {
        let mut conductor = Conductor::new();
        conductor.map_time_changes(&[
            SongTimeChange {
                time_stamp: 0.0,
                bpm: 120.0, // 500ms/beat => 4 beats in 2000ms
                ..Default::default()
            },
            SongTimeChange {
                time_stamp: 2000.0,
                bpm: 240.0, // 250ms/beat => 4 beats in next 1000ms
                ..Default::default()
            },
        ]);

        let _ = conductor.update(3000.0, false);
        assert!((conductor.bpm() - 240.0).abs() < 1e-6);
        assert_eq!(conductor.current_beat, 8);
        assert_eq!(conductor.current_step, 32);
        assert!((conductor.get_beat_time_in_ms(8.0) - 3000.0).abs() < 1e-6);
        assert!((conductor.get_step_time_in_ms(32.0) - 3000.0).abs() < 1e-6);
        assert!((conductor.get_time_in_steps(3000.0) - 32.0).abs() < 1e-6);
        assert!((conductor.get_time_in_measures(3000.0) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn negative_countdown_update_does_not_panic() {
        let mut conductor = Conductor::new();
        conductor.map_time_changes(&[SongTimeChange::default()]);
        let _ = conductor.update(-2400.0, false);
        assert!(conductor.current_beat < 0);
    }
}
