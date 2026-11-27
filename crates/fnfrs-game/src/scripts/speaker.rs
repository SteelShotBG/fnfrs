use fnfrs_core::schema::song::SongNoteData;

use super::ScriptRng;
use crate::character::BaseCharacter;
#[derive(Debug, Default)]
pub(super) struct SpeakerShooter {
    enabled: bool,
    notes: Vec<(f64, u8)>,
    cursor: usize,
}

impl SpeakerShooter {
    pub(super) fn new(gf_id: &str, picospeaker_notes: &[SongNoteData]) -> Self {
        let enabled = gf_id.eq_ignore_ascii_case("pico-speaker")
            || gf_id.eq_ignore_ascii_case("otis-speaker");
        if !enabled {
            return Self::default();
        }
        let mut notes: Vec<(f64, u8)> = picospeaker_notes
            .iter()
            .map(|n| (n.time, n.data as u8))
            .collect();
        notes.sort_by(|a, b| a.0.total_cmp(&b.0));
        Self {
            enabled,
            notes,
            cursor: 0,
        }
    }

    pub(super) fn reset(&mut self) {
        self.cursor = 0;
    }

    pub(super) fn update(&mut self, song_pos_ms: f64, gf: Option<&mut BaseCharacter>, rng: &mut ScriptRng) -> Option<usize> {
        if !self.enabled {
            return None;
        }
        let gf = gf?;
        let mut fired_dir = None;
        while self.cursor < self.notes.len() && self.notes[self.cursor].0 <= song_pos_ms {
            let raw_dir = self.notes[self.cursor].1;
            self.cursor += 1;
            let delta = rng.range_i32(0, 1);
            let dir = if raw_dir == 3 {
                (3 - delta).clamp(0, 3)
            } else {
                ((raw_dir as i32) + delta).clamp(0, 3)
            };
            let anim = match dir {
                0 => "shoot1",
                1 => "shoot2",
                2 => "shoot3",
                _ => "shoot4",
            };
            gf.play_animation(anim, true, true);
            fired_dir = Some(dir as usize);
        }
        fired_dir
    }
}
