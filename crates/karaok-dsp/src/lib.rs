//! Real-time, zero-allocation vocal DSP chain for Karaok Party.
//!
//! All structs in this crate pre-allocate every buffer during construction (`new()`).
//! None of the `process*` methods allocate memory, lock mutexes, perform I/O, or panic.

pub mod autotune;
pub mod biquad;
pub mod chain;
pub mod dynamics;
pub mod jitter;
pub mod pitch;
pub mod reverb;

pub use autotune::AutotuneProcessor;
pub use biquad::BiquadFilter;
pub use chain::{VocalDspChain, VocalTelemetrySnapshot};
pub use dynamics::{Compressor, NoiseGate};
pub use jitter::{AdaptiveJitterBuffer, JitterStats};
pub use pitch::{PitchEstimate, YinPitchDetector};
pub use reverb::StereoReverb;

/// Sanitizes an audio sample against NaN/Inf and flushes denormals to zero.
#[inline]
pub fn sanitize_sample(x: f32) -> f32 {
    if !x.is_finite() || x.abs() < 1e-24 {
        0.0
    } else {
        x
    }
}

/// Converts linear amplitude to decibels (`-96.0` floor).
#[inline]
pub fn linear_to_db(linear: f32) -> f32 {
    let clamped = linear.abs().max(1e-5);
    (20.0 * clamped.log10()).clamp(-96.0, 24.0)
}

/// Converts decibels to linear amplitude.
#[inline]
pub fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Smooth soft-clipper preventing harsh digital clipping on the master output bus.
#[inline]
pub fn soft_limit(x: f32) -> f32 {
    let s = sanitize_sample(x);
    if s.abs() <= 0.85 {
        s
    } else {
        let sign = s.signum();
        let excess = s.abs() - 0.85;
        sign * (0.85 + 0.15 * (excess / 0.15).tanh())
    }
}
