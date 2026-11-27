use std::f32::consts::PI;
use std::sync::Arc;

use fnfrs_audio::{DecodedAudio, VizSampleHistory};

const VIZ_FFT_N: usize = 256;
const VIZ_HALF_N: usize = VIZ_FFT_N / 2;
const VIZ_BAR_COUNT: usize = 7;
const VIZ_MIN_DB: f32 = -65.0;
const VIZ_DB_RANGE: f32 = 40.0;
const VIZ_MIN_FREQ: f32 = 10.0;
const VIZ_MAX_FREQ: f32 = 22000.0;
const VIZ_MAX_DELTA: f32 = 0.1;
const VIZ_DB_PER_LN: f32 = 8.685_889;
const VIZ_FFT_NORM: f32 = (2.0 / VIZ_FFT_N as f32) * (8.0 / 12.0);

/// Zero-allocation radix-2 FFT & 7-band logarithmic spectrum analyzer
/// matching `FunkinCrew/funkVis` (`SpectralAnalyzer` + `ABotVis.hx`).
#[derive(Debug)]
pub(super) struct ABotSpectralAnalyzer {
    pub(super) inst_audio: Option<(DecodedAudio, i64)>,
    pub(super) viz_history: Option<Arc<VizSampleHistory>>,
    pub(super) viz_sample_rate_hz: u32,
    pub(super) audible: bool,
    window: [f32; VIZ_FFT_N],
    bit_rev: [u8; VIZ_FFT_N],
    twiddle_cos: [f32; VIZ_HALF_N],
    twiddle_sin: [f32; VIZ_HALF_N],
    mono_scratch: [f32; VIZ_FFT_N],
    re: [f32; VIZ_FFT_N],
    im: [f32; VIZ_FFT_N],
    mag: [f32; VIZ_HALF_N],
    bar_values: [f32; VIZ_BAR_COUNT],
}

impl Default for ABotSpectralAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl ABotSpectralAnalyzer {
    pub(super) fn new() -> Self {
        let mut window = [0.0_f32; VIZ_FFT_N];
        let mut bit_rev = [0_u8; VIZ_FFT_N];
        let mut twiddle_cos = [0.0_f32; VIZ_HALF_N];
        let mut twiddle_sin = [0.0_f32; VIZ_HALF_N];
        for i in 0..VIZ_FFT_N {
            window[i] = 1.0 - 0.85 * (2.0 * PI * (i as f32) / (VIZ_FFT_N as f32)).cos();
            bit_rev[i] = (i as u8).reverse_bits();
        }
        for k in 0..VIZ_HALF_N {
            let (sin_a, cos_a) = (-2.0 * PI * (k as f32) / (VIZ_FFT_N as f32)).sin_cos();
            twiddle_cos[k] = cos_a;
            twiddle_sin[k] = sin_a;
        }
        Self {
            inst_audio: None,
            viz_history: None,
            viz_sample_rate_hz: 44_100,
            audible: true,
            window,
            bit_rev,
            twiddle_cos,
            twiddle_sin,
            mono_scratch: [0.0; VIZ_FFT_N],
            re: [0.0; VIZ_FFT_N],
            im: [0.0; VIZ_FFT_N],
            mag: [0.0; VIZ_HALF_N],
            bar_values: [0.0; VIZ_BAR_COUNT],
        }
    }

    pub(super) fn reset(&mut self) {
        self.bar_values = [0.0; VIZ_BAR_COUNT];
    }

