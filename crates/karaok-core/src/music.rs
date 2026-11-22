use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::KaraokError;

/// The 12 Western chromatic pitch classes (`C = 0` .. `B = 11`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum PitchClass {
    C = 0,
    CSharp = 1,
    D = 2,
    DSharp = 3,
    E = 4,
    F = 5,
    FSharp = 6,
    G = 7,
    GSharp = 8,
    A = 9,
    ASharp = 10,
    B = 11,
}

impl PitchClass {
    pub const ALL: [PitchClass; 12] = [
        PitchClass::C,
        PitchClass::CSharp,
        PitchClass::D,
        PitchClass::DSharp,
        PitchClass::E,
        PitchClass::F,
        PitchClass::FSharp,
        PitchClass::G,
        PitchClass::GSharp,
        PitchClass::A,
        PitchClass::ASharp,
        PitchClass::B,
    ];

    /// Converts an arbitrary integer semitone index to a `PitchClass` via Euclidean modulo 12.
    #[inline]
    pub const fn from_semitone(semitone: i32) -> Self {
        let idx = semitone.rem_euclid(12) as usize;
        Self::ALL[idx]
    }

    /// Transposes this pitch class by `semitones` (may be negative or positive).
    #[inline]
    pub const fn transpose(self, semitones: i8) -> Self {
        Self::from_semitone(self as i32 + semitones as i32)
    }

    /// Human-readable sharp notation (`C`, `C#`, ..., `B`).
    #[inline]
    pub const fn label(self) -> &'static str {
        match self {
            Self::C => "C",
            Self::CSharp => "C#",
            Self::D => "D",
            Self::DSharp => "D#",
            Self::E => "E",
            Self::F => "F",
            Self::FSharp => "F#",
            Self::G => "G",
            Self::GSharp => "G#",
            Self::A => "A",
            Self::ASharp => "A#",
            Self::B => "B",
        }
    }
}

impl fmt::Display for PitchClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for PitchClass {
    type Err = KaraokError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_uppercase().as_str() {
            "C" => Ok(Self::C),
            "C#" | "DB" => Ok(Self::CSharp),
            "D" => Ok(Self::D),
            "D#" | "EB" => Ok(Self::DSharp),
            "E" => Ok(Self::E),
            "F" => Ok(Self::F),
            "F#" | "GB" => Ok(Self::FSharp),
            "G" => Ok(Self::G),
            "G#" | "AB" => Ok(Self::GSharp),
            "A" => Ok(Self::A),
            "A#" | "BB" => Ok(Self::ASharp),
            "B" => Ok(Self::B),
            other => Err(KaraokError::InvalidKey(other.to_string())),
        }
    }
}

/// Supported autotune scale modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ScaleType {
    #[default]
    Chromatic,
    Major,
    Minor,
}

impl ScaleType {
    /// Returns a 12-bit bitmask of allowed semitone offsets relative to the root (bit 0 = root).
    #[inline]
    pub const fn bitmask(self) -> u16 {
        match self {
            // All 12 semitones
            Self::Chromatic => 0b1111_1111_1111,
            // W-W-H-W-W-W-H: 0, 2, 4, 5, 7, 9, 11
            Self::Major => (1 << 0) | (1 << 2) | (1 << 4) | (1 << 5) | (1 << 7) | (1 << 9) | (1 << 11),
            // Natural minor: 0, 2, 3, 5, 7, 8, 10
            Self::Minor => (1 << 0) | (1 << 2) | (1 << 3) | (1 << 5) | (1 << 7) | (1 << 8) | (1 << 10),
        }
    }

    /// Returns whether `semitone_from_root` (`0..12`) belongs to this scale.
    #[inline]
    pub const fn contains(self, semitone_from_root: u8) -> bool {
        (self.bitmask() & (1 << (semitone_from_root % 12))) != 0
    }
}

/// Complete musical key (root pitch class + scale mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicalKey {
    pub root: PitchClass,
    pub scale: ScaleType,
}

impl Default for MusicalKey {
    fn default() -> Self {
        Self {
            root: PitchClass::C,
            scale: ScaleType::Major,
        }
    }
}

