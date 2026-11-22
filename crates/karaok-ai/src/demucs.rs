use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use karaok_audio::{decode_audio_to_48khz_stereo, DecodedAudio, MASTER_SAMPLE_RATE};
use karaok_core::{DemucsModel, StemStatus};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tracing::{info, warn};

/// Cache format version:
/// - `2`: fallback zero-artifact time-domain Mid-Side crossover
/// - `3`: Meta AI `HTDemucs v4` / `htdemucs_6s` neural stem separation
pub const STEMS_CACHE_VERSION: u32 = 3;
const FALLBACK_CACHE_VERSION: u32 = 2;

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

    #[error("Neural Demucs process failed: {0}")]
    Process(String),
}

/// Persisted cache & resume checkpoint manifest saved in `<stems_dir>/manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemucsManifest {
    #[serde(default)]
    pub cache_version: u32,
    #[serde(default)]
    pub backend: String,
    pub content_hash: String,
    pub model: DemucsModel,
    pub sample_rate: u32,
    pub total_frames: usize,
    pub total_chunks: usize,
    pub completed_chunks: usize,
    pub completed: bool,
}

/// Real-time progress update emitted while separation runs.
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

/// Output buffers for a single 7.8s fallback chunk.
pub struct SeparatedChunk {
    pub vocals: (Vec<f32>, Vec<f32>),
    pub drums: (Vec<f32>, Vec<f32>),
    pub bass: (Vec<f32>, Vec<f32>),
    pub other: (Vec<f32>, Vec<f32>),
    pub guitar: Option<(Vec<f32>, Vec<f32>)>,
    pub piano: Option<(Vec<f32>, Vec<f32>)>,
}

/// Locates the Meta AI `demucs` CLI executable (app-managed uv venv first, then system `PATH`).
pub fn find_demucs_binary() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("KARAOK_DEMUCS_BIN") {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Some(p);
        }
    }

    if let Some(data_dir) = dirs::data_dir() {
        let p = data_dir.join("KaraokParty/demucs-venv/bin/demucs");
        if p.is_file() {
            return Some(p);
        }
    }

    if let Some(home) = dirs::home_dir() {
        let p = home.join(".local/share/karaok-party/demucs-venv/bin/demucs");
        if p.is_file() {
            return Some(p);
        }
    }

    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("demucs");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

/// Returns `true` when Meta AI `demucs` neural CLI is installed and runnable.
pub fn neural_demucs_available() -> bool {
    find_demucs_binary().is_some()
}

/// Returns `true` when `stems_dir` contains a completed neural HTDemucs cache (`cacheVersion == 3`).
pub fn has_neural_stems_cache(stems_dir: &Path) -> bool {
    let manifest_path = stems_dir.join("manifest.json");
    let Ok(raw) = std::fs::read_to_string(manifest_path) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_str::<DemucsManifest>(&raw) else {
        return false;
    };
    manifest.completed
        && manifest.cache_version >= STEMS_CACHE_VERSION
        && stems_dir.join("vocals.wav").exists()
        && stems_dir.join("other.wav").exists()
}

/// Pluggable backend interface for fallback offline separation.
pub trait StemSeparatorBackend: Send + Sync {
    fn backend_name(&self) -> &'static str;
    fn separate_chunk(
        &self,
        left: &[f32],
        right: &[f32],
        model: DemucsModel,
    ) -> SeparatedChunk;
}

/// Fallback clean time-domain Mid-Side + IIR Crossover separator when neural Demucs binary is absent.
pub struct SpectralDemucsBackend;