    pub(super) fn analyze(
        &mut self,
        song_pos_ms: f64,
        elapsed_s: f32,
        out_levels: &mut [u8; VIZ_BAR_COUNT],
    ) {
        if !self.audible || song_pos_ms < 0.0 {
            self.bar_values = [0.0; VIZ_BAR_COUNT];
            *out_levels = [0; VIZ_BAR_COUNT];
            return;
        }

        let sr_f32 = if let Some(history) = &self.viz_history {
            if !history.copy_recent(&mut self.mono_scratch) {
                self.bar_values = [0.0; VIZ_BAR_COUNT];
                *out_levels = [0; VIZ_BAR_COUNT];
                return;
            }
            for i in 0..VIZ_FFT_N {
                let dst = self.bit_rev[i] as usize;
                self.re[dst] = self.mono_scratch[i] * self.window[i];
                self.im[dst] = 0.0;
            }
            self.viz_sample_rate_hz.max(1) as f32
        } else {
            let Some((audio, offset_frames)) = &self.inst_audio else {
                self.bar_values = [0.0; VIZ_BAR_COUNT];
                *out_levels = [0; VIZ_BAR_COUNT];
                return;
            };
            if audio.frames == 0 || audio.sample_rate == 0 {
                *out_levels = [0; VIZ_BAR_COUNT];
                return;
            }

            let sr_f64 = f64::from(audio.sample_rate);
            let end_frame = ((song_pos_ms / 1000.0) * sr_f64).round() as i64 - *offset_frames;
            if end_frame <= 0 {
                self.bar_values = [0.0; VIZ_BAR_COUNT];
                *out_levels = [0; VIZ_BAR_COUNT];
                return;
            }
            let start_frame = end_frame - (VIZ_FFT_N as i64);

            for i in 0..VIZ_FFT_N {
                let f = start_frame + (i as i64);
                let mono = if f >= 0 && (f as usize) < audio.frames {
                    let s_idx = (f as usize) * 2;
                    (audio.samples[s_idx] + audio.samples[s_idx + 1]) * 0.5
                } else {
                    0.0
                };
                let dst = self.bit_rev[i] as usize;
                self.re[dst] = mono * self.window[i];
                self.im[dst] = 0.0;
            }
            audio.sample_rate as f32
        };

        // In-place Cooley-Tukey radix-2 DIT FFT with precomputed twiddle LUT (zero heap allocs).
        let mut size = 2usize;
        while size <= VIZ_FFT_N {
            let half = size / 2;
            let stride = VIZ_FFT_N / size;
            let mut start = 0usize;
            while start < VIZ_FFT_N {
                for j in 0..half {
                    let tw = j * stride;
                    let cos_a = self.twiddle_cos[tw];
                    let sin_a = self.twiddle_sin[tw];
                    let even = start + j;
                    let odd = even + half;
                    let t_re = self.re[odd] * cos_a - self.im[odd] * sin_a;
                    let t_im = self.re[odd] * sin_a + self.im[odd] * cos_a;
                    self.re[odd] = self.re[even] - t_re;
                    self.im[odd] = self.im[even] - t_im;
                    self.re[even] += t_re;
                    self.im[even] += t_im;
                }
                start += size;
            }
            size <<= 1;
        }

        for k in 0..VIZ_HALF_N {
            self.mag[k] =
                (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt() * VIZ_FFT_NORM;
        }

        let freq_ratio = VIZ_MAX_FREQ / VIZ_MIN_FREQ;
        let dt_frames = (elapsed_s.max(0.0) * 60.0).clamp(0.0, 4.0);
        let max_fall = VIZ_MAX_DELTA * dt_frames.max(0.25);

        for b in 0..VIZ_BAR_COUNT {
            let f_lo = VIZ_MIN_FREQ * freq_ratio.powf((b as f32) / (VIZ_BAR_COUNT as f32));
            let f_hi =
                VIZ_MIN_FREQ * freq_ratio.powf(((b + 1) as f32) / (VIZ_BAR_COUNT as f32));
            let bin_lo = (f_lo * (VIZ_FFT_N as f32) / sr_f32).clamp(0.0, (VIZ_HALF_N - 1) as f32);
            let bin_hi =
                (f_hi * (VIZ_FFT_N as f32) / sr_f32).clamp(bin_lo, (VIZ_HALF_N - 1) as f32);

            let mid = ((bin_lo + bin_hi) * 0.5)
                .max(0.75)
                .clamp(0.0, (VIZ_HALF_N - 1) as f32);
            let m0 = mid.floor() as usize;
            let m1 = (m0 + 1).min(VIZ_HALF_N - 1);
            let frac = mid - (m0 as f32);
            let mut amp = self.mag[m0] * (1.0 - frac) + self.mag[m1] * frac;

            let i_lo = (bin_lo.ceil() as usize).max(1);
            let i_hi = (bin_hi.floor() as usize).min(VIZ_HALF_N - 1);
            if i_lo <= i_hi {
                for &m in &self.mag[i_lo..=i_hi] {
                    if m > amp {
                        amp = m;
                    }
                }
            }

            let db = if amp > 1e-12 {
                VIZ_DB_PER_LN * amp.ln()
            } else {
                VIZ_MIN_DB
            };
            let raw_norm = ((db - VIZ_MIN_DB) / VIZ_DB_RANGE).clamp(0.0, 1.0);
            let prev = self.bar_values[b];
            let next = if raw_norm >= prev {
                raw_norm
            } else {
                (prev - max_fall).max(raw_norm)
            };
            self.bar_values[b] = next;
            out_levels[b] = (next * 6.0).round().clamp(0.0, 6.0) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abot_analyzer_fft_responds_to_sine_and_respects_mute_and_preroll() {
        let sr = 44100u32;
        let frames = 44100usize;
        let mut samples = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let s = (2.0 * PI * 440.0 * (i as f32) / (sr as f32)).sin() * 0.6;
            samples.push(s);
            samples.push(s);
        }
        let audio = DecodedAudio {
            samples: Arc::from(samples),
            frames,
            sample_rate: sr,
        };
        let mut analyzer = ABotSpectralAnalyzer::new();
        analyzer.inst_audio = Some((audio, 0));

        let mut levels = [0u8; 7];
        analyzer.analyze(-500.0, 1.0 / 60.0, &mut levels);
        assert_eq!(levels, [0; 7]);

        analyzer.analyze(500.0, 1.0 / 60.0, &mut levels);
        assert!(levels[3] >= 3, "expected 440Hz bar activation, got {levels:?}");

        analyzer.audible = false;
        analyzer.analyze(500.0, 1.0 / 60.0, &mut levels);
        assert_eq!(levels, [0; 7]);
    }

    #[test]
    fn abot_analyzer_reads_streaming_viz_history() {
        let sr = 44100u32;
        let hist = Arc::new(VizSampleHistory::new());
        for i in 0..512 {
            let s = (2.0 * PI * 440.0 * (i as f32) / (sr as f32)).sin() * 0.6;
            hist.push_mono(s);
        }

        let mut analyzer = ABotSpectralAnalyzer::new();
        analyzer.viz_history = Some(hist);
        analyzer.viz_sample_rate_hz = sr;

        let mut levels = [0u8; 7];
        analyzer.analyze(100.0, 1.0 / 60.0, &mut levels);
        assert!(levels[3] >= 3, "expected 440Hz bar activation from stream ring, got {levels:?}");
    }
}
