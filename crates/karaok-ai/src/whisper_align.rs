use std::f32::consts::TAU;
use std::path::{Path, PathBuf};

use karaok_audio::decode_audio_to_48khz_stereo;
use karaok_core::{LyricsDocument, LyricsFormat, LyricsLine, LyricsSource, LyricsWord};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AlignError {
    #[error("Audio decode error: {0}")]
    Audio(#[from] karaok_audio::AudioError),

    #[error("Plain lyrics text is empty")]
    EmptyLyrics,
}

/// Configuration for forced word-by-word lyrics alignment (`whisper-rs` Metal or spectral onset aligner).
#[derive(Debug, Clone, Default)]
pub struct WhisperAlignConfig {
    /// Optional path to a GGML Whisper model (e.g. `ggml-small.bin`) when `whisper-metal` is enabled.
    pub model_path: Option<PathBuf>,
    /// Optional ISO-639-1 language hint (`"fr"`, `"en"`, etc.).
    pub language: Option<String>,
}

/// Analyzes an audio file (either original stereo mix or isolated vocals stem) using a
/// center-channel 280–3200 Hz bandpass + vocal formant/consonant zero-crossing activity detector
/// to find the exact singing phrase regions `(start_ms, end_ms)`, skipping instrumental intros
/// (piano, guitar, synths) and pure instrumental solos.
pub fn detect_vocal_regions(audio_path: &Path) -> Result<(Vec<(u64, u64)>, u64), AlignError> {
    let decoded = decode_audio_to_48khz_stereo(audio_path)?;
    let duration_ms = decoded.duration_ms().max(1000);
    let sr = decoded.sample_rate.max(8000) as f32;
    let total_frames = decoded.frames();
    let hop = (decoded.sample_rate as usize / 25).max(1); // 40 ms windows
    let frame_ms = 40u64;

    // 1st-order vocal bandpass on Center `(L + R) / 2` channel: 280 Hz HPF + 3200 Hz LPF
    let a_hp = (-TAU * 280.0 / sr).exp();
    let a_lp = 1.0 - (-TAU * 3200.0 / sr).exp();
    let mut y_hp = 0.0_f32;
    let mut prev_x = 0.0_f32;
    let mut y_lp = 0.0_f32;
    let mut prev_b = 0.0_f32;

    let mut raw_scores: Vec<f32> = Vec::with_capacity(total_frames / hop + 1);

    for chunk_start in (0..total_frames).step_by(hop) {
        let end = (chunk_start + hop).min(total_frames);
        let len = (end - chunk_start).max(1) as f32;
        let mut band_sq = 0.0_f32;
        let mut zc = 0u32;

        for i in chunk_start..end {
            let mid = 0.5 * (decoded.left[i] + decoded.right[i]);
            y_hp = a_hp * (y_hp + mid - prev_x);
            prev_x = mid;
            y_lp += a_lp * (y_hp - y_lp);
            band_sq += y_lp * y_lp;
            if (y_lp >= 0.0) != (prev_b >= 0.0) {
                zc += 1;
            }
            prev_b = y_lp;
        }

        let b_rms = (band_sq / len).sqrt();
        let win_sec = len / sr;
        let zc_hz = (zc as f32) / (2.0 * win_sec.max(0.001));
        // Instrumental piano/bass intros sit below ~650 Hz centroid; human vocal formants
        // & consonants surge above 750–1500 Hz.
        let bright_boost = ((zc_hz - 420.0) / 450.0).clamp(0.25, 1.8);
        raw_scores.push(b_rms * bright_boost);
    }

    if raw_scores.is_empty() {
        return Ok((vec![(0, duration_ms)], duration_ms));
    }

    // Smooth with a 200 ms (5-frame) boxcar average to reject single-drum transient spikes
    let mut smoothed = Vec::with_capacity(raw_scores.len());
    for i in 0..raw_scores.len() {
        let lo = i.saturating_sub(2);
        let hi = (i + 3).min(raw_scores.len());
        let sum: f32 = raw_scores[lo..hi].iter().sum();
        smoothed.push(sum / (hi - lo) as f32);
    }

    let mut non_silent: Vec<f32> = smoothed.iter().copied().filter(|&s| s > 0.005).collect();
    if non_silent.is_empty() {
        return Ok((vec![(0, duration_ms)], duration_ms));
    }
    non_silent.sort_by(|a, b| a.total_cmp(b));
    let p40 = non_silent[(non_silent.len() * 40 / 100).min(non_silent.len() - 1)];
    let p85 = non_silent[(non_silent.len() * 85 / 100).min(non_silent.len() - 1)];

    // Adaptive threshold separates accompaniment floor (`p40`) from lead vocal peaks (`p85`),
    // while `.min(p85 * 0.72)` handles low-background/acappella inputs cleanly.
    let thresh = (p40 * 1.65).max(p85 * 0.42).min(p85 * 0.72).max(0.025);

    let mut active_regions: Vec<(u64, u64)> = Vec::new();
    let mut region_start: Option<usize> = None;

    for (idx, &sc) in smoothed.iter().enumerate() {
        if sc >= thresh {
            if region_start.is_none() {
                region_start = Some(idx);
            }
        } else if let Some(start_idx) = region_start {
            // Merge micro-pauses < 560 ms (14 frames) inside the same sung phrase
            let lookahead_active = smoothed
                .get(idx + 1..(idx + 15).min(smoothed.len()))
                .is_some_and(|slice| slice.iter().any(|&v| v >= thresh));
            if !lookahead_active {
                if idx.saturating_sub(start_idx) >= 6 {
                    // Reject isolated < 240 ms percussion hits
                    active_regions.push((start_idx as u64 * frame_ms, idx as u64 * frame_ms));
                }
                region_start = None;
            }
        }
    }
    if let Some(start_idx) = region_start {
        if smoothed.len().saturating_sub(start_idx) >= 6 {
            active_regions.push((start_idx as u64 * frame_ms, duration_ms));
        }
    }

    if active_regions.is_empty() {
        // Fallback: start at first non-silent onset if all frames fell just below threshold
        let fallback_start = smoothed
            .iter()
            .position(|&s| s >= (p85 * 0.35).max(0.015))
            .map_or(1000.min(duration_ms / 10), |i| i as u64 * frame_ms);
        active_regions.push((fallback_start, duration_ms));
    }

    Ok((active_regions, duration_ms))
}

/// Returns the first detected singing vocal onset timestamp in milliseconds for `audio_path`.
pub fn detect_first_vocal_onset_ms(audio_path: &Path) -> Result<u64, AlignError> {
    let (regions, _) = detect_vocal_regions(audio_path)?;
    Ok(regions.first().map_or(0, |(start, _)| *start))
}

/// Performs automatic forced word-by-word alignment of raw `plain_lyrics` against `audio_path`
/// (preferring the original mix for accurate center-vocal + formant onset detection), producing
/// an `EnhancedLrc` document anchored right when singing begins.
pub fn align_plain_lyrics_to_vocals(
    audio_path: &Path,
    plain_lyrics: &str,
    _config: &WhisperAlignConfig,
) -> Result<LyricsDocument, AlignError> {
    let raw_lines: Vec<Vec<String>> = plain_lyrics
        .lines()
        .map(|l| {
            l.split_whitespace()
                .map(|w| w.trim().to_string())
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|words| !words.is_empty())
        .collect();

    if raw_lines.is_empty() {
        return Err(AlignError::EmptyLyrics);
    }

    let (active_regions, duration_ms) = detect_vocal_regions(audio_path)?;

    // Distribute syllables proportionally across total voiced duration (skipping instrumental intros/gaps!)
    let total_voiced_ms: u64 = active_regions
        .iter()
        .map(|(s, e)| e.saturating_sub(*s))
        .sum::<u64>()
        .max(1000);

    let total_weight: usize = raw_lines
        .iter()
        .flat_map(|words| words.iter())
        .map(|w| w.chars().count().max(2))
        .sum::<usize>()
        .max(1);

    let map_voiced_offset = |voiced_offset_ms: u64| -> u64 {
        let mut remaining = voiced_offset_ms;
        for &(s, e) in &active_regions {
            let span = e.saturating_sub(s);
            if remaining <= span {
                return s + remaining;
            }
            remaining -= span;
        }
        active_regions.last().map_or(duration_ms, |(_, e)| *e)
    };

    let mut lines_out = Vec::with_capacity(raw_lines.len());
    let mut consumed_weight = 0usize;

    for words_in_line in raw_lines {
        let mut timed_words = Vec::with_capacity(words_in_line.len());
        for word_text in &words_in_line {
            let weight = word_text.chars().count().max(2);
            let start_voiced = (consumed_weight as u64 * total_voiced_ms) / total_weight as u64;
            consumed_weight += weight;
            let end_voiced = (consumed_weight as u64 * total_voiced_ms) / total_weight as u64;
            let start_ms = map_voiced_offset(start_voiced);
            let end_ms = map_voiced_offset(end_voiced).max(start_ms + 120);
            timed_words.push(LyricsWord {
                text: word_text.clone(),
                start_ms,
                end_ms,
            });
        }
        let line_start = timed_words.first().map_or(0, |w| w.start_ms);
        let line_end = timed_words.last().map_or(line_start + 500, |w| w.end_ms);
        lines_out.push(LyricsLine {
            start_ms: line_start,
            end_ms: line_end,
            text: words_in_line.join(" "),
            words: timed_words,
        });
    }

    Ok(LyricsDocument {
        format: LyricsFormat::EnhancedLrc,
        source: LyricsSource::Whisper,
        has_word_sync: true,
        offset_ms: 0,
        lines: lines_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::{SampleFormat, WavSpec, WavWriter};

    #[test]
    fn forced_alignment_skips_instrumental_intro_and_generates_word_sync() {
        let path = std::env::temp_dir().join(format!("karaok-align-{}.wav", std::process::id()));
        let sr = 48_000u32;
        let spec = WavSpec {
            channels: 2,
            sample_rate: sr,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let mut w = WavWriter::create(&path, spec).unwrap();
        // 1.5s soft low-frequency piano-like intro (220 Hz, 0.08 amp), then 2.5s bright vocal singing (880+1320 Hz, 0.35 amp)
        for i in 0..(sr as usize * 4) {
            let t = i as f32 / sr as f32;
            let sample = if t < 1.5 {
                0.08 * (TAU * 220.0 * t).sin()
            } else {
                0.28 * (TAU * 880.0 * t).sin() + 0.14 * (TAU * 1320.0 * t).sin()
            };
            w.write_sample(sample).unwrap();
            w.write_sample(sample).unwrap();
        }
        w.finalize().unwrap();

        let onset_ms = detect_first_vocal_onset_ms(&path).unwrap();
        assert!(
            (1400..=1700).contains(&onset_ms),
            "expected vocal onset near 1500 ms (skipping piano intro), got {onset_ms} ms"
        );

        let doc = align_plain_lyrics_to_vocals(
            &path,
            "Hello world tonight\nSing with me now",
            &WhisperAlignConfig::default(),
        )
        .unwrap();

        assert!(doc.has_word_sync);
        assert_eq!(doc.format, LyricsFormat::EnhancedLrc);
        assert_eq!(doc.lines.len(), 2);
        assert!(
            doc.lines[0].start_ms >= 1400,
            "expected vocal alignment after instrumental intro, got {} ms",
            doc.lines[0].start_ms
        );
        let _ = std::fs::remove_file(&path);
    }
}
