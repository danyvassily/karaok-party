//! Lock-free CoreAudio real-time audio engine for Karaok Party.
//!
//! Architecture:
//! - Background feeder thread decodes / mixes 4 (or 6) stems, applies real-time pitch & tempo
//!   transposition via `signalsmith-stretch`, and pushes stereo 48 kHz frames into a lock-free
//!   SPSC `ringbuf`.
//! - CoreAudio (`cpal`) real-time callback reads the stems `ringbuf`, pulls Mic 1 & Mic 2 samples
//!   from lock-free SPSC `ringbuf`s, runs independent `VocalDspChain`s, applies latency
//!   calibration delay, soft-limits the master bus, and taps a non-blocking recording `ringbuf`.
//! - Zero heap allocations, zero mutex locks, and zero panics in the CoreAudio callback.

pub mod atomics;
pub mod decoder;
pub mod engine;
pub mod error;
pub mod recorder;
pub mod stretcher;

pub use atomics::AtomicEngineState;
pub use decoder::{decode_audio_to_48khz_stereo, DecodedAudio};
pub use engine::{AudioEngine, MicAudioProducer, MASTER_SAMPLE_RATE};
pub use error::{AudioError, AudioResult};
pub use recorder::{mix_and_export_recording, RecordingHandle};
pub use stretcher::PitchTempoProcessor;
