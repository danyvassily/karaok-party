use serde::{Deserialize, Serialize};

use crate::music::{MusicalKey, ScaleType};

/// Supported CoreAudio buffer sizes (frames at 48 kHz).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AudioBufferSize {
    Frames64 = 64,
    #[default]
    Frames128 = 128,
    Frames256 = 256,
}

impl AudioBufferSize {
    pub const fn as_u32(self) -> u32 {
        self as u32
    }

    pub const fn from_u32(val: u32) -> Self {
        match val {
            64 => Self::Frames64,
            256 => Self::Frames256,
            _ => Self::Frames128,
        }
    }
}

/// Identifier for the two independent wireless iPhone microphone channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MicChannelId {
    Mic1 = 0,
    Mic2 = 1,
}

impl MicChannelId {
    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn from_one_based(id: u8) -> Option<Self> {
        match id {
            1 => Some(Self::Mic1),
            2 => Some(Self::Mic2),
            _ => None,
        }
    }

    pub const fn one_based(self) -> u8 {
        match self {
            Self::Mic1 => 1,
            Self::Mic2 => 2,
        }
    }
}

/// Instant vocal effect presets inspired by Smule & studio Voice Labs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum VocalPreset {
    Natural,
    #[default]
    Studio,
    Live,
    PopStar,
    Expert,
}

/// Full per-microphone DSP parameter set.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VocalDspParams {
    pub preset: VocalPreset,
    pub muted: bool,
    /// Input gain scalar `[0.0 .. 2.0]` (1.0 = 0 dB).
    pub gain: f32,
    /// High-pass cutoff frequency in Hz (default 80.0).
    pub hpf_hz: f32,
    /// Noise gate threshold in dBFS (`-70.0 .. -10.0`).
    pub gate_threshold_db: f32,
    /// Compressor ratio (`1.0 .. 8.0`, default 3.0).
    pub comp_ratio: f32,
    /// Compressor threshold in dBFS (default -18.0).
    pub comp_threshold_db: f32,
    /// Autotune enabled flag.
    pub autotune_enabled: bool,
    /// Autotune scale (automatically synced with song key + transposition).
    pub scale: ScaleType,
    /// Retune speed in milliseconds (`0.0` = instant T-Pain, `120.0` = natural).
    pub retune_speed_ms: f32,
    /// Autotune dry/wet blend (`0.0 .. 1.0`).
    pub autotune_mix: f32,
    /// Stereo reverb room size (`0.0 .. 1.0`).
    pub reverb_room_size: f32,
    /// Stereo reverb high-frequency damping (`0.0 .. 1.0`).
    pub reverb_damping: f32,
    /// Stereo reverb wet mix (`0.0 .. 1.0`).
    pub reverb_wet: f32,
    /// Presence / brilliance shelf boost in dB (`-6.0 .. +6.0`).
    pub presence_db: f32,
}

impl VocalDspParams {
    pub fn from_preset(preset: VocalPreset) -> Self {
        match preset {
            VocalPreset::Natural => Self {
                preset,
                muted: false,
                gain: 2.2,
                hpf_hz: 80.0,
                gate_threshold_db: -68.0,
                comp_ratio: 2.2,
                comp_threshold_db: -22.0,
                autotune_enabled: false,
                scale: ScaleType::Chromatic,
                retune_speed_ms: 120.0,
                autotune_mix: 0.0,
                reverb_room_size: 0.45,
                reverb_damping: 0.55,
                reverb_wet: 0.22,
                presence_db: 1.5,
            },
            VocalPreset::Studio => Self {
                preset,
                muted: false,
                gain: 2.5,
                hpf_hz: 80.0,
                gate_threshold_db: -66.0,
                comp_ratio: 3.2,
                comp_threshold_db: -24.0,
                autotune_enabled: true,
                scale: ScaleType::Chromatic,
                retune_speed_ms: 80.0,
                autotune_mix: 1.0,
                reverb_room_size: 0.64,
                reverb_damping: 0.38,
                reverb_wet: 0.40,
                presence_db: 2.8,
            },
            VocalPreset::Live => Self {
                preset,
                muted: false,
                gain: 2.7,
                hpf_hz: 85.0,
                gate_threshold_db: -64.0,
                comp_ratio: 3.5,
                comp_threshold_db: -24.0,
                autotune_enabled: true,
                scale: ScaleType::Chromatic,
                retune_speed_ms: 45.0,
                autotune_mix: 1.0,
                reverb_room_size: 0.84,
                reverb_damping: 0.28,
                reverb_wet: 0.55,
                presence_db: 3.0,
            },
            VocalPreset::PopStar => Self {
                preset,
                muted: false,
                gain: 2.8,
                hpf_hz: 85.0,
                gate_threshold_db: -65.0,
                comp_ratio: 4.0,
                comp_threshold_db: -25.0,
                autotune_enabled: true,
                scale: ScaleType::Chromatic,
                retune_speed_ms: 8.0,
                autotune_mix: 1.0,
                reverb_room_size: 0.68,
                reverb_damping: 0.28,
                reverb_wet: 0.45,
                presence_db: 3.8,
            },
            VocalPreset::Expert => {
                Self::from_preset(VocalPreset::Studio).with_preset(VocalPreset::Expert)
            }
        }
    }

    pub const fn with_preset(mut self, preset: VocalPreset) -> Self {
        self.preset = preset;
        self
    }
}

impl Default for VocalDspParams {
    fn default() -> Self {
        Self::from_preset(VocalPreset::Studio)
    }
}

