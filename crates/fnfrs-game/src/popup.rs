//! Pre-allocated judgement rating and combo digit popup pools matching
//! `funkin/play/components/PopUpStuff.hx`.

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::pool::{FixedPool, PoolHandle};
use fnfrs_core::scoring::Judgement;

/// Default pool capacity for active judgement banners (`sick`, `good`, `bad`, `shit`).
pub const JUDGEMENT_POPUP_CAP: usize = 32;

/// Default pool capacity for active combo digit sprites (`0..=9`).
pub const COMBO_DIGIT_POPUP_CAP: usize = 128;

/// Fast deterministic `xorshift32` PRNG used so popup velocity scatter performs zero
/// allocations and needs no external RNG crate.
#[derive(Debug, Clone)]
struct FastRng(u32);

impl FastRng {
    #[must_use]
    const fn new(seed: u32) -> Self {
        Self(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    #[inline]
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    #[inline]
    fn range_i32(&mut self, min_inclusive: i32, max_inclusive: i32) -> i32 {
        let span = (max_inclusive - min_inclusive + 1).max(1) as u32;
        min_inclusive + (self.next_u32() % span) as i32
    }

    #[inline]
    fn range_f32(&mut self, min: f32, max: f32) -> f32 {
        let unit = (self.next_u32() as f32) / (u32::MAX as f32);
        min + (max - min) * unit
    }
}

/// Active judgement banner sprite (`sick`, `good`, `bad`, `shit`, `miss`).
#[derive(Debug, Clone, Copy)]
pub struct JudgementPopUp {
    pub judgement: Judgement,
    pub x: f32,
    pub y: f32,
    pub vel_x: f32,
    pub vel_y: f32,
    pub accel_y: f32,
    pub age_s: f32,
    pub fade_delay_s: f32,
    pub alpha: f32,
    pub is_pixel: bool,
}

impl Default for JudgementPopUp {
    fn default() -> Self {
        Self {
            judgement: Judgement::Sick,
            x: 0.0,
            y: 0.0,
            vel_x: 0.0,
            vel_y: 0.0,
            accel_y: 550.0,
            age_s: 0.0,
            fade_delay_s: 0.5,
            alpha: 1.0,
            is_pixel: false,
        }
    }
}

/// Active combo digit popup sprite (`0..=9`).
#[derive(Debug, Clone, Copy)]
pub struct ComboDigitPopUp {
    pub digit: u8,
    pub x: f32,
    pub y: f32,
    pub vel_x: f32,
    pub vel_y: f32,
    pub accel_y: f32,
    pub age_s: f32,
    pub fade_delay_s: f32,
    pub alpha: f32,
    pub is_pixel: bool,
}

impl Default for ComboDigitPopUp {
    fn default() -> Self {
        Self {
            digit: 0,
            x: 0.0,
            y: 0.0,
            vel_x: 0.0,
            vel_y: 0.0,
            accel_y: 275.0,
            age_s: 0.0,
            fade_delay_s: 1.0,
            alpha: 1.0,
            is_pixel: false,
        }
    }
}

/// Pre-allocated pool managing judgement banners and combo digits with zero
/// per-frame heap allocations (`PopUpStuff.hx`).
#[derive(Debug)]
pub struct PopUpPool {
    pub ratings: FixedPool<JudgementPopUp>,
    pub digits: FixedPool<ComboDigitPopUp>,
    pub offsets: [f32; 2],
    rng: FastRng,
}

impl Default for PopUpPool {
    fn default() -> Self {
        Self::with_capacity(JUDGEMENT_POPUP_CAP, COMBO_DIGIT_POPUP_CAP)
    }
}

impl PopUpPool {
    /// Allocates fixed pools upfront.
    #[must_use]
    pub fn with_capacity(rating_cap: usize, digit_cap: usize) -> Self {
        Self {
            ratings: FixedPool::with_capacity(rating_cap, |_| JudgementPopUp::default()),
            digits: FixedPool::with_capacity(digit_cap, |_| ComboDigitPopUp::default()),
            offsets: [0.0, 0.0],
            rng: FastRng::new(0xF4F5_2026),
        }
    }

    /// Releases all active popups without freeing backing storage.
    pub fn clear(&mut self) {
        self.ratings.clear();
        self.digits.clear();
    }

    /// Spawns a judgement rating sprite (`PopUpStuff.displayRating`).
    pub fn spawn_rating(
        &mut self,
        judgement: Judgement,
        sprite_size: [f32; 2],
        style_offsets: [f32; 2],
        beat_length_ms: f64,
        is_pixel: bool,
    ) {
        let Some((_, slot)) = self.ratings.acquire() else {
            return;
        };

        let mut x = VIRTUAL_WIDTH * 0.474 - sprite_size[0] * 0.5;
        let mut y = (VIRTUAL_HEIGHT * 0.45 - 60.0) - sprite_size[1] * 0.5;
        x += self.offsets[0] + style_offsets[0];
        y += self.offsets[1] + style_offsets[1];

        let vy = -(self.rng.range_i32(140, 175) as f32);
        let vx = -(self.rng.range_i32(0, 10) as f32);

        *slot = JudgementPopUp {
            judgement,
            x,
            y,
            vel_x: vx,
            vel_y: vy,
            accel_y: 550.0,
            age_s: 0.0,
            fade_delay_s: (beat_length_ms * 0.001) as f32,
            alpha: 1.0,
            is_pixel,
        };
    }

    /// Spawns right-to-left combo digit sprites padded to at least 3 digits
    /// (`PopUpStuff.displayCombo`).
    pub fn spawn_combo(
        &mut self,
        combo: u32,
        style_offsets: [f32; 2],
        beat_length_ms: f64,
        is_pixel: bool,
    ) {
        let mut digits_buf = [0u8; 10];
        let mut count = 0usize;
        let mut tmp = combo;

        while tmp != 0 && count < digits_buf.len() {
            digits_buf[count] = (tmp % 10) as u8;
            tmp /= 10;
            count += 1;
        }
        while count < 3 {
            digits_buf[count] = 0;
            count += 1;
        }

        let fade_delay_s = (beat_length_ms * 0.002) as f32;
        for (i, &digit) in digits_buf.iter().take(count).enumerate() {
            let Some((_, slot)) = self.digits.acquire() else {
                break;
            };
            let da_loop = (i + 1) as f32;
            let x = (VIRTUAL_WIDTH * 0.507) - (36.0 * da_loop) - 65.0
                + self.offsets[0]
                + style_offsets[0];
            let y = (VIRTUAL_HEIGHT * 0.44) + self.offsets[1] + style_offsets[1];

            *slot = ComboDigitPopUp {
                digit,
                x,
                y,
                vel_x: self.rng.range_f32(-5.0, 5.0),
                vel_y: -(self.rng.range_i32(130, 150) as f32),
                accel_y: self.rng.range_i32(250, 300) as f32,
                age_s: 0.0,
                fade_delay_s,
                alpha: 1.0,
                is_pixel,
            };
        }
    }

    /// Integrates popup physics and 0.2s fade tweens with zero heap allocations.
    pub fn update(&mut self, elapsed_s: f32) {
        const FADE_DURATION_S: f32 = 0.2;

        let mut expired_ratings: [Option<PoolHandle>; JUDGEMENT_POPUP_CAP] =
            [None; JUDGEMENT_POPUP_CAP];
        let mut expired_r_len = 0usize;

        for (handle, item) in self.ratings.iter_alive_mut() {
            item.vel_y += item.accel_y * elapsed_s;
            item.x += item.vel_x * elapsed_s;
            item.y += item.vel_y * elapsed_s;
            item.age_s += elapsed_s;

            if item.age_s >= item.fade_delay_s {
                let raw_t = ((item.age_s - item.fade_delay_s) / FADE_DURATION_S).clamp(0.0, 1.0);
                let t = if item.is_pixel {
                    (raw_t * 2.0).floor() / 2.0
                } else {
                    raw_t
                };
                item.alpha = (1.0 - t).max(0.0);
                if raw_t >= 1.0 && expired_r_len < expired_ratings.len() {
                    expired_ratings[expired_r_len] = Some(handle);
                    expired_r_len += 1;
                }
            }
        }

        for handle in expired_ratings.iter().take(expired_r_len).flatten() {
            self.ratings.release(*handle);
        }

        let mut expired_digits: [Option<PoolHandle>; COMBO_DIGIT_POPUP_CAP] =
            [None; COMBO_DIGIT_POPUP_CAP];
        let mut expired_d_len = 0usize;

        for (handle, item) in self.digits.iter_alive_mut() {
            item.vel_y += item.accel_y * elapsed_s;
            item.x += item.vel_x * elapsed_s;
            item.y += item.vel_y * elapsed_s;
            item.age_s += elapsed_s;

            if item.age_s >= item.fade_delay_s {
                let raw_t = ((item.age_s - item.fade_delay_s) / FADE_DURATION_S).clamp(0.0, 1.0);
                let t = if item.is_pixel {
                    (raw_t * 2.0).floor() / 2.0
                } else {
                    raw_t
                };
                item.alpha = (1.0 - t).max(0.0);
                if raw_t >= 1.0 && expired_d_len < expired_digits.len() {
                    expired_digits[expired_d_len] = Some(handle);
                    expired_d_len += 1;
                }
            }
        }

        for handle in expired_digits.iter().take(expired_d_len).flatten() {
            self.digits.release(*handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combo_pads_to_three_digits_and_expires() {
        let mut pool = PopUpPool::default();
        pool.spawn_rating(Judgement::Sick, [400.0, 150.0], [0.0, 0.0], 500.0, false);
        pool.spawn_combo(7, [0.0, 0.0], 500.0, false);

        assert_eq!(pool.ratings.active_count(), 1);
        assert_eq!(pool.digits.active_count(), 3);

        // Run past delay (1.0s max) + fade (0.2s).
        for _ in 0..80 {
            pool.update(1.0 / 60.0);
        }

        assert_eq!(pool.ratings.active_count(), 0);
        assert_eq!(pool.digits.active_count(), 0);
    }
}
