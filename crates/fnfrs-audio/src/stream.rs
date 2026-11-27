//! Lock-free background streaming song decoder (`rtrb` SPSC) and `VizSampleHistory`
//! ring buffer so full multi-stem songs stream with ~4 MB RAM instead of decoding
//! ~200 MB of uncompressed PCM into memory upfront.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle, Thread};
use std::time::Duration;

use rtrb::{Consumer, Producer, RingBuffer};
use symphonia::core::codecs::audio::AudioDecoder;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatReader;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;

use crate::decoder::DecodeError;

/// Capacity of the streamed multi-stem frame ring buffer (~2.7s at 48 kHz).
pub const STREAM_RING_FRAMES: usize = 131_072;

/// Number of frames pre-decoded synchronously at load so frame 1 never underruns.
const PREFILL_FRAMES: usize = 16_384;

/// Number of frames pushed per worker decode batch.
const WORKER_BATCH_FRAMES: usize = 2048;

/// Power-of-two capacity of [`VizSampleHistory`] (~85ms at 48 kHz = 16 KB).
pub const VIZ_HISTORY_CAP: usize = 4096;

/// Lock-free mono instrumental sample history shared between [`MixerState`]
/// (audio thread writer) and A-Bot's FFT spectrum analyzer (main thread reader).
pub struct VizSampleHistory {
    samples: [AtomicU32; VIZ_HISTORY_CAP],
    frames_written: AtomicU64,
}

impl std::fmt::Debug for VizSampleHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VizSampleHistory")
            .field("frames_written", &self.frames_written.load(Ordering::Relaxed))
            .finish()
    }
}

impl Default for VizSampleHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl VizSampleHistory {
    /// Allocates a zero-initialized lock-free sample history buffer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            samples: std::array::from_fn(|_| AtomicU32::new(0)),
            frames_written: AtomicU64::new(0),
        }
    }

    /// Clears published write cursor (audio thread on transport reset).
    #[inline]
    pub fn reset(&self) {
        self.frames_written.store(0, Ordering::Release);
    }

    /// Appends one mono instrumental sample from the real-time audio thread.
    #[inline]
    pub fn push_mono(&self, mono: f32) {
        let w = self.frames_written.load(Ordering::Relaxed);
        let idx = (w as usize) & (VIZ_HISTORY_CAP - 1);
        self.samples[idx].store(mono.to_bits(), Ordering::Relaxed);
        self.frames_written.store(w + 1, Ordering::Release);
    }

    /// Copies the most recent `N` mono samples chronologically into `dst` (main thread).
    /// Returns `false` if no samples have been written yet.
    #[inline]
    pub fn copy_recent<const N: usize>(&self, dst: &mut [f32; N]) -> bool {
        let w = self.frames_written.load(Ordering::Acquire);
        if w == 0 {
            return false;
        }
        let start = w.saturating_sub(N as u64);
        let avail = (w - start) as usize;
        let pad = N - avail;
        for slot in &mut dst[..pad] {
            *slot = 0.0;
        }
        for i in 0..avail {
            let pos = start + (i as u64);
            let idx = (pos as usize) & (VIZ_HISTORY_CAP - 1);
            dst[pad + i] = f32::from_bits(self.samples[idx].load(Ordering::Relaxed));
        }
        true
    }
}

/// File path and offset specification for one streamed song stem.
#[derive(Debug, Clone)]
pub struct StreamStemSpec {
    pub path: PathBuf,
    pub offset_ms: f64,
}

/// Stem configuration passed to [`StreamedSong::spawn`].
#[derive(Debug, Clone)]
pub struct StreamedSongConfig {
    pub inst: Option<StreamStemSpec>,
    pub player_vocals: Option<StreamStemSpec>,
    pub opponent_vocals: Option<StreamStemSpec>,
    pub target_rate: u32,
}

/// Synchronized triple-stem PCM sample + stream epoch pushed over lock-free `rtrb`.
#[derive(Debug, Clone, Copy)]
pub struct StreamSample {
    pub inst: [f32; 2],
    pub pv: [f32; 2],
    pub ov: [f32; 2],
    pub epoch: u32,
}

