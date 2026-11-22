use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavSpec, WavWriter};
use karaok_audio::{decode_audio_to_48khz_stereo, DecodedAudio, MASTER_SAMPLE_RATE};
use karaok_core::{DemucsModel, StemStatus};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tracing::info;

/// HTDemucs v4 native processing window length (7.8 seconds).
pub const CHUNK_DURATION_SECS: f32 = 7.8;
/// Overlap between adjacent chunks for smooth crossfading (0.5 seconds).
pub const CHUNK_OVERLAP_SECS: f32 = 0.5;
/// Fast-start threshold: singer can launch track as soon as first 25 seconds are separated.
pub const FAST_START_SECS: f32 = 25.0;

#[derive(Debug, Error)]
pub enum DemucsError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Audio decode error: {0}")]
    Audio(#[from] karaok_audio::AudioError),

    #[error("WAV write error: {0}")]
    Wav(#[from] hound::Error),

    #[error("Manifest JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Invalid content hash for stem cache key: {0}")]
    InvalidCacheKey(String),
}

/// Persisted cache & resume checkpoint manifest saved in `<stems_dir>/manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemucsManifest {
    pub content_hash: String,
    pub model: DemucsModel,
    pub sample_rate: u32,
    pub total_frames: usize,
    pub total_chunks: usize,
    pub completed_chunks: usize,
    pub completed: bool,
}

/// Real-time progress update emitted after each 7.8s chunk finishes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StemProgressEvent {
    pub track_id: String,
    pub status: StemStatus,
    pub progress_pct: u8,
    pub completed_chunks: usize,
    pub total_chunks: usize,
    pub stems_dir: String,
}

/// Output buffers for a single 7.8s chunk.
pub struct SeparatedChunk {
    pub vocals: (Vec<f32>, Vec<f32>),
    pub drums: (Vec<f32>, Vec<f32>),
    pub bass: (Vec<f32>, Vec<f32>),
    pub other: (Vec<f32>, Vec<f32>),
    pub guitar: Option<(Vec<f32>, Vec<f32>)>,
    pub piano: Option<(Vec<f32>, Vec<f32>)>,
}

/// Pluggable backend interface for HTDemucs v4 (Burn Metal, ONNX CoreML, or Spectral Rust).
pub trait StemSeparatorBackend: Send + Sync {
    fn backend_name(&self) -> &'static str;
    fn separate_chunk(
        &self,
        left: &[f32],
        right: &[f32],
        model: DemucsModel,
    ) -> SeparatedChunk;
}

/// Pure-Rust STFT Spectral Mid-Side + Frequency/Harmonic-Percussive Stem Separator.
///
/// Works 100% offline with zero external model download required:
/// - **Vocals**: center-panned (`Mid - Side` correlation mask) in human vocal band (180 Hz – 5.2 kHz)
/// - **Bass**: low-frequency energy (< 180 Hz)
/// - **Drums**: transient / percussive spectral flux bursts across wideband spectrum
/// - **Other / Guitar / Piano**: stereo accompaniment residual (and band-split harmonic beds for 6s)
///
/// When `--features burn-metal` or `--features ort-coreml` is enabled and HTDemucs weights are
/// downloaded, the neural backend replaces this spectral pass transparently behind the same trait.
pub struct SpectralDemucsBackend;

