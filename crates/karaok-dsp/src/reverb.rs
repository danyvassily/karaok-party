use crate::sanitize_sample;

/// Tuned delay lengths (in samples at 44.1 kHz, scaled dynamically to target sample rate)
/// derived from Schroeder / Jezar Freeverb architecture for lush, metallic-free vocal tails.
const COMB_TUNINGS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNINGS: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;

struct CombFilter {
    buffer: Vec<f32>,
    idx: usize,
    filter_store: f32,
}

impl CombFilter {
    fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size.max(1)],
            idx: 0,
            filter_store: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, input: f32, feedback: f32, damp1: f32, damp2: f32) -> f32 {
        let len = self.buffer.len();
        let output = self.buffer.get(self.idx).copied().unwrap_or(0.0);
        self.filter_store = sanitize_sample(output * damp2 + self.filter_store * damp1);
        if let Some(slot) = self.buffer.get_mut(self.idx) {
            *slot = sanitize_sample(input + self.filter_store * feedback);
        }
        self.idx += 1;
        if self.idx >= len {
            self.idx = 0;
        }
        output
    }
}

struct AllpassFilter {
    buffer: Vec<f32>,
    idx: usize,
}

impl AllpassFilter {
    fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size.max(1)],
            idx: 0,
        }
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let len = self.buffer.len();
        let buf_out = self.buffer.get(self.idx).copied().unwrap_or(0.0);
        let output = sanitize_sample(-input + buf_out);
        if let Some(slot) = self.buffer.get_mut(self.idx) {
            *slot = sanitize_sample(input + buf_out * 0.5);
        }
        self.idx += 1;
        if self.idx >= len {
            self.idx = 0;
        }
        output
    }
}

/// Pure-Rust real-time stereo algorithmic plate/room reverb with optional subtle stereo echo
/// (used by the `Live` stage preset). Pre-allocates all delay lines once during `new()`.
pub struct StereoReverb {
    combs_l: [CombFilter; 8],
    combs_r: [CombFilter; 8],
    allpasses_l: [AllpassFilter; 4],
    allpasses_r: [AllpassFilter; 4],
    echo_l: Vec<f32>,
    echo_r: Vec<f32>,
    echo_idx_l: usize,
    echo_idx_r: usize,
}

impl StereoReverb {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(8000.0);
        let scale = (sr / 44_100.0).clamp(0.5, 4.0);
        let scaled = |n: usize| ((n as f32 * scale).round() as usize).max(16);
        // Subtle stereo slapback echo: 165 ms (L) and 215 ms (R)
        let echo_len_l = (0.165 * sr) as usize;
        let echo_len_r = (0.215 * sr) as usize;

        Self {
            combs_l: COMB_TUNINGS.map(|n| CombFilter::new(scaled(n))),
            combs_r: COMB_TUNINGS.map(|n| CombFilter::new(scaled(n + STEREO_SPREAD))),
            allpasses_l: ALLPASS_TUNINGS.map(|n| AllpassFilter::new(scaled(n))),
            allpasses_r: ALLPASS_TUNINGS.map(|n| AllpassFilter::new(scaled(n + STEREO_SPREAD))),
            echo_l: vec![0.0; echo_len_l.max(64)],
            echo_r: vec![0.0; echo_len_r.max(64)],
            echo_idx_l: 0,
            echo_idx_r: 0,
        }
    }

    /// Processes a mono vocal sample into stereo `(left, right)` with controllable
    /// `room_size` (`0..1`), `damping` (`0..1`), `wet` (`0..1`), and optional `echo_mix` (`0..0.4`).
    #[inline]
    pub fn process_mono_to_stereo(
        &mut self,
        input: f32,
        room_size: f32,
        damping: f32,
        wet: f32,
    ) -> (f32, f32) {
        self.process_mono_to_stereo_with_echo(input, room_size, damping, wet, 0.0)
    }

    /// Processes mono vocal into stereo with optional subtle ping-pong stage echo.
    #[inline]
    pub fn process_mono_to_stereo_with_echo(
        &mut self,
        input: f32,
        room_size: f32,
        damping: f32,
        wet: f32,
        echo_mix: f32,
    ) -> (f32, f32) {
        let dry = sanitize_sample(input);

        // Stereo echo tap (always ticked so toggling Live preset has zero stale buffer bursts)
        let tap_l = self.echo_l.get(self.echo_idx_l).copied().unwrap_or(0.0);
        let tap_r = self.echo_r.get(self.echo_idx_r).copied().unwrap_or(0.0);
        if let Some(slot) = self.echo_l.get_mut(self.echo_idx_l) {
            *slot = sanitize_sample(dry + tap_r * 0.24);
        }
        if let Some(slot) = self.echo_r.get_mut(self.echo_idx_r) {
            *slot = sanitize_sample(dry * 0.75 + tap_l * 0.24);
        }
        self.echo_idx_l += 1;
        if self.echo_idx_l >= self.echo_l.len() {
            self.echo_idx_l = 0;
        }
        self.echo_idx_r += 1;
        if self.echo_idx_r >= self.echo_r.len() {
            self.echo_idx_r = 0;
        }
        let e_mix = echo_mix.clamp(0.0, 0.45);

        let wet_gain = wet.clamp(0.0, 1.0);
        if wet_gain <= 0.0005 && e_mix <= 0.0005 {
            return (dry, dry);
        }

        // Studio-calibrated input scaling for warm, clearly audible plate/hall tail
        let reverb_in = dry * 0.055;
        let feedback = 0.74 + room_size.clamp(0.0, 1.0) * 0.22;
        let damp1 = damping.clamp(0.0, 1.0) * 0.45;
        let damp2 = 1.0 - damp1;

        let mut out_l = 0.0_f32;
        let mut out_r = 0.0_f32;
        for i in 0..8 {
            out_l += self.combs_l[i].process(reverb_in, feedback, damp1, damp2);
            out_r += self.combs_r[i].process(reverb_in, feedback, damp1, damp2);
        }
        for i in 0..4 {
            out_l = self.allpasses_l[i].process(out_l);
            out_r = self.allpasses_r[i].process(out_r);
        }

        // Lush stereo width blend
        let wet_l = out_l * 0.85 + out_r * 0.15;
        let wet_r = out_r * 0.85 + out_l * 0.15;
        let dry_gain = 1.0 - wet_gain * 0.25;

        (
            sanitize_sample(dry * dry_gain + wet_l * wet_gain * 2.4 + tap_l * e_mix),
            sanitize_sample(dry * dry_gain + wet_r * wet_gain * 2.4 + tap_r * e_mix),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverb_produces_stereo_decaying_tail() {
        let mut rev = StereoReverb::new(48_000.0);
        let _ = rev.process_mono_to_stereo(1.0, 0.7, 0.4, 0.5);

        let mut tail_energy_l = 0.0_f32;
        let mut tail_energy_r = 0.0_f32;
        let mut stereo_diff = 0.0_f32;
        for _ in 0..4000 {
            let (l, r) = rev.process_mono_to_stereo(0.0, 0.7, 0.4, 0.5);
            assert!(l.is_finite() && r.is_finite());
            tail_energy_l += l * l;
            tail_energy_r += r * r;
            stereo_diff += (l - r).abs();
        }
        assert!(tail_energy_l > 1e-4 && tail_energy_r > 1e-4);
        assert!(stereo_diff > 1e-3, "expected stereo decorrelation");
    }
}
