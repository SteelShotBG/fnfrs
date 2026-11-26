use std::fs::File;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use thiserror::Error;

/// Errors produced when decoding or resampling an audio file.
#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("failed to open audio file: {0}")]
    Io(#[from] std::io::Error),
    #[error("symphonia error: {0}")]
    Symphonia(#[from] SymphoniaError),
    #[error("no supported audio track found")]
    NoAudioTrack,
    #[error("audio track missing sample rate")]
    MissingSampleRate,
    #[error("resampler error: {0}")]
    Resample(String),
}

/// Pre-decoded, stereo interleaved `f32` PCM buffer at the device output sample rate.
#[derive(Debug, Clone)]
pub struct DecodedAudio {
    /// Interleaved `[L, R, L, R, ...]` samples in `[-1.0, 1.0]`.
    pub samples: Arc<[f32]>,
    /// Number of stereo sample frames (`samples.len() / 2`).
    pub frames: usize,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

impl DecodedAudio {
    /// Wraps an already-interleaved stereo `f32` vector.
    #[must_use]
    pub fn from_stereo_pcm(mut samples: Vec<f32>, sample_rate: u32) -> Self {
        if !samples.len().is_multiple_of(2) {
            samples.pop();
        }
        let frames = samples.len() / 2;
        Self {
            samples: Arc::from(samples),
            frames,
            sample_rate,
        }
    }

    /// Reads stereo `[left, right]` sample at `frame_idx`, or `[0.0, 0.0]` if out of bounds.
    #[inline]
    #[must_use]
    pub fn sample_frame(&self, frame_idx: usize) -> [f32; 2] {
        if frame_idx >= self.frames {
            return [0.0, 0.0];
        }
        let base = frame_idx * 2;
        [self.samples[base], self.samples[base + 1]]
    }

    /// Total duration in milliseconds.
    #[must_use]
    pub fn duration_ms(&self) -> f64 {
        (self.frames as f64 / f64::from(self.sample_rate.max(1))) * 1000.0
    }
}

/// Decodes an audio file from disk into stereo `f32` PCM, resampling to `target_rate` if needed.
pub fn decode_file(path: &Path, target_rate: u32) -> Result<DecodedAudio, DecodeError> {
    let file = File::open(path)?;
    let ext = path.extension().and_then(|e| e.to_str());
    decode_media_source(Box::new(file), ext, target_rate)
}

/// Decodes an in-memory audio byte slice into stereo `f32` PCM, resampling to `target_rate`.
pub fn decode_bytes(
    bytes: Vec<u8>,
    ext_hint: Option<&str>,
    target_rate: u32,
) -> Result<DecodedAudio, DecodeError> {
    decode_media_source(Box::new(Cursor::new(bytes)), ext_hint, target_rate)
}

fn decode_media_source(
    source: Box<dyn MediaSource>,
    ext_hint: Option<&str>,
    target_rate: u32,
) -> Result<DecodedAudio, DecodeError> {
    let mss = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = ext_hint {
        hint.with_extension(ext);
    }

    let mut format = symphonia::default::get_probe().probe(
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

    let mut decoder =
        symphonia::default::get_codecs().make_audio_decoder(&audio_params, &Default::default())?;

    let mut src_rate = audio_params.sample_rate;
    let mut stereo_pcm: Vec<f32> = Vec::new();
    let mut pkt_pcm: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::IoError(ref io_err))
                if io_err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                // Some V-Slice `.ogg` assets (e.g. `2hot/Voices-darnell.ogg`) lack a trailing OGG EOS page.
                break;
            }
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(err) => {
                if !stereo_pcm.is_empty() {
                    break;
                }
                return Err(err.into());
            }
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(buf) => buf,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(err) => {
                if !stereo_pcm.is_empty() {
                    break;
                }
                return Err(err.into());
            }
        };

        let spec = decoded.spec();
        if src_rate.is_none() {
            src_rate = Some(spec.rate());
        }
        let channels = spec.channels().count();
        let frames = decoded.frames();
        if channels == 0 || frames == 0 {
            continue;
        }

        let needed = decoded.samples_interleaved();
        pkt_pcm.resize(needed, 0.0);
        decoded.copy_to_slice_interleaved::<f32, _>(pkt_pcm.as_mut_slice());

        append_as_stereo(&pkt_pcm, channels, frames, &mut stereo_pcm);
    }

    let source_rate = src_rate.ok_or(DecodeError::MissingSampleRate)?;
    if source_rate != target_rate && !stereo_pcm.is_empty() {
        stereo_pcm = resample_stereo(&stereo_pcm, source_rate, target_rate)?;
        return Ok(DecodedAudio::from_stereo_pcm(stereo_pcm, target_rate));
    }

    Ok(DecodedAudio::from_stereo_pcm(stereo_pcm, source_rate))
}