/// Shared lock-free coordination state between main thread, audio thread, and worker.
#[derive(Debug)]
pub struct StreamControl {
    pub epoch: AtomicU32,
    pub seek_pending: AtomicBool,
    pub seek_target_frame: AtomicU64,
    pub stop_requested: AtomicBool,
    pub eof_epoch: AtomicU32,
    pub eof_reached: AtomicBool,
}

impl StreamControl {
    fn new() -> Self {
        Self {
            epoch: AtomicU32::new(1),
            seek_pending: AtomicBool::new(false),
            seek_target_frame: AtomicU64::new(0),
            stop_requested: AtomicBool::new(false),
            eof_epoch: AtomicU32::new(0),
            eof_reached: AtomicBool::new(false),
        }
    }
}

/// Audio-thread handle for reading streamed [`StreamSample`] frames cleanly.
pub struct StreamAudioSink {
    pub rx: Consumer<StreamSample>,
    pub ctl: Arc<StreamControl>,
    pub duration_frames: Option<u64>,
}

/// Main-thread owner of the background streaming decoder thread.
pub struct StreamedSongOwner {
    ctl: Arc<StreamControl>,
    worker_thread: Thread,
    join_handle: Option<JoinHandle<()>>,
    pub duration_frames: Option<u64>,
}

impl StreamedSongOwner {
    /// Signals the background worker to seek cleanly to `start_frame` under a new `epoch`.
    pub fn seek_frame(&self, start_frame: u64) -> u32 {
        let next_epoch = self.ctl.epoch.fetch_add(1, Ordering::AcqRel).wrapping_add(1).max(1);
        self.ctl.seek_target_frame.store(start_frame, Ordering::Release);
        self.ctl.seek_pending.store(true, Ordering::Release);
        self.worker_thread.unpark();
        next_epoch
    }
}

