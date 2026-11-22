//! Domain types, Enhanced LRC lyrics engine, and musical theory utilities shared across
//! all Karaok Party crates.

pub mod error;
pub mod lyrics;
pub mod music;
pub mod types;

pub use error::{KaraokError, Result};
pub use lyrics::{
    ActiveLyricsWindow, LyricsDocument, LyricsFormat, LyricsLine, LyricsSource, LyricsWord,
};
pub use music::{freq_to_midi, midi_to_freq, midi_to_note_name, MusicalKey, PitchClass, ScaleType};
pub use types::*;