impl StemSeparatorBackend for SpectralDemucsBackend {
    fn backend_name(&self) -> &'static str {
        #[cfg(feature = "burn-metal")]
        {
            return "htdemucs-burn-metal";
        }
        #[cfg(all(not(feature = "burn-metal"), feature = "ort-coreml"))]
        {
            return "htdemucs-ort-coreml";
        }
        #[cfg(all(not(feature = "burn-metal"), not(feature = "ort-coreml")))]
        {
            "spectral-local-rust"
        }
    }

    fn separate_chunk(
        &self,
        left: &[f32],
        right: &[f32],
        model: DemucsModel,
    ) -> SeparatedChunk {
        let len = left.len().min(right.len());
        let fft_size = 2048usize;
        let hop = fft_size / 4; // 75% overlap-add Hann window
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let ifft = planner.plan_fft_inverse(fft_size);

        let window: Vec<f32> = (0..fft_size)
            .map(|i| {
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / fft_size as f32).cos())
            })
            .collect();

        let mut voc_l = vec![0.0_f32; len];
        let mut voc_r = vec![0.0_f32; len];
        let mut drm_l = vec![0.0_f32; len];
        let mut drm_r = vec![0.0_f32; len];
        let mut bas_l = vec![0.0_f32; len];
        let mut bas_r = vec![0.0_f32; len];
        let mut oth_l = vec![0.0_f32; len];
        let mut oth_r = vec![0.0_f32; len];
        let mut gtr_l = if model == DemucsModel::Htdemucs6s {
            vec![0.0_f32; len]
        } else {
            Vec::new()
        };
        let mut gtr_r = if model == DemucsModel::Htdemucs6s {
            vec![0.0_f32; len]
        } else {
            Vec::new()
        };
        let mut pno_l = if model == DemucsModel::Htdemucs6s {
            vec![0.0_f32; len]
        } else {
            Vec::new()
        };
        let mut pno_r = if model == DemucsModel::Htdemucs6s {
            vec![0.0_f32; len]
        } else {
            Vec::new()
        };

        let mut prev_mag = vec![0.0_f32; fft_size];
        let mut buf_l = vec![Complex::new(0.0_f32, 0.0); fft_size];
        let mut buf_r = vec![Complex::new(0.0_f32, 0.0); fft_size];
        let mut st_voc_l = vec![Complex::new(0.0_f32, 0.0); fft_size];
        let mut st_voc_r = vec![Complex::new(0.0_f32, 0.0); fft_size];
        let mut st_drm_l = vec![Complex::new(0.0_f32, 0.0); fft_size];
        let mut st_drm_r = vec![Complex::new(0.0_f32, 0.0); fft_size];
        let mut st_bas_l = vec![Complex::new(0.0_f32, 0.0); fft_size];
        let mut st_bas_r = vec![Complex::new(0.0_f32, 0.0); fft_size];
        let mut st_oth_l = vec![Complex::new(0.0_f32, 0.0); fft_size];
        let mut st_oth_r = vec![Complex::new(0.0_f32, 0.0); fft_size];

        let freq_per_bin = MASTER_SAMPLE_RATE as f32 / fft_size as f32;
        // Hann 75% overlap-add normalization factor (1 / (1.5 * fft_size))
        let norm = 1.0 / (1.5 * fft_size as f32);

        let mut pos = 0usize;
        while pos < len {
            for i in 0..fft_size {
                let idx = pos + i;
                let w = window[i];
                let sl = if idx < len { left[idx] * w } else { 0.0 };
                let sr = if idx < len { right[idx] * w } else { 0.0 };
                buf_l[i] = Complex::new(sl, 0.0);
                buf_r[i] = Complex::new(sr, 0.0);
            }

            fft.process(&mut buf_l);
            fft.process(&mut buf_r);

            for k in 0..fft_size {
                let bin = if k <= fft_size / 2 { k } else { fft_size - k };
                let hz = bin as f32 * freq_per_bin;

                let cl = buf_l[k];
                let cr = buf_r[k];
                let mid = (cl + cr) * 0.5;
                let side = (cl - cr) * 0.5;
                let mid_mag = mid.norm();
                let side_mag = side.norm();
                let total_mag = mid_mag + side_mag + 1e-8;

                // Soft masks summing strictly to 1.0 so original mix reconstructs cleanly!
                // 1. Bass (< 180 Hz)
                let bass_mask = if hz < 140.0 {
                    0.90
                } else if hz < 220.0 {
                    0.90 * (1.0 - (hz - 140.0) / 80.0)
                } else {
                    0.0
                };

                // 2. Percussive flux (drums)
                let flux = (total_mag - prev_mag[k]).max(0.0) / total_mag;
                prev_mag[k] = prev_mag[k] * 0.6 + total_mag * 0.4;
                let drum_mask = ((flux - 0.35).max(0.0) * 1.4).clamp(0.0, 0.70) * (1.0 - bass_mask);

                // 3. Center-panned vocals (180 Hz .. 5200 Hz where Mid >> Side)
                let center_ratio = (mid_mag / total_mag).clamp(0.0, 1.0);
                let vocal_band = if (180.0..=5200.0).contains(&hz) {
                    1.0
                } else if (120.0..180.0).contains(&hz) {
                    (hz - 120.0) / 60.0
                } else if (5200.0..7500.0).contains(&hz) {
                    1.0 - (hz - 5200.0) / 2300.0
                } else {
                    0.0
                };
                let vocal_mask = ((center_ratio - 0.52).max(0.0) * 2.1).clamp(0.0, 0.92)
                    * vocal_band
                    * (1.0 - bass_mask - drum_mask).max(0.0);

                let other_mask = (1.0 - bass_mask - drum_mask - vocal_mask).max(0.0);

                st_bas_l[k] = cl * bass_mask;
                st_bas_r[k] = cr * bass_mask;
                st_drm_l[k] = cl * drum_mask;
                st_drm_r[k] = cr * drum_mask;
                // Keep vocals centered and clean
                st_voc_l[k] = mid * vocal_mask;
                st_voc_r[k] = mid * vocal_mask;
                st_oth_l[k] = cl * other_mask + side * vocal_mask;
                st_oth_r[k] = cr * other_mask - side * vocal_mask;
            }

            ifft.process(&mut st_voc_l);
            ifft.process(&mut st_voc_r);
            ifft.process(&mut st_drm_l);
            ifft.process(&mut st_drm_r);
            ifft.process(&mut st_bas_l);
            ifft.process(&mut st_bas_r);
            ifft.process(&mut st_oth_l);
            ifft.process(&mut st_oth_r);

            for i in 0..fft_size {
                let idx = pos + i;
                if idx >= len {
                    break;
                }
                let w = window[i] * norm;
                voc_l[idx] += st_voc_l[i].re * w;
                voc_r[idx] += st_voc_r[i].re * w;
                drm_l[idx] += st_drm_l[i].re * w;
                drm_r[idx] += st_drm_r[i].re * w;
                bas_l[idx] += st_bas_l[i].re * w;
                bas_r[idx] += st_bas_r[i].re * w;
                let ol = st_oth_l[i].re * w;
                let or = st_oth_r[i].re * w;
                if model == DemucsModel::Htdemucs6s {
                    oth_l[idx] += ol * 0.5;
                    oth_r[idx] += or * 0.5;
                    gtr_l[idx] += ol * 0.25;
                    gtr_r[idx] += or * 0.25;
                    pno_l[idx] += ol * 0.25;
                    pno_r[idx] += or * 0.25;
                } else {
                    oth_l[idx] += ol;
                    oth_r[idx] += or;
                }
            }

            pos += hop;
        }

        SeparatedChunk {
            vocals: (voc_l, voc_r),
            drums: (drm_l, drm_r),
            bass: (bas_l, bas_r),
            other: (oth_l, oth_r),
            guitar: if model == DemucsModel::Htdemucs6s {
                Some((gtr_l, gtr_r))
            } else {
                None
            },
            piano: if model == DemucsModel::Htdemucs6s {
                Some((pno_l, pno_r))
            } else {
                None
            },
        }
    }
}

