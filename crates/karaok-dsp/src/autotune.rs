use std::f32::consts::PI;

use karaok_core::{midi_to_freq, MusicalKey};

use crate::pitch::PitchEstimate;
use crate::sanitize_sample;

/// Real-time low-latency vocal pitch correction (Autotune) locked to the active musical scale.
///
/// Uses a pre-allocated circular delay buffer with dual Hann-windowed fractional read heads
/// (PSOLA-style phase-continuous granular shifter) so pitch correction introduces < 15 ms latency
/// with zero heap allocations and zero zipper noise.
pub struct AutotuneProcessor {
    sample_rate: f32,
    delay_buf: Vec<f32>,
    buf_mask: usize,
    write_idx: usize,
    phase: f32,
    window_samples: f32,
    current_ratio: f32,
    target_cents_error: f32,
}

impl AutotuneProcessor {
    const BUFFER_LEN: usize = 4096; // Power of 2 for bitmask wrapping

    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(8000.0);
        Self {
            sample_rate: sr,
            delay_buf: vec![0.0; Self::BUFFER_LEN],
            buf_mask: Self::BUFFER_LEN - 1,
            write_idx: 0,
            phase: 0.0,
            // ~12 ms crossfade grain keeps vocal pitch correction crisp with ~6 ms avg latency
            window_samples: (0.012 * sr).clamp(192.0, 1024.0),
            current_ratio: 1.0,
            target_cents_error: 0.0,
        }
    }

    /// Processes a single mono sample, correcting pitch toward the nearest note in `active_key`.
    ///
    /// - `retune_speed_ms`: `0.0` = instant hard-tune (T-Pain / Pop Star), `120.0` = smooth natural.
    /// - `dry_wet`: `0.0` = bypass, `1.0` = 100 % corrected signal.
    #[inline]
    pub fn process(
        &mut self,
        input: f32,
        estimate: PitchEstimate,
        active_key: MusicalKey,
        enabled: bool,
        retune_speed_ms: f32,
        dry_wet: f32,
    ) -> f32 {
        let x = sanitize_sample(input);

        // Always keep delay line warm so toggling autotune causes zero discontinuity
        if let Some(slot) = self.delay_buf.get_mut(self.write_idx) {
            *slot = x;
        }
        let current_write = self.write_idx;
        self.write_idx = (self.write_idx + 1) & self.buf_mask;

        // Always compute scale cents error when voiced so the live pitch gauge works
        // even in Natural preset (when autotune pitch correction is disabled).
        let target_ratio = if estimate.confidence > 0.72 && estimate.frequency_hz > 75.0 {
            let quantized_midi = active_key.quantize_midi(estimate.midi_note);
            self.target_cents_error =
                ((estimate.midi_note - quantized_midi) * 100.0).clamp(-50.0, 50.0);
            if enabled {
                let target_hz = midi_to_freq(quantized_midi);
                // Clamp correction to +-3 semitones to prevent octave-jump artifacts
                (target_hz / estimate.frequency_hz).clamp(0.84, 1.19)
            } else {
                1.0
            }
        } else {
            self.target_cents_error = 0.0;
            1.0
        };

        // Exponential smoothing controlled by retune_speed_ms
        let speed_sec = (retune_speed_ms.clamp(0.0, 250.0) / 1000.0).max(0.0015);
        let coeff = (-1.0 / (speed_sec * self.sample_rate)).exp();
        self.current_ratio = coeff * self.current_ratio + (1.0 - coeff) * target_ratio;

        if !enabled || dry_wet <= 0.001 {
            return x;
        }

        // Advance dual-tap sawtooth phase according to (1 - pitch_ratio)
        let phase_inc = (1.0 - self.current_ratio) / self.window_samples;
        self.phase += phase_inc;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        } else if self.phase < 0.0 {
            self.phase += 1.0;
        }

        let phase_a = self.phase;
        let phase_b = if phase_a >= 0.5 {
            phase_a - 0.5
        } else {
            phase_a + 0.5
        };

        // Complementary Hann windows: w(p) + w(p + 0.5) == 1.0 everywhere
        let win_a = 0.5 * (1.0 - (2.0 * PI * phase_a).cos());
        let win_b = 0.5 * (1.0 - (2.0 * PI * phase_b).cos());

        let delay_a = 16.0 + phase_a * self.window_samples;
        let delay_b = 16.0 + phase_b * self.window_samples;

        let tap_a = self.read_interpolated(current_write, delay_a);
        let tap_b = self.read_interpolated(current_write, delay_b);
        let wet = sanitize_sample(tap_a * win_a + tap_b * win_b);

        let mix = dry_wet.clamp(0.0, 1.0);
        sanitize_sample(x * (1.0 - mix) + wet * mix)
    }

    #[inline]
    pub fn last_cents_error(&self) -> f32 {
        self.target_cents_error
    }

    #[inline]
    pub fn current_ratio(&self) -> f32 {
        self.current_ratio
    }

    #[inline]
    fn read_interpolated(&self, write_idx: usize, delay_samples: f32) -> f32 {
        let len = Self::BUFFER_LEN as f32;
        let mut read_pos = write_idx as f32 - delay_samples;
        if read_pos < 0.0 {
            read_pos += len;
        }
        let idx0 = (read_pos as usize) & self.buf_mask;
        let idx1 = (idx0 + 1) & self.buf_mask;
        let frac = read_pos - read_pos.floor();
        let s0 = self.delay_buf.get(idx0).copied().unwrap_or(0.0);
        let s1 = self.delay_buf.get(idx1).copied().unwrap_or(0.0);
        s0 + (s1 - s0) * frac
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use karaok_core::{freq_to_midi, PitchClass, ScaleType};

    #[test]
    fn autotune_pulls_flat_a4_upward_in_c_major() {
        let sr = 48_000.0;
        let mut tuner = AutotuneProcessor::new(sr);
        let key = MusicalKey::new(PitchClass::C, ScaleType::Major);
        let est = PitchEstimate {
            frequency_hz: 430.0, // ~40 cents flat of A4 (440 Hz)
            confidence: 0.96,
            midi_note: freq_to_midi(430.0),
        };

        for i in 0..2048 {
            let s = (i as f32 * 0.05).sin() * 0.5;
            let out = tuner.process(s, est, key, true, 10.0, 1.0);
            assert!(out.is_finite());
        }
        // Ratio must be > 1.0 (pulling 430 Hz up toward 440 Hz)
        assert!(
            tuner.current_ratio() > 1.015,
            "expected upward pitch ratio > 1.015, got {}",
            tuner.current_ratio()
        );
    }

    #[test]
    fn reports_pitch_cents_error_even_when_autotune_disabled() {
        let mut tuner = AutotuneProcessor::new(48_000.0);
        let key = MusicalKey::new(PitchClass::C, ScaleType::Major);
        let est = PitchEstimate {
            frequency_hz: 430.0, // ~40 cents flat of A4 (440 Hz)
            confidence: 0.95,
            midi_note: freq_to_midi(430.0),
        };
        let out = tuner.process(0.3, est, key, false, 120.0, 0.0);
        assert!((out - 0.3).abs() < 1e-6);
        assert!(
            tuner.last_cents_error() < -30.0,
            "expected negative cents error when singing flat with autotune off, got {}",
            tuner.last_cents_error()
        );
    }
}