impl Drop for StreamedSongOwner {
    fn drop(&mut self) {
        self.ctl.stop_requested.store(true, Ordering::Release);
        self.worker_thread.unpark();
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Spawns a background streaming decoder for synchronized multi-stem song playback,
/// pre-filling [`PREFILL_FRAMES`] synchronously so playback begins with zero latency.
pub fn spawn_song_stream(
    cfg: StreamedSongConfig,
    start_frame: u64,
) -> Result<(StreamAudioSink, StreamedSongOwner, u32), DecodeError> {
    let target_rate = cfg.target_rate.max(1);
    let mut inst = match &cfg.inst {
        Some(spec) => Some(StemStreamReader::open(&spec.path, spec.offset_ms, target_rate)?),
        None => None,
    };
    let mut pv = match &cfg.player_vocals {
        Some(spec) => Some(StemStreamReader::open(&spec.path, spec.offset_ms, target_rate)?),
        None => None,
    };
    let mut ov = match &cfg.opponent_vocals {
        Some(spec) => Some(StemStreamReader::open(&spec.path, spec.offset_ms, target_rate)?),
        None => None,
    };

    let duration_frames = [
        inst.as_ref().and_then(|s| s.total_song_frames()),
        pv.as_ref().and_then(|s| s.total_song_frames()),
        ov.as_ref().and_then(|s| s.total_song_frames()),
    ]
    .into_iter()
    .flatten()
    .max();

    let (mut tx, rx) = RingBuffer::<StreamSample>::new(STREAM_RING_FRAMES);
    let ctl = Arc::new(StreamControl::new());
    let initial_epoch = ctl.epoch.load(Ordering::Acquire);

    if start_frame > 0 {
        seek_all_stems(
            &mut inst,
            &mut pv,
            &mut ov,
            cfg.inst.as_ref(),
            cfg.player_vocals.as_ref(),
            cfg.opponent_vocals.as_ref(),
            target_rate,
            start_frame,
        );
    }

    // Pre-fill `PREFILL_FRAMES` synchronously so audio never starves on frame 1.
    let eof_now = push_stream_frames(
        &mut tx,
        &ctl,
        initial_epoch,
        PREFILL_FRAMES,
        &mut inst,
        &mut pv,
        &mut ov,
    );
    if eof_now {
        ctl.eof_epoch.store(initial_epoch, Ordering::Release);
        ctl.eof_reached.store(true, Ordering::Release);
    }

    let worker_ctl = Arc::clone(&ctl);
    let join_handle = thread::Builder::new()
        .name("fnfrs-audio-stream".to_owned())
        .spawn(move || {
            let mut active_epoch = initial_epoch;
            let mut eof_sent_for_epoch = eof_now;

            while !worker_ctl.stop_requested.load(Ordering::Acquire) {
                if worker_ctl.seek_pending.swap(false, Ordering::AcqRel) {
                    active_epoch = worker_ctl.epoch.load(Ordering::Acquire);
                    let target = worker_ctl.seek_target_frame.load(Ordering::Acquire);
                    worker_ctl.eof_reached.store(false, Ordering::Release);
                    seek_all_stems(
                        &mut inst,
                        &mut pv,
                        &mut ov,
                        cfg.inst.as_ref(),
                        cfg.player_vocals.as_ref(),
                        cfg.opponent_vocals.as_ref(),
                        target_rate,
                        target,
                    );
                    eof_sent_for_epoch = false;
                }

                let avail = tx.slots();
                if avail >= WORKER_BATCH_FRAMES && !eof_sent_for_epoch {
                    let count = avail.min(WORKER_BATCH_FRAMES * 2);
                    let hit_eof = push_stream_frames(
                        &mut tx,
                        &worker_ctl,
                        active_epoch,
                        count,
                        &mut inst,
                        &mut pv,
                        &mut ov,
                    );
                    if hit_eof {
                        worker_ctl.eof_epoch.store(active_epoch, Ordering::Release);
                        worker_ctl.eof_reached.store(true, Ordering::Release);
                        eof_sent_for_epoch = true;
                    }
                } else {
                    thread::park_timeout(Duration::from_millis(8));
                }
            }
        })?;

    let worker_thread = join_handle.thread().clone();
    let sink = StreamAudioSink {
        rx,
        ctl: Arc::clone(&ctl),
        duration_frames,
    };
    let owner = StreamedSongOwner {
        ctl,
        worker_thread,
        join_handle: Some(join_handle),
        duration_frames,
    };

    Ok((sink, owner, initial_epoch))
}

#[allow(clippy::too_many_arguments)]
fn seek_all_stems(
    inst: &mut Option<StemStreamReader>,
    pv: &mut Option<StemStreamReader>,
    ov: &mut Option<StemStreamReader>,
    inst_spec: Option<&StreamStemSpec>,
    pv_spec: Option<&StreamStemSpec>,
    ov_spec: Option<&StreamStemSpec>,
    target_rate: u32,
    song_frame: u64,
) {
    for (slot, spec_opt) in [(inst, inst_spec), (pv, pv_spec), (ov, ov_spec)] {
        if let Some(spec) = spec_opt {
            if let Ok(mut reopened) =
                StemStreamReader::open(&spec.path, spec.offset_ms, target_rate)
            {
                reopened.skip_to_song_frame(song_frame);
                *slot = Some(reopened);
            }
        }
    }
}

fn push_stream_frames(
    tx: &mut Producer<StreamSample>,
    ctl: &StreamControl,
    epoch: u32,
    count: usize,
    inst: &mut Option<StemStreamReader>,
    pv: &mut Option<StemStreamReader>,
    ov: &mut Option<StemStreamReader>,
) -> bool {
    let writable = tx.slots().min(count);
    for _ in 0..writable {
        if ctl.stop_requested.load(Ordering::Acquire) || ctl.seek_pending.load(Ordering::Acquire) {
            return false;
        }
        let s_inst = inst.as_mut().map_or([0.0, 0.0], StemStreamReader::next_frame);
        let s_pv = pv.as_mut().map_or([0.0, 0.0], StemStreamReader::next_frame);
        let s_ov = ov.as_mut().map_or([0.0, 0.0], StemStreamReader::next_frame);

        let all_done = inst.as_ref().is_none_or(|s| s.eof)
            && pv.as_ref().is_none_or(|s| s.eof)
            && ov.as_ref().is_none_or(|s| s.eof);
        if all_done {
            return true;
        }
        if tx
            .push(StreamSample {
                inst: s_inst,
                pv: s_pv,
                ov: s_ov,
                epoch,
            })
            .is_err()
        {
            return false;
        }
    }
    false
}

struct StemStreamReader {
    format: Box<dyn FormatReader>,
    track_id: u32,
    decoder: Box<dyn AudioDecoder>,
    src_rate: u32,
    target_rate: u32,
    header_src_frames: Option<u64>,
    offset_frames: i64,
    song_cursor: i64,
    pcm_buf: Vec<f32>,
    pcm_cursor: usize,
    pkt_scratch: Vec<f32>,
    resample_phase: f64,
    eof_decoded: bool,
    eof: bool,
}

impl StemStreamReader {
    fn open(path: &Path, offset_ms: f64, target_rate: u32) -> Result<Self, DecodeError> {
        let file = File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe().probe(
            &hint,
            mss,
            Default::default(),
            Default::default(),
        )?;
        let (track_id, audio_params) = format
            .tracks()
            .iter()
            .find_map(|track| {
                let params = track.codec_params.as_ref()?.audio()?;
                Some((track.id, params.clone()))
            })
            .ok_or(DecodeError::NoAudioTrack)?;

        let src_rate = audio_params.sample_rate.ok_or(DecodeError::MissingSampleRate)?;
        let header_src_frames = audio_params.n_frames;
        let decoder =
            symphonia::default::get_codecs().make_audio_decoder(&audio_params, &Default::default())?;

        let offset_frames =
            ((offset_ms / 1000.0) * f64::from(target_rate.max(1))).round() as i64;

        let mut reader = Self {
            format,
            track_id,
            decoder,
            src_rate: src_rate.max(1),
            target_rate: target_rate.max(1),
            header_src_frames,
            offset_frames,
            song_cursor: 0,
            pcm_buf: Vec::with_capacity(16_384),
            pcm_cursor: 0,
            pkt_scratch: Vec::with_capacity(8192),
            resample_phase: 0.0,
            eof_decoded: false,
            eof: false,
        };
        if offset_frames < 0 {
            reader.advance_stem_frames((-offset_frames) as u64);
        }
        Ok(reader)
    }

    fn total_song_frames(&self) -> Option<u64> {
        let src_frames = self.header_src_frames?;
        let target_frames =
            ((src_frames as f64) * f64::from(self.target_rate) / f64::from(self.src_rate)).ceil()
                as i64;
        Some((target_frames + self.offset_frames).max(0) as u64)
    }

    fn skip_to_song_frame(&mut self, target_song_frame: u64) {
        self.song_cursor = target_song_frame as i64;
        let target_stem = self.song_cursor - self.offset_frames;
        if target_stem > 0 {
            self.advance_stem_frames(target_stem as u64);
        }
    }

    fn advance_stem_frames(&mut self, frames_to_skip: u64) {
        for _ in 0..frames_to_skip {
            let _ = self.next_stem_frame();
            if self.eof {
                break;
            }
        }
    }

    fn decode_more(&mut self) -> bool {
        if self.eof_decoded {
            return false;
        }
        if self.pcm_cursor >= 4096 {
            let drain_samples = self.pcm_cursor * 2;
            self.pcm_buf.drain(..drain_samples);
            self.pcm_cursor = 0;
        }

        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    self.eof_decoded = true;
                    return false;
                }
                Err(SymphoniaError::IoError(ref io_err))
                    if io_err.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    self.eof_decoded = true;
                    return false;
                }
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(_) => {
                    self.eof_decoded = true;
                    return false;
                }
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(buf) => buf,
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(_) => {
                    self.eof_decoded = true;
                    return false;
                }
            };
            let spec = decoded.spec();
            let channels = spec.channels().count();
            let frames = decoded.frames();
            if channels == 0 || frames == 0 {
                continue;
            }
            let needed = decoded.samples_interleaved();
            self.pkt_scratch.resize(needed, 0.0);
            decoded.copy_to_slice_interleaved::<f32, _>(self.pkt_scratch.as_mut_slice());
            self.pcm_buf.reserve(frames * 2);
            match channels {
                1 => {
                    for &s in self.pkt_scratch.iter().take(frames) {
                        self.pcm_buf.push(s);
                        self.pcm_buf.push(s);
                    }
                }
                2 => {
                    self.pcm_buf.extend_from_slice(&self.pkt_scratch[..frames * 2]);
                }
                n => {
                    for frame in self.pkt_scratch.chunks_exact(n).take(frames) {
                        self.pcm_buf.push(frame[0]);
                        self.pcm_buf.push(frame[1]);
                    }
                }
            }
            return true;
        }
    }

    #[inline]
    fn ensure_src_frames(&mut self, needed_from_cursor: usize) {
        while (self.pcm_buf.len() / 2).saturating_sub(self.pcm_cursor) < needed_from_cursor {
            if !self.decode_more() {
                break;
            }
        }
    }

    #[inline]
    fn next_frame(&mut self) -> [f32; 2] {
        let cur = self.song_cursor;
        self.song_cursor += 1;
        if cur < self.offset_frames {
            return [0.0, 0.0];
        }
        self.next_stem_frame()
    }

    fn next_stem_frame(&mut self) -> [f32; 2] {
        if self.eof {
            return [0.0, 0.0];
        }
        if self.src_rate == self.target_rate {
            self.ensure_src_frames(1);
            let avail = (self.pcm_buf.len() / 2).saturating_sub(self.pcm_cursor);
            if avail == 0 {
                self.eof = true;
                return [0.0, 0.0];
            }
            let base = self.pcm_cursor * 2;
            self.pcm_cursor += 1;
            return [self.pcm_buf[base], self.pcm_buf[base + 1]];
        }

        // Phase-continuous 4-tap Catmull-Rom resampler when `src_rate != target_rate`.
        self.ensure_src_frames(4);
        let avail = (self.pcm_buf.len() / 2).saturating_sub(self.pcm_cursor);
        if avail == 0 {
            self.eof = true;
            return [0.0, 0.0];
        }
        let idx = self.resample_phase.floor() as usize;
        let t = (self.resample_phase - (idx as f64)) as f32;
        let step = f64::from(self.src_rate) / f64::from(self.target_rate);

        let sample_at = |rel: isize| -> [f32; 2] {
            let max_rel = (avail as isize).saturating_sub(1);
            let clamped = (idx as isize + rel).clamp(0, max_rel) as usize;
            let base = (self.pcm_cursor + clamped) * 2;
            [self.pcm_buf[base], self.pcm_buf[base + 1]]
        };
        let p0 = sample_at(-1);
        let p1 = sample_at(0);
        let p2 = sample_at(1);
        let p3 = sample_at(2);

        let interp = |c0: f32, c1: f32, c2: f32, c3: f32| -> f32 {
            let a = -0.5 * c0 + 1.5 * c1 - 1.5 * c2 + 0.5 * c3;
            let b = c0 - 2.5 * c1 + 2.0 * c2 - 0.5 * c3;
            let c = -0.5 * c0 + 0.5 * c2;
            ((a * t + b) * t + c) * t + c1
        };
        let out = [interp(p0[0], p1[0], p2[0], p3[0]), interp(p0[1], p1[1], p2[1], p3[1])];

        self.resample_phase += step;
        let consume = self.resample_phase.floor() as usize;
        if consume > 0 {
            self.pcm_cursor += consume.min(avail);
            self.resample_phase -= consume as f64;
        }
        if idx >= avail && self.eof_decoded {
            self.eof = true;
        }
        out
    }
}