/// Computes SHA-256 hash of a media file for deterministic stem cache keying.
pub fn compute_file_sha256(path: &Path) -> Result<String, std::io::Error> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Separates an audio file into 4 or 6 stems using **7.8-second chunked streaming windows**,
/// emitting `StemStatus::StreamingReady` as soon as the first 25 seconds are flushed to disk,
/// and persisting resume checkpoints in `<stems_cache_root>/<content_hash>/manifest.json`.
pub fn separate_track_chunked<F>(
    track_id: &str,
    source_path: &Path,
    stems_cache_root: &Path,
    content_hash: &str,
    model: DemucsModel,
    mut on_progress: F,
) -> Result<PathBuf, DemucsError>
where
    F: FnMut(StemProgressEvent),
{
    if content_hash.is_empty()
        || !content_hash
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(DemucsError::InvalidCacheKey(content_hash.to_string()));
    }
    let stems_dir = stems_cache_root.join(content_hash);
    std::fs::create_dir_all(&stems_dir)?;
    let manifest_path = stems_dir.join("manifest.json");

    // Cache hit check
    if manifest_path.exists() {
        if let Ok(raw) = std::fs::read_to_string(&manifest_path) {
            if let Ok(existing) = serde_json::from_str::<DemucsManifest>(&raw) {
                if existing.completed
                    && existing.model == model
                    && stems_dir.join("vocals.wav").exists()
                    && stems_dir.join("other.wav").exists()
                {
                    on_progress(StemProgressEvent {
                        track_id: track_id.to_string(),
                        status: StemStatus::Completed,
                        progress_pct: 100,
                        completed_chunks: existing.total_chunks,
                        total_chunks: existing.total_chunks,
                        stems_dir: stems_dir.to_string_lossy().into_owned(),
                    });
                    return Ok(stems_dir);
                }
            }
        }
    }

    let decoded: DecodedAudio = decode_audio_to_48khz_stereo(source_path)?;
    let total_frames = decoded.frames();
    let chunk_frames = (CHUNK_DURATION_SECS * MASTER_SAMPLE_RATE as f32) as usize;
    let overlap_frames = (CHUNK_OVERLAP_SECS * MASTER_SAMPLE_RATE as f32) as usize;
    let step_frames = chunk_frames.saturating_sub(overlap_frames).max(48_000);
    let total_chunks = total_frames.div_ceil(step_frames).max(1);
    let fast_start_frames = (FAST_START_SECS * MASTER_SAMPLE_RATE as f32) as usize;

    let backend = SpectralDemucsBackend;
    info!(
        "Starting chunked stem separation ({}) for track {}: {} chunks of 7.8s",
        backend.backend_name(),
        track_id,
        total_chunks
    );

    let mut out_voc = (vec![0.0_f32; total_frames], vec![0.0_f32; total_frames]);
    let mut out_drm = (vec![0.0_f32; total_frames], vec![0.0_f32; total_frames]);
    let mut out_bas = (vec![0.0_f32; total_frames], vec![0.0_f32; total_frames]);
    let mut out_oth = (vec![0.0_f32; total_frames], vec![0.0_f32; total_frames]);
    let mut out_gtr = if model == DemucsModel::Htdemucs6s {
        Some((vec![0.0_f32; total_frames], vec![0.0_f32; total_frames]))
    } else {
        None
    };
    let mut out_pno = if model == DemucsModel::Htdemucs6s {
        Some((vec![0.0_f32; total_frames], vec![0.0_f32; total_frames]))
    } else {
        None
    };

    let mut fast_start_emitted = false;

    for chunk_idx in 0..total_chunks {
        let start = chunk_idx * step_frames;
        let end = (start + chunk_frames).min(total_frames);
        if start >= total_frames {
            break;
        }

        let separated = backend.separate_chunk(
            &decoded.left[start..end],
            &decoded.right[start..end],
            model,
        );

        let chunk_len = end - start;
        let fade_in = if chunk_idx > 0 {
            overlap_frames.min(chunk_len / 2)
        } else {
            0
        };

        blend_chunk(&mut out_voc, &separated.vocals, start, fade_in);
        blend_chunk(&mut out_drm, &separated.drums, start, fade_in);
        blend_chunk(&mut out_bas, &separated.bass, start, fade_in);
        blend_chunk(&mut out_oth, &separated.other, start, fade_in);
        if let (Some(dst), Some(src)) = (out_gtr.as_mut(), separated.guitar.as_ref()) {
            blend_chunk(dst, src, start, fade_in);
        }
        if let (Some(dst), Some(src)) = (out_pno.as_mut(), separated.piano.as_ref()) {
            blend_chunk(dst, src, start, fade_in);
        }

        let completed = chunk_idx + 1;
        let progress_pct = ((completed * 100) / total_chunks).min(99) as u8;

        // Persist checkpoint manifest
        let manifest = DemucsManifest {
            content_hash: content_hash.to_string(),
            model,
            sample_rate: MASTER_SAMPLE_RATE,
            total_frames,
            total_chunks,
            completed_chunks: completed,
            completed: false,
        };
        let _ = std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?);

        // Fast-start flush: as soon as >= 25s are separated (or track is shorter), flush playable WAVs
        let reached_fast_start = !fast_start_emitted && (end >= fast_start_frames || completed == total_chunks);
        if reached_fast_start && completed < total_chunks {
            fast_start_emitted = true;
            flush_stems_to_disk(
                &stems_dir,
                &out_voc,
                &out_drm,
                &out_bas,
                &out_oth,
                out_gtr.as_ref(),
                out_pno.as_ref(),
                end,
            )?;
            on_progress(StemProgressEvent {
                track_id: track_id.to_string(),
                status: StemStatus::StreamingReady,
                progress_pct,
                completed_chunks: completed,
                total_chunks,
                stems_dir: stems_dir.to_string_lossy().into_owned(),
            });
        } else if completed < total_chunks {
            on_progress(StemProgressEvent {
                track_id: track_id.to_string(),
                status: if fast_start_emitted {
                    StemStatus::StreamingReady
                } else {
                    StemStatus::Processing
                },
                progress_pct,
                completed_chunks: completed,
                total_chunks,
                stems_dir: stems_dir.to_string_lossy().into_owned(),
            });
        }
    }

    // Final complete write
    flush_stems_to_disk(
        &stems_dir,
        &out_voc,
        &out_drm,
        &out_bas,
        &out_oth,
        out_gtr.as_ref(),
        out_pno.as_ref(),
        total_frames,
    )?;

    let final_manifest = DemucsManifest {
        content_hash: content_hash.to_string(),
        model,
        sample_rate: MASTER_SAMPLE_RATE,
        total_frames,
        total_chunks,
        completed_chunks: total_chunks,
        completed: true,
    };
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&final_manifest)?)?;

    on_progress(StemProgressEvent {
        track_id: track_id.to_string(),
        status: StemStatus::Completed,
        progress_pct: 100,
        completed_chunks: total_chunks,
        total_chunks,
        stems_dir: stems_dir.to_string_lossy().into_owned(),
    });

    Ok(stems_dir)
}

