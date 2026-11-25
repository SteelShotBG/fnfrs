use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};
use thiserror::Error;

use crate::clock::{SampleClock, SampleClockShared};
use crate::decoder::DecodedAudio;

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
        let offset_frames =
            ((offset_ms / 1000.0) * f64::from(audio.sample_rate)).round() as i64;
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
        let ov_frames = opponent_vocals.as_ref().map_or(0, |t| t.audio.frames as u64);
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
}

/// Replaced buffers returned to the main thread so the audio callback never deallocates.
pub enum RetiredResource {
    Song(Box<SongTracks>),
    Sfx(Box<SfxBank>),
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

    song: Option<Box<SongTracks>>,
    song_cursor_frames: u64,
    is_playing: bool,

    inst_gain: f32,
    player_vocals_gain: f32,
    opponent_vocals_gain: f32,
    master_gain: f32,

    sfx_bank: Option<Box<SfxBank>>,
    sfx_voices: [SfxVoice; MAX_SFX_VOICES],
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
            song: None,
            song_cursor_frames: 0,
            is_playing: false,
            inst_gain: 1.0,
            player_vocals_gain: 1.0,
            opponent_vocals_gain: 1.0,
            master_gain: 1.0,
            sfx_bank: None,
            sfx_voices: [SfxVoice::default(); MAX_SFX_VOICES],
        }
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
                    self.song_cursor_frames = start_frame;
                    self.is_playing = false;
                    transport_reset = true;
                }
                AudioCommand::UnloadSong => {
                    if let Some(old) = self.song.take() {
                        let _ = self.retired_tx.push(RetiredResource::Song(old));
                    }
                    self.song_cursor_frames = 0;
                    self.is_playing = false;
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
            }
        }

        for frame_out in output.chunks_exact_mut(channels) {
            let mut left = 0.0_f32;
            let mut right = 0.0_f32;

            if self.is_playing {
                if let Some(song) = &self.song {
                    if self.song_cursor_frames < song.duration_frames {
                        if let Some(inst) = &song.inst {
                            let [l, r] = inst.sample_at_song_frame(self.song_cursor_frames);
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
            self.clock.bump_generation(self.song_cursor_frames, self.is_playing);
        } else {
            self.clock.store_state(self.song_cursor_frames, self.is_playing);
        }
    }

    fn trigger_sfx(&mut self, slot: usize, gain: f32) {
        let target = self
            .sfx_voices
            .iter_mut()
            .find(|v| !v.active)
            .or_else(|| self.sfx_voices.first_mut());
        if let Some(voice) = target {
            *voice = SfxVoice {
                active: true,
                slot,
                cursor: 0,
                gain,
            };
        }
    }
}

/// Main-thread handle to the lock-free `cpal` audio engine.
pub struct AudioEngine {
    _stream: Stream,
    cmd_tx: Producer<AudioCommand>,
    retired_rx: Consumer<RetiredResource>,
    clock: SampleClock,
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
        let mut mixer = MixerState::new(cmd_rx, retired_tx, clock_shared.clone());

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
            sample_rate,
        })
    }

    /// Output device sample rate in Hz.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
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

    /// Loads synchronized song stems starting at `start_ms`.
    pub fn load_song(&mut self, tracks: SongTracks, start_ms: f64) -> Result<(), AudioError> {
        let start_frame =
            ((start_ms.max(0.0) / 1000.0) * f64::from(self.sample_rate)).round() as u64;
        self.send(AudioCommand::LoadSong {
            tracks: Box::new(tracks),
            start_frame,
        })
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
        self.send(AudioCommand::SeekFrame(frame))
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
        let vocals = DecodedAudio::from_stereo_pcm(
            vec![0.5, -0.5, 0.5, -0.5, 0.5, -0.5, 0.5, -0.5],
            48_000,
        );

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
        cmd_tx.push(AudioCommand::RegisterSfxBank(Box::new(vec![sfx]))).unwrap();
        cmd_tx
            .push(AudioCommand::SetTrackGain {
                track: TrackKind::PlayerVocals,
                gain: 0.0,
            })
            .unwrap();
        cmd_tx.push(AudioCommand::PlaySfx { slot: 0, gain: 1.0 }).unwrap();

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