/// 4-stem (or 6-stem grouped) real-time mix levels (`0.0 .. 1.5`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StemMix {
    /// Original isolated vocal track (Guide Vocal: `0.0` = pure instrumental, `1.0` = original mix).
    pub vocals: f32,
    pub drums: f32,
    pub bass: f32,
    pub other: f32,
    /// Optional extra stems when `htdemucs_6s` is used.
    pub guitar: f32,
    pub piano: f32,
}

impl Default for StemMix {
    fn default() -> Self {
        Self {
            vocals: 1.0,
            drums: 1.0,
            bass: 1.0,
            other: 1.0,
            guitar: 1.0,
            piano: 1.0,
        }
    }
}

/// HTDemucs model variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DemucsModel {
    #[default]
    Htdemucs,
    Htdemucs6s,
}

/// Stem separation lifecycle status for a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StemStatus {
    #[default]
    Pending,
    Processing,
    /// First 25 seconds processed: track can start playing while remaining chunks finish.
    StreamingReady,
    Completed,
    Failed,
}

/// Track source origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrackSource {
    #[default]
    Local,
    Youtube,
}

/// Library track metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: u64,
    pub source_type: TrackSource,
    pub source_path: String,
    pub source_url: Option<String>,
    pub content_hash: String,
    pub musical_key: MusicalKey,
    pub bpm: Option<f32>,
    pub favorite_transpose: i8,
    pub is_favorite: bool,
    pub stem_status: StemStatus,
    pub stem_progress: u8,
    pub stem_model: DemucsModel,
    pub stems_dir: Option<String>,
    pub has_lyrics: bool,
    pub has_word_sync: bool,
    pub created_at: String,
}

/// Solo or Duo mode for Party Queue entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum QueueMode {
    #[default]
    Solo,
    Duo,
}

/// Party Queue item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub id: String,
    pub track_id: String,
    pub track_title: String,
    pub track_artist: String,
    pub position: i64,
    pub mode: QueueMode,
    pub singer_1: String,
    pub singer_2: Option<String>,
    pub status: String,
    pub added_by: String,
}

/// Saved recording metadata (SingSnap-style review & post-mix).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingEntry {
    pub id: String,
    pub track_id: Option<String>,
    pub title: String,
    pub created_at: String,
    pub duration_ms: u64,
    pub instrumental_path: String,
    pub mic1_path: String,
    pub mic2_path: String,
    pub mix_vocal_balance: f32,
    pub exported_path: Option<String>,
}

/// Real-time telemetry for a single microphone channel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MicTelemetry {
    pub connected: bool,
    pub muted: bool,
    pub vu_db: f32,
    pub peak: f32,
    pub jitter_ms: f32,
    pub packet_loss_pct: f32,
    /// Detected vocal pitch in Hz (`0.0` when unvoiced).
    pub pitch_hz: f32,
    /// Closest MIDI note number (`0` when unvoiced).
    pub pitch_midi: i32,
    /// Pitch error relative to target scale note in cents (`-50.0 .. +50.0`).
    pub pitch_cents: f32,
    /// Gate open indicator.
    pub gate_open: bool,
}

/// A-B loop region in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LoopRegion {
    pub enabled: bool,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// 60 Hz real-time playback & audio engine telemetry pushed to Main + TV windows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackTelemetry {
    pub track_id: Option<String>,
    pub is_playing: bool,
    pub is_recording: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub transpose_semitones: i8,
    pub tempo_pct: f32,
    pub effective_key: MusicalKey,
    pub stem_mix: StemMix,
    pub ab_loop: LoopRegion,
    pub latency_calibration_ms: u32,
    pub buffer_size: AudioBufferSize,
    pub mic1: MicTelemetry,
    pub mic2: MicTelemetry,
}

impl Default for PlaybackTelemetry {
    fn default() -> Self {
        Self {
            track_id: None,
            is_playing: false,
            is_recording: false,
            position_ms: 0,
            duration_ms: 0,
            transpose_semitones: 0,
            tempo_pct: 100.0,
            effective_key: MusicalKey::default(),
            stem_mix: StemMix::default(),
            ab_loop: LoopRegion::default(),
            latency_calibration_ms: 0,
            buffer_size: AudioBufferSize::Frames128,
            mic1: MicTelemetry::default(),
            mic2: MicTelemetry::default(),
        }
    }
}

/// Native macOS CoreAudio input device discovered via `cpal` (Continuity iPhone Mic, USB iPhone,
/// MacBook built-in microphone, USB audio interface, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInputDeviceInfo {
    pub name: String,
    pub is_default: bool,
    pub is_iphone: bool,
    #[serde(default)]
    pub is_bluetooth: bool,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Native macOS CoreAudio output device discovered via `cpal` (speakers, HDMI TVs, Bluetooth, headphones).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioOutputDeviceInfo {
    pub name: String,
    pub is_default: bool,
    #[serde(default)]
    pub is_bluetooth: bool,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Complete active CoreAudio routing state (selected output speakers + Mic 1 & Mic 2 inputs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AudioRoutingState {
    pub output_device: Option<String>,
    pub mic1_device: Option<String>,
    pub mic2_device: Option<String>,
    #[serde(default)]
    pub clamshell_closed: bool,
}

/// Currently bound CoreAudio input devices for Mic 1 and Mic 2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActiveMicInputs {
    pub mic1_device: Option<String>,
    pub mic2_device: Option<String>,
}

/// Pairing information exposed by the local HTTPS/WSS microphone server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicPairingInfo {
    pub server_url: String,
    pub mdns_hostname: String,
    pub lan_ip: String,
    pub port: u16,
    pub ca_cert_url: String,
    pub ca_cert_qr_svg: String,
    pub mic1_url: String,
    pub mic2_url: String,
    pub mic1_qr_svg: String,
    pub mic2_qr_svg: String,
    pub session_token: String,
}
