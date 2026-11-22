use signalsmith_stretch::Stretch;

/// Real-time stereo pitch transposition (`-6 .. +6` semitones) and time-stretching
/// (`70 % .. 130 %` tempo) powered by `signalsmith-stretch` with timbre preservation.
pub struct PitchTempoProcessor {
    stretch: Stretch,
    sample_rate: u32,
    current_semitones: f32,
    in_scratch: Vec<f32>,
    out_scratch: Vec<f32>,
}

impl PitchTempoProcessor {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000);
        let stretch = Stretch::preset_default(2, sr);
        Self {
            stretch,
            sample_rate: sr,
            current_semitones: 0.0,
            in_scratch: Vec::with_capacity(4096),
            out_scratch: Vec::with_capacity(4096),
        }
    }

    /// Sets key transposition in semitones (`-6.0 .. +6.0`).
    pub fn set_semitones(&mut self, semitones: f32) {
        let clamped = semitones.clamp(-6.0, 6.0);
        if (clamped - self.current_semitones).abs() > 0.001 {
            self.stretch.set_transpose_factor_semitones(clamped, None);
            self.current_semitones = clamped;
        }
    }

    pub fn reset(&mut self) {
        self.stretch.reset();
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Processes a stereo planar input block (`in_l`, `in_r`) into `output_frames` interleaved
    /// stereo output samples `(L0, R0, L1, R1, ...)`.
    ///
    /// Tempo ratio is controlled naturally by choosing `in_l.len() != output_frames`:
    /// e.g., `tempo_pct = 80%` feeds `0.8 * output_frames` input frames into `output_frames` output.
    pub fn process_stereo(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        output_frames: usize,
        out_interleaved: &mut Vec<f32>,
    ) {
        let in_frames = in_l.len().min(in_r.len());
        out_interleaved.clear();
        if in_frames == 0 || output_frames == 0 {
            return;
        }

        // Fast bypass when pitch == 0 and tempo == 100%
        if self.current_semitones.abs() < 0.001 && in_frames == output_frames {
            out_interleaved.reserve(output_frames * 2);
            for i in 0..output_frames {
                out_interleaved.push(in_l[i]);
                out_interleaved.push(in_r[i]);
            }
            return;
        }

        self.in_scratch.clear();
        self.in_scratch.reserve(in_frames * 2);
        for i in 0..in_frames {
            self.in_scratch.push(in_l[i]);
            self.in_scratch.push(in_r[i]);
        }

        self.out_scratch.resize(output_frames * 2, 0.0);
        self.stretch.process(&self.in_scratch, &mut self.out_scratch);

        out_interleaved.extend_from_slice(&self.out_scratch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn transposes_and_time_stretches_without_nan() {
        let mut proc = PitchTempoProcessor::new(48_000);
        proc.set_semitones(3.0);

        let in_frames = 512;
        let out_frames = 640; // 80% tempo slowdown
        let in_l: Vec<f32> = (0..in_frames)
            .map(|i| 0.4 * (TAU * 440.0 * i as f32 / 48_000.0).sin())
            .collect();
        let in_r = in_l.clone();
        let mut out = Vec::new();

        for _ in 0..8 {
            proc.process_stereo(&in_l, &in_r, out_frames, &mut out);
            assert_eq!(out.len(), out_frames * 2);
            assert!(out.iter().all(|s| s.is_finite()));
        }
    }
}
