use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};
use thiserror::Error;

use crate::clock::{SampleClock, SampleClockShared};
use crate::decoder::{DecodeError, DecodedAudio};
use crate::stream::{
    spawn_song_stream, StreamAudioSink, StreamedSongConfig, StreamedSongOwner, VizSampleHistory,
};

/// Maximum concurrent one-shot sound effects mixed on the audio thread.
pub const MAX_SFX_VOICES: usize = 16;

/// Errors returned when initializing or controlling [`AudioEngine`].
#[derive(Debug, Error)]
pub enum AudioError {
    #[error("no default audio output device available")]
    NoOutputDevice,
    #[error("failed to query default output config: {0}")]
    DefaultConfig(String),
    #[error("failed to build audio output stream: {0}")]
    BuildStream(String),
    #[error("failed to start audio output stream: {0}")]
    PlayStream(String),
    #[error("audio command queue full")]
    QueueFull,
    #[error("stream decode failed: {0}")]
    Decode(#[from] DecodeError),
}

/// Identifies one of the three synchronized song stems.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Inst,
    PlayerVocals,
    OpponentVocals,
}

/// A song stem with optional sample-frame offset relative to the instrumental.
#[derive(Debug, Clone)]
pub struct TrackStem {
    pub audio: DecodedAudio,
    pub offset_frames: i64,
}

impl TrackStem {
    /// Creates a [`TrackStem`] from a decoded buffer and millisecond offset.
    #[must_use]
    pub fn new(audio: DecodedAudio, offset_ms: f64) -> Self {
        let offset_frames = ((offset_ms / 1000.0) * f64::from(audio.sample_rate)).round() as i64;
        Self {
            audio,
            offset_frames,
        }
    }

    #[inline]
    fn sample_at_song_frame(&self, song_frame: u64) -> [f32; 2] {
        let idx = song_frame as i64 - self.offset_frames;
        if idx < 0 {
            return [0.0, 0.0];
        }
        self.audio.sample_frame(idx as usize)
    }
}

/// Complete set of synchronized song stems loaded before starting playback.
#[derive(Debug, Clone)]
pub struct SongTracks {
    pub inst: Option<TrackStem>,
    pub player_vocals: Option<TrackStem>,
    pub opponent_vocals: Option<TrackStem>,
    pub duration_frames: u64,
}

impl SongTracks {
    /// Computes `duration_frames` from the instrumental (or longest stem).
    #[must_use]
    pub fn new(
        inst: Option<TrackStem>,
        player_vocals: Option<TrackStem>,
        opponent_vocals: Option<TrackStem>,
    ) -> Self {
        let inst_frames = inst.as_ref().map_or(0, |t| t.audio.frames as u64);
        let pv_frames = player_vocals.as_ref().map_or(0, |t| t.audio.frames as u64);
        let ov_frames = opponent_vocals
            .as_ref()
            .map_or(0, |t| t.audio.frames as u64);
        let duration_frames = inst_frames.max(pv_frames).max(ov_frames);
        Self {
            inst,
            player_vocals,
            opponent_vocals,
            duration_frames,
        }
    }
}

/// Pre-decoded sound-effect bank swapped cleanly via the command queue.
pub type SfxBank = Vec<DecodedAudio>;

/// Lock-free commands sent from the main thread to the audio thread.
pub enum AudioCommand {
    LoadSong {
        tracks: Box<SongTracks>,
        start_frame: u64,
    },
    LoadStream {
        sink: Box<StreamAudioSink>,
        start_frame: u64,
        epoch: u32,
    },
    SeekStream {
        start_frame: u64,
        epoch: u32,
    },
    UnloadSong,
    Play,
    Pause,
    SeekFrame(u64),
    SetTrackGain {
        track: TrackKind,
        gain: f32,
    },
    SetMasterGain(f32),
    RegisterSfxBank(Box<SfxBank>),
    PlaySfx {
        slot: usize,
        gain: f32,
    },
    StopAllSfx,
    StartBgLoop(Box<DecodedAudio>),
    StopBgLoop,
    SetBgLoopPaused(bool),
    SetBgLoopGain(f32),
    StartPauseLoop(Box<DecodedAudio>),
    StopPauseLoop,
    SetPauseLoopGain(f32),
    AttachVideoAudio(Box<VideoAudioConsumer>),
    DetachVideoAudio,
    SetVideoAudioPaused(bool),
}