fn append_as_stereo(src: &[f32], channels: usize, frames: usize, dst: &mut Vec<f32>) {
    dst.reserve(frames * 2);
    match channels {
        1 => {
            for &s in src.iter().take(frames) {
                dst.push(s);
                dst.push(s);
            }
        }
        2 => {
            dst.extend_from_slice(&src[..frames * 2]);
        }
        n => {
            for frame in src.chunks_exact(n).take(frames) {
                dst.push(frame[0]);
                dst.push(frame[1]);
            }
        }
    }
}

fn resample_stereo(stereo: &[f32], src_rate: u32, dst_rate: u32) -> Result<Vec<f32>, DecodeError> {
    let input_frames = stereo.len() / 2;
    if input_frames == 0 {
        return Ok(Vec::new());
    }

    let mut resampler = Fft::<f32>::new(
        src_rate as usize,
        dst_rate as usize,
        1024,
        2,
        FixedSync::Input,
    )
    .map_err(|e| DecodeError::Resample(e.to_string()))?;

    let in_adapter = InterleavedSlice::new(stereo, 2, input_frames)
        .map_err(|e| DecodeError::Resample(e.to_string()))?;

    let needed_out = resampler.process_all_needed_output_len(input_frames);
    let mut out = vec![0.0_f32; needed_out * 2];
    let mut out_adapter = InterleavedSlice::new_mut(&mut out, 2, needed_out)
        .map_err(|e| DecodeError::Resample(e.to_string()))?;

    let (_in_used, out_written) = resampler
        .process_all_into_buffer(&in_adapter, &mut out_adapter, input_frames, None)
        .map_err(|e| DecodeError::Resample(e.to_string()))?;

    out.truncate(out_written * 2);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_mono_pcm_wav(sample_rate: u32, samples: &[i16]) -> Vec<u8> {
        let data_bytes = (samples.len() * 2) as u32;
        let riff_size = 36 + data_bytes;
        let byte_rate = sample_rate * 2;
        let mut buf = Vec::with_capacity(44 + data_bytes as usize);
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&riff_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16_u32.to_le_bytes());
        buf.extend_from_slice(&1_u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&1_u16.to_le_bytes()); // mono
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&2_u16.to_le_bytes()); // block align
        buf.extend_from_slice(&16_u16.to_le_bytes()); // bits per sample
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_bytes.to_le_bytes());
        for &s in samples {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        buf
    }

    #[test]
    fn decodes_and_resamples_wav_to_stereo() {
        // 0.1s mono 22050 Hz -> resampled to 44100 Hz stereo
        let src_samples = vec![8_192_i16; 2_205];
        let wav = build_mono_pcm_wav(22_050, &src_samples);
        let decoded = decode_bytes(wav, Some("wav"), 44_100).expect("decode wav");

        assert_eq!(decoded.sample_rate, 44_100);
        assert!(decoded.frames >= 4_000);
        let mid = decoded.sample_frame(decoded.frames / 2);
        assert!((mid[0] - mid[1]).abs() < 1e-4);
        assert!(mid[0] > 0.15);
    }
}
