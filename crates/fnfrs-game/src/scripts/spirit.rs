use fnfrs_render::SpriteBatcher;

use crate::character::BaseCharacter;
#[derive(Debug, Clone, Copy)]
struct SpiritGhostSample {
    valid: bool,
    anim_index: usize,
    frame_index: usize,
    position: [f32; 2],
    anim_offsets: [f32; 2],
    flip_x: bool,
}

impl Default for SpiritGhostSample {
    fn default() -> Self {
        Self {
            valid: false,
            anim_index: 0,
            frame_index: 0,
            position: [0.0, 0.0],
            anim_offsets: [0.0, 0.0],
            flip_x: false,
        }
    }
}

#[derive(Debug)]
pub(super) struct SpiritTrail {
    pub(super) enabled: bool,
    pub(super) z_index: i32,
    delay_s: f32,
    base_alpha: f32,
    alpha_diff: f32,
    timer_s: f32,
    samples: [SpiritGhostSample; 4],
}

impl Default for SpiritTrail {
    fn default() -> Self {
        Self {
            enabled: false,
            z_index: 190,
            delay_s: 0.4,
            base_alpha: 0.3,
            alpha_diff: 0.069,
            timer_s: 0.0,
            samples: [SpiritGhostSample::default(); 4],
        }
    }
}

impl SpiritTrail {
    pub(super) fn reset(&mut self) {
        self.timer_s = 0.0;
        self.samples = [SpiritGhostSample::default(); 4];
    }

    pub(super) fn update(&mut self, elapsed_s: f32, dad: Option<&BaseCharacter>) {
        if !self.enabled {
            return;
        }
        let Some(dad) = dad else {
            return;
        };
        self.timer_s += elapsed_s.max(0.0);
        if self.timer_s < self.delay_s {
            return;
        }
        self.timer_s -= self.delay_s;

        // Shift trail buffer backward without allocating.
        for i in (1..self.samples.len()).rev() {
            self.samples[i] = self.samples[i - 1];
        }
        if let Some((anim_idx, frame_idx, offsets)) = dad.active_frame_snapshot() {
            self.samples[0] = SpiritGhostSample {
                valid: true,
                anim_index: anim_idx,
                frame_index: frame_idx,
                position: dad.position,
                anim_offsets: offsets,
                flip_x: dad.flip_x,
            };
        }
    }

    pub(super) fn draw(&self, batcher: &mut SpriteBatcher, cam_pos: [f32; 2], cam_zoom: f32, dad: Option<&BaseCharacter>) {
        if !self.enabled {
            return;
        }
        let Some(dad) = dad else {
            return;
        };
        for (i, sample) in self.samples.iter().enumerate().rev() {
            if !sample.valid {
                continue;
            }
            let alpha = (self.base_alpha - self.alpha_diff * (i as f32)).max(0.0);
            if alpha <= 0.001 {
                continue;
            }
            dad.draw_snapshot_ghost(
                batcher,
                cam_pos,
                cam_zoom,
                sample.anim_index,
                sample.frame_index,
                sample.position,
                sample.anim_offsets,
                sample.flip_x,
                alpha,
            );
        }
    }
}