/// Replaced buffers returned to the main thread so the audio callback never deallocates.
pub enum RetiredResource {
    Song(Box<SongTracks>),
    Stream(Box<StreamAudioSink>),
    Sfx(Box<SfxBank>),
    Audio(Box<DecodedAudio>),
    VideoAudio(Box<VideoAudioConsumer>),
}

/// Lock-free consumer side of streamed video cutscene stereo PCM frames (`MixerState` thread).
pub struct VideoAudioConsumer {
    rx: Consumer<[f32; 2]>,
}

/// Producer side fed from VLC's `amem` decode callback on the video audio thread.
pub struct VideoAudioProducer {
    tx: Producer<[f32; 2]>,
}

impl VideoAudioProducer {
    /// Converts interleaved stereo signed 16-bit (`S16N`) PCM samples into `[f32; 2]` frames
    /// and pushes as many as fit into the lock-free ring buffer.
    #[inline]
    pub fn push_s16n_stereo(&mut self, samples: &[i16]) {
        const INV_I16: f32 = 1.0 / 32768.0;
        for pair in samples.as_chunks::<2>().0 {
            let l = f32::from(pair[0]) * INV_I16;
            let r = f32::from(pair[1]) * INV_I16;
            if self.tx.push([l, r]).is_err() {
                break;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct SfxVoice {
    active: bool,
    slot: usize,
    cursor: usize,
    gain: f32,
}

/// Real-time mixer state owned exclusively by the `cpal` output callback.
pub struct MixerState {
    cmd_rx: Consumer<AudioCommand>,
    retired_tx: Producer<RetiredResource>,
    clock: Arc<SampleClockShared>,
    viz_history: Arc<VizSampleHistory>,

    song: Option<Box<SongTracks>>,
    stream: Option<Box<StreamAudioSink>>,
    stream_epoch: u32,
    song_cursor_frames: u64,
    is_playing: bool,

    inst_gain: f32,
    player_vocals_gain: f32,
    opponent_vocals_gain: f32,
    master_gain: f32,

    sfx_bank: Option<Box<SfxBank>>,
    sfx_voices: [SfxVoice; MAX_SFX_VOICES],
    bg_loop: Option<Box<DecodedAudio>>,
    bg_loop_cursor: usize,
    bg_loop_paused: bool,
    bg_loop_gain: f32,
    pause_loop: Option<Box<DecodedAudio>>,
    pause_loop_cursor: usize,
    pause_loop_gain: f32,
    video_audio: Option<Box<VideoAudioConsumer>>,
    video_audio_paused: bool,
}

impl MixerState {
    /// Creates a [`MixerState`] driven by `cmd_rx` and publishing to `clock`.
    #[must_use]
    pub fn new(
        cmd_rx: Consumer<AudioCommand>,
        retired_tx: Producer<RetiredResource>,
        clock: Arc<SampleClockShared>,
    ) -> Self {
        Self {
            cmd_rx,
            retired_tx,
            clock,
            viz_history: Arc::new(VizSampleHistory::new()),
            song: None,
            stream: None,
            stream_epoch: 1,
            song_cursor_frames: 0,
            is_playing: false,
            inst_gain: 1.0,
            player_vocals_gain: 1.0,
            opponent_vocals_gain: 1.0,
            master_gain: 1.0,
            sfx_bank: None,
            sfx_voices: [SfxVoice::default(); MAX_SFX_VOICES],
            bg_loop: None,
            bg_loop_cursor: 0,
            bg_loop_paused: false,
            bg_loop_gain: 0.75,
            pause_loop: None,
            pause_loop_cursor: 0,
            pause_loop_gain: 0.0,
            video_audio: None,
            video_audio_paused: false,
        }
    }

    /// Attaches a shared [`VizSampleHistory`] buffer populated cleanly on the audio thread.
    #[must_use]
    pub fn with_viz_history(mut self, viz_history: Arc<VizSampleHistory>) -> Self {
        self.viz_history = viz_history;
        self
    }

    /// Processes queued commands and fills `output` with mixed PCM frames.
    /// Zero allocations and zero mutexes.
    pub fn render(&mut self, output: &mut [f32], channels: usize) {
        if channels == 0 {
            return;
        }

        let mut transport_reset = false;
        while let Ok(cmd) = self.cmd_rx.pop() {
            match cmd {
                AudioCommand::LoadSong {
                    tracks,
                    start_frame,
                } => {
                    if let Some(old) = self.song.replace(tracks) {
                        let _ = self.retired_tx.push(RetiredResource::Song(old));
                    }
                    if let Some(old_stream) = self.stream.take() {
                        let _ = self.retired_tx.push(RetiredResource::Stream(old_stream));
                    }
                    self.song_cursor_frames = start_frame;
                    self.is_playing = false;
                    self.viz_history.reset();
                    transport_reset = true;
                }
                AudioCommand::LoadStream {
                    sink,
                    start_frame,
                    epoch,
                } => {
                    if let Some(old_stream) = self.stream.replace(sink) {
                        let _ = self.retired_tx.push(RetiredResource::Stream(old_stream));
                    }
                    if let Some(old) = self.song.take() {
                        let _ = self.retired_tx.push(RetiredResource::Song(old));
                    }
                    self.stream_epoch = epoch;
                    self.song_cursor_frames = start_frame;
                    self.is_playing = false;
                    self.viz_history.reset();
                    transport_reset = true;
                }
                AudioCommand::SeekStream { start_frame, epoch } => {
                    self.stream_epoch = epoch;
                    self.song_cursor_frames = start_frame;
                    self.viz_history.reset();
                    transport_reset = true;
                }
                AudioCommand::UnloadSong => {
                    if let Some(old) = self.song.take() {
                        let _ = self.retired_tx.push(RetiredResource::Song(old));
                    }
                    if let Some(old_stream) = self.stream.take() {
                        let _ = self.retired_tx.push(RetiredResource::Stream(old_stream));
                    }
                    self.song_cursor_frames = 0;
                    self.is_playing = false;
                    self.viz_history.reset();
                    transport_reset = true;
                }
                AudioCommand::Play => {
                    self.is_playing = true;
                }
                AudioCommand::Pause => {
                    self.is_playing = false;
                }
                AudioCommand::SeekFrame(frame) => {
                    self.song_cursor_frames = frame;
                    self.viz_history.reset();
                    transport_reset = true;
                }
                AudioCommand::SetTrackGain { track, gain } => match track {
                    TrackKind::Inst => self.inst_gain = gain,
                    TrackKind::PlayerVocals => self.player_vocals_gain = gain,
                    TrackKind::OpponentVocals => self.opponent_vocals_gain = gain,
                },
                AudioCommand::SetMasterGain(gain) => {
                    self.master_gain = gain;
                }
                AudioCommand::RegisterSfxBank(bank) => {
                    for voice in &mut self.sfx_voices {
                        voice.active = false;
                    }
                    if let Some(old) = self.sfx_bank.replace(bank) {
                        let _ = self.retired_tx.push(RetiredResource::Sfx(old));
                    }
                }
                AudioCommand::PlaySfx { slot, gain } => {
                    self.trigger_sfx(slot, gain);
                }
                AudioCommand::StopAllSfx => {
                    for voice in &mut self.sfx_voices {
                        voice.active = false;
                    }
                }
                AudioCommand::StartBgLoop(audio) => {
                    self.bg_loop_cursor = 0;
                    self.bg_loop_paused = false;
                    self.bg_loop_gain = 0.75;
                    if let Some(old) = self.bg_loop.replace(audio) {
                        let _ = self.retired_tx.push(RetiredResource::Audio(old));
                    }
                }
                AudioCommand::StopBgLoop => {
                    self.bg_loop_cursor = 0;
                    self.bg_loop_paused = false;
                    if let Some(old) = self.bg_loop.take() {
                        let _ = self.retired_tx.push(RetiredResource::Audio(old));
                    }
                }
                AudioCommand::SetBgLoopPaused(paused) => {
                    self.bg_loop_paused = paused;
                }
                AudioCommand::SetBgLoopGain(gain) => {
                    self.bg_loop_gain = gain.clamp(0.0, 1.5);
                }
                AudioCommand::StartPauseLoop(audio) => {
                    self.pause_loop_cursor = if audio.frames > 2 {
                        (audio.frames / 3) % audio.frames
                    } else {
                        0
                    };
                    self.pause_loop_gain = 0.0;
                    if let Some(old) = self.pause_loop.replace(audio) {
                        let _ = self.retired_tx.push(RetiredResource::Audio(old));
                    }
                }
                AudioCommand::StopPauseLoop => {
                    self.pause_loop_cursor = 0;
                    if let Some(old) = self.pause_loop.take() {
                        let _ = self.retired_tx.push(RetiredResource::Audio(old));
                    }
                }
                AudioCommand::SetPauseLoopGain(gain) => {
                    self.pause_loop_gain = gain.clamp(0.0, 1.5);
                }
                AudioCommand::AttachVideoAudio(consumer) => {
                    self.video_audio_paused = false;
                    if let Some(old) = self.video_audio.replace(consumer) {
                        let _ = self.retired_tx.push(RetiredResource::VideoAudio(old));
                    }
                }
                AudioCommand::DetachVideoAudio => {
                    self.video_audio_paused = false;
                    if let Some(old) = self.video_audio.take() {
                        let _ = self.retired_tx.push(RetiredResource::VideoAudio(old));
                    }
                }
                AudioCommand::SetVideoAudioPaused(paused) => {
                    self.video_audio_paused = paused;
                }
            }
        }

        for frame_out in output.chunks_exact_mut(channels) {
            let mut left = 0.0_f32;
            let mut right = 0.0_f32;

            if self.is_playing {
                if let Some(stream) = &mut self.stream {
                    let target_epoch = self.stream_epoch;
                    let mut popped = None;
                    while let Ok(s) = stream.rx.pop() {
                        if s.epoch == target_epoch {
                            popped = Some(s);
                            break;
                        }
                    }
                    if let Some(s) = popped {
                        self.viz_history.push_mono((s.inst[0] + s.inst[1]) * 0.5);
                        left += s.inst[0] * self.inst_gain
                            + s.pv[0] * self.player_vocals_gain
                            + s.ov[0] * self.opponent_vocals_gain;
                        right += s.inst[1] * self.inst_gain
                            + s.pv[1] * self.player_vocals_gain
                            + s.ov[1] * self.opponent_vocals_gain;
                        self.song_cursor_frames += 1;
                    } else if stream
                        .ctl
                        .eof_reached
                        .load(std::sync::atomic::Ordering::Acquire)
                        && stream
                            .ctl
                            .eof_epoch
                            .load(std::sync::atomic::Ordering::Acquire)
                            == target_epoch
                    {
                        self.is_playing = false;
                    }
                } else if let Some(song) = &self.song {
                    if self.song_cursor_frames < song.duration_frames {
                        if let Some(inst) = &song.inst {
                            let [l, r] = inst.sample_at_song_frame(self.song_cursor_frames);
                            self.viz_history.push_mono((l + r) * 0.5);
                            left += l * self.inst_gain;
                            right += r * self.inst_gain;
                        }
                        if let Some(pv) = &song.player_vocals {
                            let [l, r] = pv.sample_at_song_frame(self.song_cursor_frames);
                            left += l * self.player_vocals_gain;
                            right += r * self.player_vocals_gain;
                        }
                        if let Some(ov) = &song.opponent_vocals {
                            let [l, r] = ov.sample_at_song_frame(self.song_cursor_frames);
                            left += l * self.opponent_vocals_gain;
                            right += r * self.opponent_vocals_gain;
                        }
                        self.song_cursor_frames += 1;
                    } else {
                        self.is_playing = false;
                    }
                }
            } else if let Some(pause) = &self.pause_loop {
                if pause.frames > 0 {
                    let [l, r] = pause.sample_frame(self.pause_loop_cursor);
                    left += l * self.pause_loop_gain;
                    right += r * self.pause_loop_gain;
                    self.pause_loop_cursor = (self.pause_loop_cursor + 1) % pause.frames;
                }
            } else if let Some(bg) = &self.bg_loop {
                if !self.bg_loop_paused && bg.frames > 0 {
                    let [l, r] = bg.sample_frame(self.bg_loop_cursor);
                    left += l * self.bg_loop_gain;
                    right += r * self.bg_loop_gain;
                    self.bg_loop_cursor = (self.bg_loop_cursor + 1) % bg.frames;
                }
            }

            if !self.video_audio_paused {
                if let Some(vid) = &mut self.video_audio {
                    if let Ok([l, r]) = vid.rx.pop() {
                        left += l;
                        right += r;
                    }
                }
            }

            if let Some(bank) = &self.sfx_bank {
                for voice in &mut self.sfx_voices {
                    if !voice.active {
                        continue;
                    }
                    if let Some(sfx) = bank.get(voice.slot) {
                        if voice.cursor < sfx.frames {
                            let [l, r] = sfx.sample_frame(voice.cursor);
                            left += l * voice.gain;
                            right += r * voice.gain;
                            voice.cursor += 1;
                        } else {
                            voice.active = false;
                        }
                    } else {
                        voice.active = false;
                    }
                }
            }

            left = (left * self.master_gain).clamp(-1.0, 1.0);
            right = (right * self.master_gain).clamp(-1.0, 1.0);

            if channels == 1 {
                frame_out[0] = (left + right) * 0.5;
            } else {
                frame_out[0] = left;
                frame_out[1] = right;
                for ch in &mut frame_out[2..] {
                    *ch = 0.0;
                }
            }
        }

        if transport_reset {
            self.clock
                .bump_generation(self.song_cursor_frames, self.is_playing);
        } else {
            self.clock
                .store_state(self.song_cursor_frames, self.is_playing);
        }
    }

    fn trigger_sfx(&mut self, slot: usize, gain: f32) {
        let idx = self.sfx_voices.iter().position(|v| !v.active).unwrap_or(0);
        self.sfx_voices[idx] = SfxVoice {
            active: true,
            slot,
            cursor: 0,
            gain,
        };
    }
}

/// Main-thread handle to the lock-free `cpal` audio engine.
pub struct AudioEngine {
    _stream: Stream,
    cmd_tx: Producer<AudioCommand>,
    retired_rx: Consumer<RetiredResource>,
    clock: SampleClock,
    viz_history: Arc<VizSampleHistory>,
    stream_owner: Option<StreamedSongOwner>,
    sample_rate: u32,
}

impl AudioEngine {
    /// Opens the system default `cpal` output stream and starts the audio thread.
    pub fn try_new() -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or(AudioError::NoOutputDevice)?;
        let supported = device
            .default_output_config()
            .map_err(|e| AudioError::DefaultConfig(e.to_string()))?;

        let sample_rate = supported.sample_rate();
        let channels = usize::from(supported.channels().max(1));
        let stream_config: StreamConfig = supported.into();

        let (cmd_tx, cmd_rx) = RingBuffer::<AudioCommand>::new(64);
        let (retired_tx, retired_rx) = RingBuffer::<RetiredResource>::new(16);
        let clock_shared = Arc::new(SampleClockShared::new(sample_rate));
        let viz_history = Arc::new(VizSampleHistory::new());
        let mut mixer = MixerState::new(cmd_rx, retired_tx, clock_shared.clone())
            .with_viz_history(Arc::clone(&viz_history));

        let stream = device
            .build_output_stream::<f32, _, _>(
                stream_config,
                move |data, _info| mixer.render(data, channels),
                |err| eprintln!("audio output error: {err}"),
                None,
            )
            .map_err(|e| AudioError::BuildStream(e.to_string()))?;

        stream
            .play()
            .map_err(|e| AudioError::PlayStream(e.to_string()))?;

        Ok(Self {
            _stream: stream,
            cmd_tx,
            retired_rx,
            clock: SampleClock::new(clock_shared),
            viz_history,
            stream_owner: None,
            sample_rate,
        })
    }

    /// Output device sample rate in Hz.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Lock-free mono instrumental sample history for A-Bot FFT visualization.
    #[must_use]
    pub fn viz_history(&self) -> Arc<VizSampleHistory> {
        Arc::clone(&self.viz_history)
    }

    /// Mutable reference to the main-thread [`SampleClock`].
    pub fn clock_mut(&mut self) -> &mut SampleClock {
        &mut self.clock
    }

    /// Immutable reference to the main-thread [`SampleClock`].
    #[must_use]
    pub fn clock(&self) -> &SampleClock {
        &self.clock
    }

    /// Reclaims and drops old song/SFX buffers retired by the audio thread.
    pub fn reclaim_retired(&mut self) {
        while let Ok(retired) = self.retired_rx.pop() {
            drop(retired);
        }
    }

    /// Sends a command to the audio thread without blocking or allocating.
    pub fn send(&mut self, cmd: AudioCommand) -> Result<(), AudioError> {
        self.reclaim_retired();
        self.cmd_tx.push(cmd).map_err(|_| AudioError::QueueFull)
    }

    /// Loads synchronized pre-decoded song stems starting at `start_ms`.
    pub fn load_song(&mut self, tracks: SongTracks, start_ms: f64) -> Result<(), AudioError> {
        self.stream_owner = None;
        let start_frame =
            ((start_ms.max(0.0) / 1000.0) * f64::from(self.sample_rate)).round() as u64;
        self.send(AudioCommand::LoadSong {
            tracks: Box::new(tracks),
            start_frame,
        })
    }

    /// Streams synchronized multi-stem song tracks from disk via background decoder thread,
    /// keeping song RAM under ~4 MB. Returns optional duration in milliseconds.
    pub fn load_song_streamed(
        &mut self,
        cfg: StreamedSongConfig,
        start_ms: f64,
    ) -> Result<Option<f64>, AudioError> {
        let start_frame =
            ((start_ms.max(0.0) / 1000.0) * f64::from(self.sample_rate)).round() as u64;
        let (sink, owner, epoch) = spawn_song_stream(cfg, start_frame)?;
        let duration_ms = owner
            .duration_frames
            .map(|f| (f as f64 / f64::from(self.sample_rate.max(1))) * 1000.0);
        self.stream_owner = Some(owner);
        self.send(AudioCommand::LoadStream {
            sink: Box::new(sink),
            start_frame,
            epoch,
        })?;
        Ok(duration_ms)
    }

    /// Starts or resumes song transport.
    pub fn play(&mut self) -> Result<(), AudioError> {
        self.send(AudioCommand::Play)
    }

    /// Pauses song transport.
    pub fn pause(&mut self) -> Result<(), AudioError> {
        self.send(AudioCommand::Pause)
    }

    /// Seeks song transport to `ms`.
    pub fn seek_ms(&mut self, ms: f64) -> Result<(), AudioError> {
        let frame = ((ms.max(0.0) / 1000.0) * f64::from(self.sample_rate)).round() as u64;
        if let Some(owner) = &self.stream_owner {
            let epoch = owner.seek_frame(frame);
            self.send(AudioCommand::SeekStream {
                start_frame: frame,
                epoch,
            })
        } else {
            self.send(AudioCommand::SeekFrame(frame))
        }
    }

    /// Adjusts individual stem gain (`Inst`, `PlayerVocals`, `OpponentVocals`).
    pub fn set_track_gain(&mut self, track: TrackKind, gain: f32) -> Result<(), AudioError> {
        self.send(AudioCommand::SetTrackGain { track, gain })
    }

    /// Registers a pre-decoded SFX bank for lock-free playback by slot index.
    pub fn register_sfx_bank(&mut self, bank: SfxBank) -> Result<(), AudioError> {
        self.send(AudioCommand::RegisterSfxBank(Box::new(bank)))
    }

    /// Triggers a pre-registered sound effect slot.
    pub fn play_sfx(&mut self, slot: usize, gain: f32) -> Result<(), AudioError> {
        self.send(AudioCommand::PlaySfx { slot, gain })
    }

    /// Immediately silences any currently active SFX voices.
    pub fn stop_all_sfx(&mut self) -> Result<(), AudioError> {
        self.send(AudioCommand::StopAllSfx)
    }

    /// Starts an independent looping background audio track (e.g. death or dialogue music).
    pub fn start_bg_loop(&mut self, audio: DecodedAudio) -> Result<(), AudioError> {
        self.send(AudioCommand::StartBgLoop(Box::new(audio)))
    }

    /// Stops any active background loop audio track.
    pub fn stop_bg_loop(&mut self) -> Result<(), AudioError> {
        self.send(AudioCommand::StopBgLoop)
    }

    /// Pauses or resumes the active background loop without losing cursor position.
    pub fn set_bg_loop_paused(&mut self, paused: bool) -> Result<(), AudioError> {
        self.send(AudioCommand::SetBgLoopPaused(paused))
    }

    /// Adjusts background loop gain (e.g. ducking during death quotes).
    pub fn set_bg_loop_gain(&mut self, gain: f32) -> Result<(), AudioError> {
        self.send(AudioCommand::SetBgLoopGain(gain))
    }

    /// Starts pause menu background music cleanly over paused gameplay or dialogue.
    pub fn start_pause_loop(&mut self, audio: DecodedAudio) -> Result<(), AudioError> {
        self.send(AudioCommand::StartPauseLoop(Box::new(audio)))
    }

    /// Stops pause menu background music cleanly.
    pub fn stop_pause_loop(&mut self) -> Result<(), AudioError> {
        self.send(AudioCommand::StopPauseLoop)
    }

    /// Sets pause menu music gain (used for the `0.0 -> 0.75` 5-second fade-in).
    pub fn set_pause_loop_gain(&mut self, gain: f32) -> Result<(), AudioError> {
        self.send(AudioCommand::SetPauseLoopGain(gain))
    }

    /// Creates a lock-free stereo ring-buffer stream for video cutscene audio (`libvlc` `amem`).
    pub fn attach_video_audio(&mut self) -> Option<(VideoAudioProducer, u32)> {
        let cap = (self.sample_rate.max(8_000) as usize).saturating_mul(4);
        let (tx, rx) = RingBuffer::<[f32; 2]>::new(cap);
        if self
            .send(AudioCommand::AttachVideoAudio(Box::new(VideoAudioConsumer { rx })))
            .is_ok()
        {
            Some((VideoAudioProducer { tx }, self.sample_rate.max(8_000)))
        } else {
            None
        }
    }

    /// Detaches and retires any active video cutscene audio ring buffer.
    pub fn detach_video_audio(&mut self) -> Result<(), AudioError> {
        self.send(AudioCommand::DetachVideoAudio)
    }

    /// Pauses or resumes video cutscene audio consumption on the mixer thread.
    pub fn set_video_audio_paused(&mut self, paused: bool) -> Result<(), AudioError> {
        self.send(AudioCommand::SetVideoAudioPaused(paused))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_mixer_syncs_stems_sfx_and_retires_old_buffers() {
        let (mut cmd_tx, cmd_rx) = RingBuffer::<AudioCommand>::new(16);
        let (retired_tx, mut retired_rx) = RingBuffer::<RetiredResource>::new(8);
        let shared = Arc::new(SampleClockShared::new(48_000));
        let mut mixer = MixerState::new(cmd_rx, retired_tx, shared.clone());

        let inst = DecodedAudio::from_stereo_pcm(vec![0.25; 8], 48_000);
        let vocals =
            DecodedAudio::from_stereo_pcm(vec![0.5, -0.5, 0.5, -0.5, 0.5, -0.5, 0.5, -0.5], 48_000);

        let song = SongTracks::new(
            Some(TrackStem::new(inst, 0.0)),
            Some(TrackStem::new(vocals, 0.0)),
            None,
        );
        cmd_tx
            .push(AudioCommand::LoadSong {
                tracks: Box::new(song),
                start_frame: 0,
            })
            .unwrap();
        cmd_tx.push(AudioCommand::Play).unwrap();

        let mut out = [0.0_f32; 4]; // 2 stereo frames
        mixer.render(&mut out, 2);

        assert!((out[0] - 0.75).abs() < 1e-6);
        assert!((out[1] - (-0.25)).abs() < 1e-6);
        assert_eq!(shared.played_frames(), 2);
        assert!(shared.is_playing());

        // Mute player vocals and trigger SFX.
        let sfx = DecodedAudio::from_stereo_pcm(vec![0.1, 0.2, 0.1, 0.2], 48_000);
        cmd_tx
            .push(AudioCommand::RegisterSfxBank(Box::new(vec![sfx])))
            .unwrap();
        cmd_tx
            .push(AudioCommand::SetTrackGain {
                track: TrackKind::PlayerVocals,
                gain: 0.0,
            })
            .unwrap();
        cmd_tx
            .push(AudioCommand::PlaySfx { slot: 0, gain: 1.0 })
            .unwrap();

        mixer.render(&mut out, 2);
        assert!((out[0] - 0.35).abs() < 1e-6);
        assert!((out[1] - 0.45).abs() < 1e-6);
        assert_eq!(shared.played_frames(), 4);

        // Replacing song retires the previous buffer to main thread instead of dropping in RT.
        cmd_tx.push(AudioCommand::UnloadSong).unwrap();
        mixer.render(&mut out, 2);
        assert!(matches!(retired_rx.pop(), Ok(RetiredResource::Song(_))));
    }
}
