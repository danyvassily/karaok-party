use crate::{db_to_linear, linear_to_db, sanitize_sample};

/// Intelligent RMS Noise Gate with hysteresis and hold time for anti-Larsen speaker rejection.
#[derive(Debug, Clone)]
pub struct NoiseGate {
    sample_rate: f32,
    rms_sq: f32,
    rms_coeff: f32,
    gain: f32,
    attack_coeff: f32,
    release_coeff: f32,
    hold_samples: u32,
    hold_counter: u32,
    is_open: bool,
}

impl NoiseGate {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(8000.0);
        Self {
            sample_rate: sr,
            rms_sq: 0.0,
            rms_coeff: (-1.0 / (0.008 * sr)).exp(),
            gain: 0.0,
            attack_coeff: (-1.0 / (0.002 * sr)).exp(),
            release_coeff: (-1.0 / (0.080 * sr)).exp(),
            hold_samples: (0.045 * sr) as u32,
            hold_counter: 0,
            is_open: false,
        }
    }

    /// Processes one sample with the given opening threshold in dBFS (hysteresis = 4 dB).
    #[inline]
    pub fn process(&mut self, input: f32, threshold_db: f32) -> f32 {
        let x = sanitize_sample(input);
        self.rms_sq = self.rms_coeff * self.rms_sq + (1.0 - self.rms_coeff) * (x * x);
        let level_db = linear_to_db(self.rms_sq.sqrt());

        let open_thresh = threshold_db.clamp(-75.0, -5.0);
        let close_thresh = open_thresh - 4.0;

        if level_db >= open_thresh {
            self.is_open = true;
            self.hold_counter = self.hold_samples;
        } else if self.is_open {
            if self.hold_counter > 0 {
                self.hold_counter -= 1;
            } else if level_db < close_thresh {
                self.is_open = false;
            }
        }

        let target_gain = if self.is_open { 1.0 } else { 0.08 }; // Gentle -22dB floor keeps quiet vocals natural
        let coeff = if target_gain > self.gain {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        self.gain = sanitize_sample(coeff * self.gain + (1.0 - coeff) * target_gain);
        x * self.gain
    }

    #[inline]
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }
}

/// Smooth soft-knee vocal dynamic compressor (default 3:1 ratio, fast attack, musical release).
#[derive(Debug, Clone)]
pub struct Compressor {
    envelope_db: f32,
    attack_coeff: f32,
    release_coeff: f32,
    knee_width_db: f32,
}

impl Compressor {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(8000.0);
        Self {
            envelope_db: -96.0,
            attack_coeff: (-1.0 / (0.005 * sr)).exp(),
            release_coeff: (-1.0 / (0.090 * sr)).exp(),
            knee_width_db: 6.0,
        }
    }

    #[inline]
    pub fn process(&mut self, input: f32, threshold_db: f32, ratio: f32) -> f32 {
        let x = sanitize_sample(input);
        let in_db = linear_to_db(x);

        let coeff = if in_db > self.envelope_db {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        self.envelope_db = coeff * self.envelope_db + (1.0 - coeff) * in_db;

        let r = ratio.clamp(1.0, 12.0);
        let thresh = threshold_db.clamp(-48.0, 0.0);
        let half_knee = self.knee_width_db * 0.5;
        let diff = self.envelope_db - thresh;

        let gain_reduction_db = if diff <= -half_knee {
            0.0
        } else if diff >= half_knee {
            diff * (1.0 - 1.0 / r)
        } else {
            // Quadratic soft knee interpolation
            let knee_pos = diff + half_knee;
            ((1.0 - 1.0 / r) * knee_pos * knee_pos) / (2.0 * self.knee_width_db)
        };

        // Gentle automatic makeup gain so vocals stay prominent
        let makeup_db = ((-thresh) * (1.0 - 1.0 / r) * 0.25).clamp(0.0, 6.0);
        let linear_gain = db_to_linear(makeup_db - gain_reduction_db);
        sanitize_sample(x * linear_gain)
    }
}
