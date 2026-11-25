use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Shared atomic counters published by the real-time audio callback.
#[derive(Debug)]
pub struct SampleClockShared {
    played_frames: AtomicU64,
    generation: AtomicU64,
    is_playing: AtomicBool,
    sample_rate: AtomicU32,
    playback_rate_bits: AtomicU32,
}

impl SampleClockShared {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            played_frames: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            is_playing: AtomicBool::new(false),
            sample_rate: AtomicU32::new(sample_rate.max(1)),
            playback_rate_bits: AtomicU32::new(1.0_f32.to_bits()),
        }
    }

    /// Updates played sample frames from the audio thread (`Ordering::Release`).
    #[inline]
    pub fn store_state(&self, played_frames: u64, is_playing: bool) {
        self.played_frames.store(played_frames, Ordering::Release);
        self.is_playing.store(is_playing, Ordering::Release);
    }

    /// Bumps the transport generation when loading or seeking a song.
    #[inline]
    pub fn bump_generation(&self, played_frames: u64, is_playing: bool) {
        self.played_frames.store(played_frames, Ordering::Release);
        self.is_playing.store(is_playing, Ordering::Release);
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Sets playback speed multiplier (`1.0` = normal speed).
    #[inline]
    pub fn set_playback_rate(&self, rate: f32) {
        self.playback_rate_bits
            .store(rate.max(0.01).to_bits(), Ordering::Release);
    }

    /// Returns device output sample rate in Hz.
    #[inline]
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Acquire)
    }

    /// Returns current played sample frames.
    #[inline]
    #[must_use]
    pub fn played_frames(&self) -> u64 {
        self.played_frames.load(Ordering::Acquire)
    }

    /// Returns whether transport is currently playing.
    #[inline]
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.is_playing.load(Ordering::Acquire)
    }

    /// Returns playback speed multiplier.
    #[inline]
    #[must_use]
    pub fn playback_rate(&self) -> f32 {
        f32::from_bits(self.playback_rate_bits.load(Ordering::Acquire))
    }

    /// Returns current transport generation counter.
    #[inline]
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
}

/// Main-thread / input-thread view of the atomic sample clock with `Instant`
/// sub-buffer interpolation for jitter-free scrolling and sub-millisecond input timing.
#[derive(Debug, Clone)]
pub struct SampleClock {
    shared: Arc<SampleClockShared>,
    last_frames: u64,
    last_generation: u64,
    anchor: Instant,
    max_interp_ms: f64,
}

impl SampleClock {
    /// Creates a [`SampleClock`] backed by `shared`.
    #[must_use]
    pub fn new(shared: Arc<SampleClockShared>) -> Self {
        Self {
            last_frames: shared.played_frames(),
            last_generation: shared.generation(),
            shared,
            anchor: Instant::now(),
            max_interp_ms: 40.0,
        }
    }

    /// Polls atomic counters and refreshes the `Instant` anchor whenever the audio
    /// callback has advanced `played_frames` or reset the transport generation.
    pub fn sync_anchor(&mut self) {
        let gen = self.shared.generation();
        let frames = self.shared.played_frames();
        if gen != self.last_generation || frames != self.last_frames {
            self.last_generation = gen;
            self.last_frames = frames;
            self.anchor = Instant::now();
        }
    }

    /// Returns the base sample-clock position in milliseconds without interpolation.
    #[must_use]
    pub fn raw_ms(&self) -> f64 {
        let frames = self.shared.played_frames();
        let rate = f64::from(self.shared.sample_rate().max(1));
        (frames as f64 / rate) * 1000.0
    }

    /// Returns the interpolated song position in milliseconds at `now`.
    #[must_use]
    pub fn position_ms_at(&mut self, at: Instant) -> f64 {
        self.sync_anchor();
        let base_ms =
            (self.last_frames as f64 / f64::from(self.shared.sample_rate().max(1))) * 1000.0;
        if !self.shared.is_playing() {
            return base_ms;
        }
        let delta_ms = if at >= self.anchor {
            (at - self.anchor).as_secs_f64() * 1000.0 * f64::from(self.shared.playback_rate())
        } else {
            -((self.anchor - at).as_secs_f64() * 1000.0 * f64::from(self.shared.playback_rate()))
        };
        (base_ms + delta_ms.clamp(-self.max_interp_ms, self.max_interp_ms)).max(0.0)
    }

    /// Returns the interpolated song position in milliseconds at `Instant::now()`.
    #[must_use]
    pub fn now_ms(&mut self) -> f64 {
        self.position_ms_at(Instant::now())
    }

    /// Returns whether transport is playing.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.shared.is_playing()
    }

    /// Device sample rate in Hz.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.shared.sample_rate()
    }

    /// Monotonic transport generation counter incremented on load/unload/seek.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.shared.generation()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn interpolates_between_audio_callbacks() {
        let shared = Arc::new(SampleClockShared::new(48_000));
        shared.store_state(48_000, true);
        let mut clock = SampleClock::new(shared.clone());

        let t0 = clock.anchor;
        let ms0 = clock.position_ms_at(t0);
        assert!((ms0 - 1000.0).abs() < 1e-6);

        let ms_mid = clock.position_ms_at(t0 + Duration::from_millis(10));
        assert!((ms_mid - 1010.0).abs() < 1e-6);

        // Paused clock does not extrapolate forward.
        shared.store_state(48_000, false);
        let ms_paused = clock.position_ms_at(t0 + Duration::from_millis(20));
        assert!((ms_paused - 1000.0).abs() < 1e-6);
    }
}