impl MusicalKey {
    #[inline]
    pub const fn new(root: PitchClass, scale: ScaleType) -> Self {
        Self { root, scale }
    }

    /// Transposes the key root while keeping the scale quality intact.
    #[inline]
    pub const fn transpose(self, semitones: i8) -> Self {
        Self {
            root: self.root.transpose(semitones),
            scale: self.scale,
        }
    }

    /// Quantizes a continuous MIDI note number (e.g. `69.32`) to the closest MIDI note
    /// belonging to `self` (`root` + `scale`).
    ///
    /// **Real-time safe**: zero allocation, O(1) bounded loop.
    #[inline]
    pub fn quantize_midi(self, midi_float: f32) -> f32 {
        if !midi_float.is_finite() {
            return 69.0;
        }
        let base = midi_float.floor() as i32;
        let root = self.root as i32;
        let mut best_midi = base as f32;
        let mut best_dist = f32::MAX;

        // Searching within [-2, +3] semitones is guaranteed to find the nearest degree
        // for Chromatic, Major, and Minor scales (max gap is 2 semitones).
        let mut candidate = base - 2;
        while candidate <= base + 3 {
            let rel = (candidate - root).rem_euclid(12) as u8;
            if self.scale.contains(rel) {
                let dist = (midi_float - candidate as f32).abs();
                if dist < best_dist {
                    best_dist = dist;
                    best_midi = candidate as f32;
                }
            }
            candidate += 1;
        }
        best_midi
    }

    /// Formats a musical key like `"C Major"`, `"F# Minor"`, `"A Chromatic"`.
    pub fn display_label(self) -> String {
        let suffix = match self.scale {
            ScaleType::Chromatic => "Chrom.",
            ScaleType::Major => "Maj",
            ScaleType::Minor => "Min",
        };
        format!("{} {}", self.root.label(), suffix)
    }
}

/// Converts frequency in Hz to continuous MIDI note number (`A4 = 440 Hz -> 69.0`).
#[inline]
pub fn freq_to_midi(hz: f32) -> f32 {
    if !hz.is_finite() || hz <= 1.0 {
        return 0.0;
    }
    69.0 + 12.0 * (hz / 440.0).log2()
}

/// Converts MIDI note number to frequency in Hz (`69.0 -> 440.0`).
#[inline]
pub fn midi_to_freq(midi: f32) -> f32 {
    if !midi.is_finite() {
        return 440.0;
    }
    440.0 * 2.0_f32.powf((midi - 69.0) / 12.0)
}

/// Formats an integer MIDI note to scientific pitch notation (e.g., `69 -> "A4"`, `60 -> "C4"`).
pub fn midi_to_note_name(midi: i32) -> String {
    let pc = PitchClass::from_semitone(midi);
    let octave = midi.div_euclid(12) - 1;
    format!("{}{}", pc.label(), octave)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transpose_wraps_around_octave() {
        let key = MusicalKey::new(PitchClass::A, ScaleType::Minor);
        assert_eq!(key.transpose(3), MusicalKey::new(PitchClass::C, ScaleType::Minor));
        assert_eq!(key.transpose(-6), MusicalKey::new(PitchClass::DSharp, ScaleType::Minor));
    }

    #[test]
    fn quantize_midi_respects_major_scale() {
        let c_maj = MusicalKey::new(PitchClass::C, ScaleType::Major);
        // C#4 (61.0) is equidistant from C4 (60) and D4 (62); slightly sharp snaps to D4 (62).
        assert_eq!(c_maj.quantize_midi(61.1), 62.0);
        // Slightly flat snaps to C4 (60).
        assert_eq!(c_maj.quantize_midi(60.9), 60.0);
        // E4 (64.0) is in C Major -> stays E4.
        assert_eq!(c_maj.quantize_midi(64.0), 64.0);
    }

    #[test]
    fn freq_midi_roundtrip() {
        assert!((freq_to_midi(440.0) - 69.0).abs() < 1e-5);
        assert!((midi_to_freq(69.0) - 440.0).abs() < 1e-4);
        assert_eq!(midi_to_note_name(69), "A4");
        assert_eq!(midi_to_note_name(60), "C4");
    }
}
