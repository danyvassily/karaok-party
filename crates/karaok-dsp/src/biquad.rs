use std::f32::consts::PI;

use crate::sanitize_sample;

/// Direct Form II Transposed 2nd-order IIR biquad filter.
/// Real-time safe: fixed stack storage, zero allocation.
#[derive(Debug, Clone, Copy)]
pub struct BiquadFilter {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Default for BiquadFilter {
    fn default() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }
}

impl BiquadFilter {
    /// Creates a 2nd-order Butterworth high-pass filter (`Q = 0.7071`).
    pub fn high_pass(sample_rate: f32, cutoff_hz: f32) -> Self {
        let mut f = Self::default();
        f.set_high_pass(sample_rate, cutoff_hz);
        f
    }

    /// Recomputes Butterworth high-pass coefficients without resetting state.
    pub fn set_high_pass(&mut self, sample_rate: f32, cutoff_hz: f32) {
        let sr = sample_rate.max(8000.0);
        let fc = cutoff_hz.clamp(20.0, sr * 0.45);
        let w0 = 2.0 * PI * fc / sr;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * std::f32::consts::FRAC_1_SQRT_2);

        let a0 = 1.0 + alpha;
        self.b0 = ((1.0 + cos_w0) * 0.5) / a0;
        self.b1 = -(1.0 + cos_w0) / a0;
        self.b2 = ((1.0 + cos_w0) * 0.5) / a0;
        self.a1 = (-2.0 * cos_w0) / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    /// Recomputes high-shelf EQ coefficients (used for vocal brilliance / presence).
    pub fn set_high_shelf(&mut self, sample_rate: f32, freq_hz: f32, gain_db: f32) {
        let sr = sample_rate.max(8000.0);
        let fc = freq_hz.clamp(1000.0, sr * 0.45);
        let db = gain_db.clamp(-12.0, 12.0);
        let a = 10.0_f32.powf(db / 40.0);
        let w0 = 2.0 * PI * fc / sr;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 * 0.5 * std::f32::consts::SQRT_2;
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

        let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
        self.b0 = (a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha)) / a0;
        self.b1 = (-2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0)) / a0;
        self.b2 = (a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha)) / a0;
        self.a1 = (2.0 * ((a - 1.0) - (a + 1.0) * cos_w0)) / a0;
        self.a2 = ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha) / a0;
    }

    /// Processes a single audio sample in-place. Zero allocations.
    #[inline]
    pub fn process(&mut self, input: f32) -> f32 {
        let x = sanitize_sample(input);
        let y = self.b0 * x + self.z1;
        self.z1 = sanitize_sample(self.b1 * x - self.a1 * y + self.z2);
        self.z2 = sanitize_sample(self.b2 * x - self.a2 * y);
        sanitize_sample(y)
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}