impl StemSeparatorBackend for SpectralDemucsBackend {
    fn backend_name(&self) -> &'static str {
        "mid-side-crossover-fallback"
    }

    fn separate_chunk(
        &self,
        left: &[f32],
        right: &[f32],
        model: DemucsModel,
    ) -> SeparatedChunk {
        let len = left.len().min(right.len());
        let sr = MASTER_SAMPLE_RATE as f32;

        let alpha_bass = 1.0 - (-2.0 * std::f32::consts::PI * 170.0 / sr).exp();
        let alpha_high = 1.0 - (-2.0 * std::f32::consts::PI * 5200.0 / sr).exp();

        let mut voc_l = Vec::with_capacity(len);
        let mut voc_r = Vec::with_capacity(len);
        let mut drm_l = Vec::with_capacity(len);
        let mut drm_r = Vec::with_capacity(len);
        let mut bas_l = Vec::with_capacity(len);
        let mut bas_r = Vec::with_capacity(len);
        let mut oth_l = Vec::with_capacity(len);
        let mut oth_r = Vec::with_capacity(len);
        let six_stem = model == DemucsModel::Htdemucs6s;
        let cap6 = if six_stem { len } else { 0 };
        let mut gtr_l = Vec::with_capacity(cap6);
        let mut gtr_r = Vec::with_capacity(cap6);
        let mut pno_l = Vec::with_capacity(cap6);
        let mut pno_r = Vec::with_capacity(cap6);

        let mut lp_bass_l = 0.0_f32;
        let mut lp_bass_r = 0.0_f32;
        let mut lp_mid_high = 0.0_f32;

        for i in 0..len {
            let l = left[i];
            let r = right[i];

            lp_bass_l += alpha_bass * (l - lp_bass_l);
            lp_bass_r += alpha_bass * (r - lp_bass_r);
            let hi_l = l - lp_bass_l;
            let hi_r = r - lp_bass_r;

            let mid = 0.5 * (hi_l + hi_r);
            let side = 0.5 * (hi_l - hi_r);

            lp_mid_high += alpha_high * (mid - lp_mid_high);
            let vocal = lp_mid_high;
            let high_mid = mid - lp_mid_high;

            bas_l.push(lp_bass_l);
            bas_r.push(lp_bass_r);
            voc_l.push(vocal);
            voc_r.push(vocal);
            drm_l.push(high_mid);
            drm_r.push(high_mid);

            if six_stem {
                oth_l.push(side * 0.5);
                oth_r.push(-side * 0.5);
                gtr_l.push(side * 0.25);
                gtr_r.push(-side * 0.25);
                pno_l.push(side * 0.25);
                pno_r.push(-side * 0.25);
            } else {
                oth_l.push(side);
                oth_r.push(-side);
            }
        }

        SeparatedChunk {
            vocals: (voc_l, voc_r),
            drums: (drm_l, drm_r),
            bass: (bas_l, bas_r),
            other: (oth_l, oth_r),
            guitar: if six_stem { Some((gtr_l, gtr_r)) } else { None },
            piano: if six_stem { Some((pno_l, pno_r)) } else { None },
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

/// Runs neural Meta AI `HTDemucs` when installed (`cacheVersion: 3`), or falls back cleanly
/// to the zero-artifact Mid-Side crossover separator (`cacheVersion: 2`).
pub fn separate_track_chunked<F>(
    track_id: &str,
    source_path: &Path,
    stems_cache_root: &Path,
    content_hash: &str,
    model: DemucsModel,
    on_progress: F,
) -> Result<PathBuf, DemucsError>
where
    F: FnMut(StemProgressEvent),
{
    separate_track_with_mode(
        track_id,
        source_path,
        stems_cache_root,
        content_hash,
        model,
        true,
        on_progress,
    )
}

fn separate_track_with_mode<F>(
    track_id: &str,
    source_path: &Path,
    stems_cache_root: &Path,
    content_hash: &str,
    model: DemucsModel,
    allow_neural: bool,
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

    let neural_bin = if allow_neural {
        find_demucs_binary()
    } else {
        None
    };
    let required_version = if neural_bin.is_some() {
        STEMS_CACHE_VERSION
    } else {
        FALLBACK_CACHE_VERSION
    };

    // Cache hit check (automatically upgrades legacy v1/v2 stems when neural HTDemucs is present!)
    if manifest_path.exists() {
        if let Ok(raw) = std::fs::read_to_string(&manifest_path) {
            if let Ok(existing) = serde_json::from_str::<DemucsManifest>(&raw) {
                if existing.completed
                    && existing.cache_version >= required_version
                    && existing.model == model
                    && stems_dir.join("vocals.wav").exists()
                    && stems_dir.join("other.wav").exists()
                {
                    on_progress(StemProgressEvent {
                        track_id: track_id.to_string(),
                        status: StemStatus::Completed,
                        progress_pct: 100,
                        completed_chunks: existing.total_chunks.max(1),
                        total_chunks: existing.total_chunks.max(1),
                        stems_dir: stems_dir.to_string_lossy().into_owned(),
                    });
                    return Ok(stems_dir);
                }
            }
        }
    }

    // 1. Primary Studio Path: Real Meta AI HTDemucs neural source separation (GPU MPS on Apple Silicon)
    if let Some(bin) = neural_bin {
        match run_neural_demucs(
            &bin,
            track_id,
            source_path,
            &stems_dir,
            content_hash,
            model,
            &mut on_progress,
        ) {
            Ok(()) => return Ok(stems_dir),
            Err(err) => {
                warn!("Neural HTDemucs failed, falling back to Mid-Side separator: {err}");
            }
        }
    }

    // 2. Fallback Path (offline / no Python neural runtime)
    run_fallback_chunked(
        track_id,
        source_path,
        &stems_dir,
        content_hash,
        model,
        &mut on_progress,
    )?;
    Ok(stems_dir)
}

fn run_neural_demucs<F>(
    demucs_bin: &Path,
    track_id: &str,
    source_path: &Path,
    stems_dir: &Path,
    content_hash: &str,
    model: DemucsModel,
    on_progress: &mut F,
) -> Result<(), DemucsError>
where
    F: FnMut(StemProgressEvent),
{
    let model_name = match model {
        DemucsModel::Htdemucs => "htdemucs",
        DemucsModel::Htdemucs6s => "htdemucs_6s",
    };

    let tmp_out = stems_dir.join(".tmp_neural");
    let _ = std::fs::remove_dir_all(&tmp_out);
    std::fs::create_dir_all(&tmp_out)?;

    on_progress(StemProgressEvent {
        track_id: track_id.to_string(),
        status: StemStatus::Processing,
        progress_pct: 3,
        completed_chunks: 0,
        total_chunks: 100,
        stems_dir: stems_dir.to_string_lossy().into_owned(),
    });

    let devices: &[&str] = if cfg!(target_os = "macos") {
        &["mps", "cpu"]
    } else {
        &["cpu"]
    };

    let mut succeeded = false;
    let mut last_err = String::new();

    for &dev in devices {
        let _ = std::fs::remove_dir_all(&tmp_out);
        std::fs::create_dir_all(&tmp_out)?;

        info!(
            "Running neural Demucs ({model_name} on device={dev}) for track {track_id}"
        );

        let mut child = Command::new(demucs_bin)
            .args(["-n", model_name, "-d", dev, "--float32", "-o"])
            .arg(&tmp_out)
            .arg(source_path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;

        if let Some(mut stderr) = child.stderr.take() {
            let mut buf = [0u8; 256];
            let mut line_acc = String::new();
            let mut last_pct: u8 = 3;
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let text = String::from_utf8_lossy(&buf[..n]);
                for ch in text.chars() {
                    if ch == '\r' || ch == '\n' {
                        if let Some(pct) = parse_tqdm_pct(&line_acc) {
                            let scaled = pct.clamp(3, 96);
                            if scaled > last_pct {
                                last_pct = scaled;
                                on_progress(StemProgressEvent {
                                    track_id: track_id.to_string(),
                                    status: StemStatus::Processing,
                                    progress_pct: scaled,
                                    completed_chunks: scaled as usize,
                                    total_chunks: 100,
                                    stems_dir: stems_dir.to_string_lossy().into_owned(),
                                });
                            }
                        }
                        line_acc.clear();
                    } else if line_acc.len() < 200 {
                        line_acc.push(ch);
                    }
                }
            }
        }

        let status = child.wait()?;
        if status.success() {
            succeeded = true;
            break;
        }
        last_err = format!("demucs exited with status {status} on device {dev}");
        warn!("{last_err}");
    }

    if !succeeded {
        let _ = std::fs::remove_dir_all(&tmp_out);
        return Err(DemucsError::Process(last_err));
    }

    // Locate `<tmp_out>/<model_name>/<track_stem>/` containing vocals.wav & other.wav
    let model_dir = tmp_out.join(model_name);
    let mut stem_subdir: Option<PathBuf> = None;
    if let Ok(entries) = std::fs::read_dir(&model_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() && p.join("vocals.wav").exists() && p.join("other.wav").exists() {
                stem_subdir = Some(p);
                break;
            }
        }
    }

    let Some(src_dir) = stem_subdir else {
        let _ = std::fs::remove_dir_all(&tmp_out);
        return Err(DemucsError::Process(
            "Demucs output stems directory was not created".into(),
        ));
    };

    let stem_files: &[&str] = if model == DemucsModel::Htdemucs6s {
        &["vocals.wav", "drums.wav", "bass.wav", "other.wav", "guitar.wav", "piano.wav"]
    } else {
        &["vocals.wav", "drums.wav", "bass.wav", "other.wav"]
    };

    for fname in stem_files {
        let src = src_dir.join(fname);
        if src.exists() {
            let dst = stems_dir.join(fname);
            let tmp_dst = stems_dir.join(format!("{fname}.tmp"));
            if std::fs::rename(&src, &tmp_dst).is_err() {
                std::fs::copy(&src, &tmp_dst)?;
            }
            std::fs::rename(&tmp_dst, &dst)?;
        }
    }

    let (sample_rate, total_frames) = WavReader::open(stems_dir.join("vocals.wav"))
        .map(|r| {
            let spec = r.spec();
            let ch = spec.channels.max(1) as usize;
            (spec.sample_rate, r.len() as usize / ch)
        })
        .unwrap_or((44_100, 0));

    let _ = std::fs::remove_dir_all(&tmp_out);

    let final_manifest = DemucsManifest {
        cache_version: STEMS_CACHE_VERSION,
        backend: model_name.to_string(),
        content_hash: content_hash.to_string(),
        model,
        sample_rate,
        total_frames,
        total_chunks: 100,
        completed_chunks: 100,
        completed: true,
    };
    std::fs::write(
        stems_dir.join("manifest.json"),
        serde_json::to_string_pretty(&final_manifest)?,
    )?;

    on_progress(StemProgressEvent {
        track_id: track_id.to_string(),
        status: StemStatus::Completed,
        progress_pct: 100,
        completed_chunks: 100,
        total_chunks: 100,
        stems_dir: stems_dir.to_string_lossy().into_owned(),
    });

    info!(
        "Neural HTDemucs separation complete for track {track_id} -> {}",
        stems_dir.display()
    );
    Ok(())
}

fn parse_tqdm_pct(line: &str) -> Option<u8> {
    let idx = line.find('%')?;
    let prefix = line[..idx].trim();
    let digits: String = prefix
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u8>().ok()
}

fn run_fallback_chunked<F>(
    track_id: &str,
    source_path: &Path,
    stems_dir: &Path,
    content_hash: &str,
    model: DemucsModel,
    on_progress: &mut F,
) -> Result<(), DemucsError>
where
    F: FnMut(StemProgressEvent),
{
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

    let manifest_path = stems_dir.join("manifest.json");
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

        let manifest = DemucsManifest {
            cache_version: FALLBACK_CACHE_VERSION,
            backend: backend.backend_name().to_string(),
            content_hash: content_hash.to_string(),
            model,
            sample_rate: MASTER_SAMPLE_RATE,
            total_frames,
            total_chunks,
            completed_chunks: completed,
            completed: false,
        };
        let _ = std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?);

        let reached_fast_start =
            !fast_start_emitted && (end >= fast_start_frames || completed == total_chunks);
        if reached_fast_start && completed < total_chunks {
            fast_start_emitted = true;
            flush_stems_to_disk(
                stems_dir,
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

    flush_stems_to_disk(
        stems_dir,
        &out_voc,
        &out_drm,
        &out_bas,
        &out_oth,
        out_gtr.as_ref(),
        out_pno.as_ref(),
        total_frames,
    )?;

    let final_manifest = DemucsManifest {
        cache_version: FALLBACK_CACHE_VERSION,
        backend: backend.backend_name().to_string(),
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

    Ok(())
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
    fn parses_demucs_tqdm_progress_percentages() {
        assert_eq!(parse_tqdm_pct("  37%|██████   | 87.75/239.85"), Some(37));
        assert_eq!(parse_tqdm_pct("100%|██████| 239.85/239.85"), Some(100));
        assert_eq!(parse_tqdm_pct("Loading weights..."), None);
    }

    #[test]
    fn spectral_demucs_separates_and_writes_4_stems_with_manifest() {
        let tmp = std::env::temp_dir().join(format!("karaok-demucs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

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
        let out_dir = separate_track_with_mode(
            "trk-test",
            &src_wav,
            &tmp.join("cache"),
            "testhash001",
            DemucsModel::Htdemucs,
            false,
            |ev| events.push(ev),
        )
        .unwrap();

        assert!(out_dir.join("vocals.wav").exists());
        assert!(out_dir.join("drums.wav").exists());
        assert!(out_dir.join("bass.wav").exists());
        assert!(out_dir.join("other.wav").exists());
        assert!(out_dir.join("manifest.json").exists());
        assert_eq!(events.last().unwrap().status, StemStatus::Completed);

        let bad = separate_track_with_mode(
            "trk-bad",
            &src_wav,
            &tmp.join("cache"),
            "../evil",
            DemucsModel::Htdemucs,
            false,
            |_| {},
        );
        assert!(matches!(bad, Err(DemucsError::InvalidCacheKey(_))));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "Requires installed neural HTDemucs venv + local Stay audio track"]
    fn upgrade_stay_track_with_neural_htdemucs_and_verify_attenuation() {
        assert!(
            neural_demucs_available(),
            "Neural HTDemucs binary must be installed in KaraokParty/demucs-venv"
        );
        let data_dir = dirs::data_local_dir()
            .unwrap()
            .join("KaraokParty");
        let src_wav = data_dir.join("downloads").join("_bXqoIzH0N8.wav");
        if !src_wav.exists() {
            panic!("Missing test fixture audio: {}", src_wav.display());
        }
        let stems_root = data_dir.join("stems");
        let hash = "464bbd56d73bb327890475c51c8a562da57191e0fc9f8364aad2512488dc15f4";

        let mut events = Vec::new();
        let out_dir = separate_track_chunked(
            "ef931612-e295-487e-ab08-4dac256cdc2e",
            &src_wav,
            &stems_root,
            hash,
            DemucsModel::Htdemucs,
            |ev| events.push(ev),
        )
        .expect("Neural HTDemucs separation on Stay");

        assert!(has_neural_stems_cache(&out_dir), "Expected cacheVersion: 3 manifest");
        assert_eq!(events.last().unwrap().status, StemStatus::Completed);

        let voc = karaok_audio::decode_audio_to_48khz_stereo(&out_dir.join("vocals.wav")).unwrap();
        let oth = karaok_audio::decode_audio_to_48khz_stereo(&out_dir.join("other.wav")).unwrap();

        let rms = |buf: &[f32], s0: usize, s1: usize| -> f32 {
            let slice = &buf[s0.min(buf.len())..s1.min(buf.len())];
            (slice.iter().map(|x| x * x).sum::<f32>() / slice.len().max(1) as f32).sqrt()
        };

        // 2s..10s = piano-only intro, 18s..35s = Rihanna lead vocal verse
        let intro_voc = rms(&voc.left, 48_000 * 2, 48_000 * 10);
        let verse_voc = rms(&voc.left, 48_000 * 18, 48_000 * 35);
        let verse_oth = rms(&oth.left, 48_000 * 18, 48_000 * 35);
        eprintln!(
            "Stay Neural Stems RMS -> intro_voc={intro_voc:.4}, verse_voc={verse_voc:.4}, verse_other={verse_oth:.4}"
        );
        assert!(intro_voc < 0.025, "Expected silent vocals during piano intro, got {intro_voc}");
        assert!(
            verse_voc > intro_voc * 4.0 && verse_voc > 0.05,
            "Expected isolated singing vocals in verse, got verse={verse_voc}, intro={intro_voc}"
        );
        assert!(verse_oth > 0.03, "Expected piano accompaniment preserved in other.wav, got {verse_oth}");
    }
}