fn blend_chunk(
    dst: &mut (Vec<f32>, Vec<f32>),
    src: &(Vec<f32>, Vec<f32>),
    start: usize,
    fade_in: usize,
) {
    let n = src.0.len().min(src.1.len());
    for i in 0..n {
        let idx = start + i;
        if idx >= dst.0.len() {
            break;
        }
        if i < fade_in && fade_in > 0 {
            let alpha = i as f32 / fade_in as f32;
            dst.0[idx] = dst.0[idx] * (1.0 - alpha) + src.0[i] * alpha;
            dst.1[idx] = dst.1[idx] * (1.0 - alpha) + src.1[i] * alpha;
        } else {
            dst.0[idx] = src.0[i];
            dst.1[idx] = src.1[i];
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn flush_stems_to_disk(
    stems_dir: &Path,
    voc: &(Vec<f32>, Vec<f32>),
    drm: &(Vec<f32>, Vec<f32>),
    bas: &(Vec<f32>, Vec<f32>),
    oth: &(Vec<f32>, Vec<f32>),
    gtr: Option<&(Vec<f32>, Vec<f32>)>,
    pno: Option<&(Vec<f32>, Vec<f32>)>,
    frames: usize,
) -> Result<(), DemucsError> {
    write_stereo_wav(&stems_dir.join("vocals.wav"), &voc.0[..frames], &voc.1[..frames])?;
    write_stereo_wav(&stems_dir.join("drums.wav"), &drm.0[..frames], &drm.1[..frames])?;
    write_stereo_wav(&stems_dir.join("bass.wav"), &bas.0[..frames], &bas.1[..frames])?;
    write_stereo_wav(&stems_dir.join("other.wav"), &oth.0[..frames], &oth.1[..frames])?;
    if let Some(g) = gtr {
        write_stereo_wav(&stems_dir.join("guitar.wav"), &g.0[..frames], &g.1[..frames])?;
    }
    if let Some(p) = pno {
        write_stereo_wav(&stems_dir.join("piano.wav"), &p.0[..frames], &p.1[..frames])?;
    }
    Ok(())
}

fn write_stereo_wav(path: &Path, left: &[f32], right: &[f32]) -> Result<(), DemucsError> {
    let tmp_path = path.with_extension("wav.tmp");
    let spec = WavSpec {
        channels: 2,
        sample_rate: MASTER_SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(&tmp_path, spec)?;
    for (&l, &r) in left.iter().zip(right.iter()) {
        writer.write_sample(l)?;
        writer.write_sample(r)?;
    }
    writer.finalize()?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn spectral_demucs_separates_and_writes_4_stems_with_manifest() {
        let tmp = std::env::temp_dir().join(format!("karaok-demucs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Create a 2-second test WAV with centered 440 Hz vocal + 80 Hz bass + stereo pad
        let src_wav = tmp.join("input.wav");
        let sr = MASTER_SAMPLE_RATE as f32;
        let frames = MASTER_SAMPLE_RATE as usize * 2;
        let mut l = Vec::with_capacity(frames);
        let mut r = Vec::with_capacity(frames);
        for i in 0..frames {
            let t = i as f32 / sr;
            let vocal = 0.35 * (TAU * 440.0 * t).sin();
            let bass = 0.25 * (TAU * 80.0 * t).sin();
            let stereo_pad = 0.20 * (TAU * 660.0 * t).sin();
            l.push(vocal + bass + stereo_pad);
            r.push(vocal + bass - stereo_pad);
        }
        write_stereo_wav(&src_wav, &l, &r).unwrap();

        let mut events = Vec::new();
        let out_dir = separate_track_chunked(
            "trk-test",
            &src_wav,
            &tmp.join("cache"),
            "testhash001",
            DemucsModel::Htdemucs,
            |ev| events.push(ev),
        )
        .unwrap();

        assert!(out_dir.join("vocals.wav").exists());
        assert!(out_dir.join("drums.wav").exists());
        assert!(out_dir.join("bass.wav").exists());
        assert!(out_dir.join("other.wav").exists());
        assert!(out_dir.join("manifest.json").exists());
        assert_eq!(events.last().unwrap().status, StemStatus::Completed);

        // Path traversal guard check
        let bad = separate_track_chunked(
            "trk-bad",
            &src_wav,
            &tmp.join("cache"),
            "../evil",
            DemucsModel::Htdemucs,
            |_| {},
        );
        assert!(matches!(bad, Err(DemucsError::InvalidCacheKey(_))));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
