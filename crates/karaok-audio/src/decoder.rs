use std::fs::File;
use std::path::Path;

use rubato::{FftFixedIn, Resampler};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::engine::MASTER_SAMPLE_RATE;
use crate::error::{AudioError, AudioResult};

/// Decoded stereo audio buffer normalized to 48 000 Hz `f32` planar channels.
#[derive(Debug, Clone)]
pub struct DecodedAudio {
    pub sample_rate: u32,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

impl DecodedAudio {
    pub fn frames(&self) -> usize {
        self.left.len().min(self.right.len())
    }

    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        ((self.frames() as u64) * 1000) / (self.sample_rate as u64)
    }
}

/// Decodes MP3, FLAC, WAV, AAC/M4A, or OGG via `symphonia` and resamples high-precision
/// to stereo 48 000 Hz using `rubato`.
pub fn decode_audio_to_48khz_stereo(path: &Path) -> AudioResult<DecodedAudio> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| AudioError::Decode(e.to_string()))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| AudioError::Decode("No supported audio track found".into()))?;

    let track_id = track.id;
    let source_rate = track.codec_params.sample_rate.unwrap_or(MASTER_SAMPLE_RATE);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| AudioError::Decode(e.to_string()))?;

    let mut raw_left: Vec<f32> = Vec::new();
    let mut raw_right: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(SymphoniaError::ResetRequired) => continue,
            Err(err) => return Err(AudioError::Decode(err.to_string())),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(err) => return Err(AudioError::Decode(err.to_string())),
        };

        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1);
        let mut sample_buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);
        let samples = sample_buf.samples();

        if channels == 1 {
            for &s in samples {
                raw_left.push(s);
                raw_right.push(s);
            }
        } else {
            for frame in samples.chunks_exact(channels) {
                raw_left.push(frame[0]);
                raw_right.push(frame[1]);
            }
        }
    }

    if source_rate == MASTER_SAMPLE_RATE || raw_left.is_empty() {
        return Ok(DecodedAudio {
            sample_rate: MASTER_SAMPLE_RATE,
            left: raw_left,
            right: raw_right,
        });
    }

    resample_stereo_rubato(&raw_left, &raw_right, source_rate, MASTER_SAMPLE_RATE)
}

/// High-precision stereo resampling using `rubato::FftFixedIn`.
pub fn resample_stereo_rubato(
    left: &[f32],
    right: &[f32],
    in_rate: u32,
    out_rate: u32,
) -> AudioResult<DecodedAudio> {
    let chunk_size = 1024usize;
    let mut resampler =
        FftFixedIn::<f32>::new(in_rate as usize, out_rate as usize, chunk_size, 2, 2)
            .map_err(|e| AudioError::Resample(e.to_string()))?;

    let mut out_l = Vec::with_capacity((left.len() as u64 * out_rate as u64 / in_rate as u64) as usize + 2048);
    let mut out_r = Vec::with_capacity(out_l.capacity());

    let total = left.len().min(right.len());
    let mut pos = 0usize;
    while pos < total {
        let needed = resampler.input_frames_next();
        let end = (pos + needed).min(total);
        let mut chunk_l = left[pos..end].to_vec();
        let mut chunk_r = right[pos..end].to_vec();
        if chunk_l.len() < needed {
            chunk_l.resize(needed, 0.0);
            chunk_r.resize(needed, 0.0);
        }
        let waves = resampler
            .process(&[chunk_l, chunk_r], None)
            .map_err(|e| AudioError::Resample(e.to_string()))?;
        out_l.extend_from_slice(&waves[0]);
        out_r.extend_from_slice(&waves[1]);
        pos += needed;
    }

    let expected_len = ((total as f64) * (out_rate as f64) / (in_rate as f64)).round() as usize;
    out_l.truncate(expected_len);
    out_r.truncate(expected_len);

    Ok(DecodedAudio {
        sample_rate: out_rate,
        left: out_l,
        right: out_r,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn resamples_44100_to_48000_cleanly() {
        let in_rate = 44_100u32;
        let frames = in_rate as usize; // 1 second
        let left: Vec<f32> = (0..frames)
            .map(|i| 0.5 * (TAU * 440.0 * i as f32 / in_rate as f32).sin())
            .collect();
        let right = left.clone();

        let resampled = resample_stereo_rubato(&left, &right, in_rate, 48_000).unwrap();
        assert_eq!(resampled.sample_rate, 48_000);
        assert!((resampled.frames() as i64 - 48_000).abs() <= 4);
        assert!(resampled.left.iter().all(|s| s.is_finite()));
    }
}
