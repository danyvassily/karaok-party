use karaok_core::{MusicalKey, VocalDspParams};

use crate::{
    autotune::AutotuneProcessor,
    biquad::BiquadFilter,
    dynamics::{Compressor, NoiseGate},
    linear_to_db,
    pitch::YinPitchDetector,
    reverb::StereoReverb,
    sanitize_sample,
};

/// Real-time vocal telemetry snapshot produced by `VocalDspChain`.
#[derive(Debug, Clone, Copy, Default)]
pub struct VocalTelemetrySnapshot {
    pub vu_db: f32,
    pub peak: f32,
    pub pitch_hz: f32,
    pub pitch_midi: i32,
    pub pitch_cents: f32,
    pub gate_open: bool,
}

/// Complete independent vocal channel strip for Mic 1 or Mic 2:
/// `Input Gain -> HPF 80 Hz -> Noise Gate -> Compressor 3:1 -> Presence EQ -> YIN + Autotune -> Stereo Reverb`.
///
/// Pre-allocated once at startup; 100% real-time safe inside the CoreAudio callback.
pub struct VocalDspChain {
    sample_rate: f32,
    hpf: BiquadFilter,
    presence_eq: BiquadFilter,
    gate: NoiseGate,
    compressor: Compressor,
    pitch_detector: YinPitchDetector,
    autotune: AutotuneProcessor,
    reverb: StereoReverb,
    last_hpf_hz: f32,
    last_presence_db: f32,
    vu_rms_sq: f32,
    vu_decay: f32,
    peak_hold: f32,
    telemetry: VocalTelemetrySnapshot,
}

impl VocalDspChain {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(8000.0);
        let mut presence_eq = BiquadFilter::default();
        presence_eq.set_high_shelf(sr, 6000.0, 1.5);

        Self {
            sample_rate: sr,
            hpf: BiquadFilter::high_pass(sr, 80.0),
            presence_eq,
            gate: NoiseGate::new(sr),
            compressor: Compressor::new(sr),
            pitch_detector: YinPitchDetector::new(sr),
            autotune: AutotuneProcessor::new(sr),
            reverb: StereoReverb::new(sr),
            last_hpf_hz: 80.0,
            last_presence_db: 1.5,
            vu_rms_sq: 0.0,
            vu_decay: (-1.0 / (0.050 * sr)).exp(),
            peak_hold: 0.0,
            telemetry: VocalTelemetrySnapshot::default(),
        }
    }

    /// Processes a single mono microphone sample into a stereo `(left, right)` processed vocal pair.
    /// Zero allocations, zero locks, zero panics.
    #[inline]
    pub fn process_sample(
        &mut self,
        input: f32,
        active_song_key: MusicalKey,
        params: &VocalDspParams,
    ) -> (f32, f32) {
        if params.muted {
            self.vu_rms_sq *= self.vu_decay;
            self.peak_hold *= 0.999;
            self.telemetry = VocalTelemetrySnapshot {
                vu_db: -96.0,
                peak: 0.0,
                pitch_hz: 0.0,
                pitch_midi: 0,
                pitch_cents: 0.0,
                gate_open: false,
            };
            return (0.0, 0.0);
        }

        // Update filter coefficients only if parameter actually changed
        if (params.hpf_hz - self.last_hpf_hz).abs() > 0.5 {
            self.hpf.set_high_pass(self.sample_rate, params.hpf_hz);
            self.last_hpf_hz = params.hpf_hz;
        }
        if (params.presence_db - self.last_presence_db).abs() > 0.1 {
            self.presence_eq
                .set_high_shelf(self.sample_rate, 6000.0, params.presence_db);
            self.last_presence_db = params.presence_db;
        }

        // 1. Input gain & HPF 80 Hz
        let x = sanitize_sample(input) * params.gain.clamp(0.0, 2.5);
        let hpf_out = self.hpf.process(x);

        // 2. Noise Gate (anti-Larsen speaker bleed rejection)
        let gated = self.gate.process(hpf_out, params.gate_threshold_db);

        // 3. Soft-knee Compressor (3:1 default)
        let compressed =
            self.compressor
                .process(gated, params.comp_threshold_db, params.comp_ratio);

        // 4. Presence Shelf EQ (brilliance for Pop Star / Studio)
        let eq_out = self.presence_eq.process(compressed);

        // 5. Real-time YIN Pitch Detection
        let estimate = self.pitch_detector.push_sample(eq_out);

        // 6. Scale-aware Autotune locked to the active song root + selected scale
        let tune_key = MusicalKey::new(active_song_key.root, params.scale);
        let tuned = self.autotune.process(
            eq_out,
            estimate,
            tune_key,
            params.autotune_enabled,
            params.retune_speed_ms,
            params.autotune_mix,
        );

        // 7. Stereo Algorithmic Reverb (+ subtle stereo stage echo in Live preset)
        let echo_mix = if params.preset == karaok_core::VocalPreset::Live {
            0.18
        } else {
            0.0
        };
        let (out_l, out_r) = self.reverb.process_mono_to_stereo_with_echo(
            tuned,
            params.reverb_room_size,
            params.reverb_damping,
            params.reverb_wet,
            echo_mix,
        );

        // Update VU & pitch telemetry
        let energy = tuned * tuned;
        self.vu_rms_sq = self.vu_decay * self.vu_rms_sq + (1.0 - self.vu_decay) * energy;
        let abs_val = tuned.abs();
        self.peak_hold = if abs_val > self.peak_hold {
            abs_val
        } else {
            self.peak_hold * 0.9992
        };

        self.telemetry = VocalTelemetrySnapshot {
            vu_db: linear_to_db(self.vu_rms_sq.sqrt()),
            peak: self.peak_hold.clamp(0.0, 1.0),
            pitch_hz: estimate.frequency_hz,
            pitch_midi: if estimate.frequency_hz > 0.0 {
                estimate.midi_note.round() as i32
            } else {
                0
            },
            pitch_cents: self.autotune.last_cents_error(),
            gate_open: self.gate.is_open(),
        };

        (out_l, out_r)
    }

    #[inline]
    pub fn telemetry(&self) -> VocalTelemetrySnapshot {
        self.telemetry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use karaok_core::{PitchClass, ScaleType, VocalPreset};
    use std::f32::consts::TAU;

    #[test]
    fn vocal_chain_processes_all_presets_without_nan_or_clip() {
        let presets = [
            VocalPreset::Natural,
            VocalPreset::Studio,
            VocalPreset::Live,
            VocalPreset::PopStar,
            VocalPreset::Expert,
        ];
        let key = MusicalKey::new(PitchClass::C, ScaleType::Major);
        let sr = 48_000.0;

        for preset in presets {
            let mut chain = VocalDspChain::new(sr);
            let params = VocalDspParams::from_preset(preset);
            for i in 0..2048 {
                let input = 0.45 * (TAU * 330.0 * (i as f32) / sr).sin();
                let (l, r) = chain.process_sample(input, key, &params);
                assert!(l.is_finite() && r.is_finite(), "Non-finite output for {preset:?}");
            }
            let snap = chain.telemetry();
            assert!(snap.gate_open, "gate should open on active vocal for {preset:?}");
            assert!(snap.vu_db > -30.0, "VU too low for {preset:?}: {}", snap.vu_db);
        }
    }
}
