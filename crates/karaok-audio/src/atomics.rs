use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};

use karaok_core::{
    AudioBufferSize, LoopRegion, MicChannelId, MicTelemetry, MusicalKey, PitchClass, ScaleType,
    StemMix, VocalDspParams, VocalPreset,
};
use karaok_dsp::VocalTelemetrySnapshot;

#[inline]
fn store_f32(slot: &AtomicU32, v: f32) {
    slot.store(v.to_bits(), Ordering::Relaxed);
}

#[inline]
fn load_f32(slot: &AtomicU32) -> f32 {
    f32::from_bits(slot.load(Ordering::Relaxed))
}

/// Lock-free atomic mirror of `VocalDspParams` so the CoreAudio callback reads vocal settings
/// with zero mutex locks and zero allocations.
pub struct AtomicVocalParams {
    preset: AtomicU32,
    muted: AtomicBool,
    gain: AtomicU32,
    hpf_hz: AtomicU32,
    gate_threshold_db: AtomicU32,
    comp_ratio: AtomicU32,
    comp_threshold_db: AtomicU32,
    autotune_enabled: AtomicBool,
    scale: AtomicU32,
    retune_speed_ms: AtomicU32,
    autotune_mix: AtomicU32,
    reverb_room_size: AtomicU32,
    reverb_damping: AtomicU32,
    reverb_wet: AtomicU32,
    presence_db: AtomicU32,
}

impl AtomicVocalParams {
    pub fn new(initial: VocalDspParams) -> Self {
        let s = Self {
            preset: AtomicU32::new(0),
            muted: AtomicBool::new(false),
            gain: AtomicU32::new(1.0_f32.to_bits()),
            hpf_hz: AtomicU32::new(80.0_f32.to_bits()),
            gate_threshold_db: AtomicU32::new((-45.0_f32).to_bits()),
            comp_ratio: AtomicU32::new(3.0_f32.to_bits()),
            comp_threshold_db: AtomicU32::new((-18.0_f32).to_bits()),
            autotune_enabled: AtomicBool::new(true),
            scale: AtomicU32::new(1),
            retune_speed_ms: AtomicU32::new(85.0_f32.to_bits()),
            autotune_mix: AtomicU32::new(0.55_f32.to_bits()),
            reverb_room_size: AtomicU32::new(0.55_f32.to_bits()),
            reverb_damping: AtomicU32::new(0.45_f32.to_bits()),
            reverb_wet: AtomicU32::new(0.24_f32.to_bits()),
            presence_db: AtomicU32::new(1.8_f32.to_bits()),
        };
        s.store(&initial);
        s
    }

    pub fn store(&self, p: &VocalDspParams) {
        self.preset.store(
            match p.preset {
                VocalPreset::Natural => 0,
                VocalPreset::Studio => 1,
                VocalPreset::Live => 2,
                VocalPreset::PopStar => 3,
                VocalPreset::Expert => 4,
            },
            Ordering::Relaxed,
        );
        self.muted.store(p.muted, Ordering::Relaxed);
        store_f32(&self.gain, p.gain);
        store_f32(&self.hpf_hz, p.hpf_hz);
        store_f32(&self.gate_threshold_db, p.gate_threshold_db);
        store_f32(&self.comp_ratio, p.comp_ratio);
        store_f32(&self.comp_threshold_db, p.comp_threshold_db);
        self.autotune_enabled
            .store(p.autotune_enabled, Ordering::Relaxed);
        self.scale.store(
            match p.scale {
                ScaleType::Chromatic => 0,
                ScaleType::Major => 1,
                ScaleType::Minor => 2,
            },
            Ordering::Relaxed,
        );
        store_f32(&self.retune_speed_ms, p.retune_speed_ms);
        store_f32(&self.autotune_mix, p.autotune_mix);
        store_f32(&self.reverb_room_size, p.reverb_room_size);
        store_f32(&self.reverb_damping, p.reverb_damping);
        store_f32(&self.reverb_wet, p.reverb_wet);
        store_f32(&self.presence_db, p.presence_db);
    }

    #[inline]
    pub fn load(&self) -> VocalDspParams {
        VocalDspParams {
            preset: match self.preset.load(Ordering::Relaxed) {
                0 => VocalPreset::Natural,
                2 => VocalPreset::Live,
                3 => VocalPreset::PopStar,
                4 => VocalPreset::Expert,
                _ => VocalPreset::Studio,
            },
            muted: self.muted.load(Ordering::Relaxed),
            gain: load_f32(&self.gain),
            hpf_hz: load_f32(&self.hpf_hz),
            gate_threshold_db: load_f32(&self.gate_threshold_db),
            comp_ratio: load_f32(&self.comp_ratio),
            comp_threshold_db: load_f32(&self.comp_threshold_db),
            autotune_enabled: self.autotune_enabled.load(Ordering::Relaxed),
            scale: match self.scale.load(Ordering::Relaxed) {
                0 => ScaleType::Chromatic,
                2 => ScaleType::Minor,
                _ => ScaleType::Major,
            },
            retune_speed_ms: load_f32(&self.retune_speed_ms),
            autotune_mix: load_f32(&self.autotune_mix),
            reverb_room_size: load_f32(&self.reverb_room_size),
            reverb_damping: load_f32(&self.reverb_damping),
            reverb_wet: load_f32(&self.reverb_wet),
            presence_db: load_f32(&self.presence_db),
        }
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    pub fn set_gain(&self, gain: f32) {
        store_f32(&self.gain, gain.clamp(0.0, 2.5));
    }
}

/// Lock-free atomic telemetry for one wireless microphone channel.
pub struct AtomicMicTelemetry {
    pub connected: AtomicBool,
    pub muted: AtomicBool,
    pub vu_db: AtomicU32,
    pub peak: AtomicU32,
    pub jitter_ms: AtomicU32,
    pub packet_loss_pct: AtomicU32,
    pub pitch_hz: AtomicU32,
    pub pitch_midi: AtomicI32,
    pub pitch_cents: AtomicU32,
    pub gate_open: AtomicBool,
}

impl Default for AtomicMicTelemetry {
    fn default() -> Self {
        Self {
            connected: AtomicBool::new(false),
            muted: AtomicBool::new(false),
            vu_db: AtomicU32::new((-96.0_f32).to_bits()),
            peak: AtomicU32::new(0.0_f32.to_bits()),
            jitter_ms: AtomicU32::new(0.0_f32.to_bits()),
            packet_loss_pct: AtomicU32::new(0.0_f32.to_bits()),
            pitch_hz: AtomicU32::new(0.0_f32.to_bits()),
            pitch_midi: AtomicI32::new(0),
            pitch_cents: AtomicU32::new(0.0_f32.to_bits()),
            gate_open: AtomicBool::new(false),
        }
    }
}

impl AtomicMicTelemetry {
    #[inline]
    pub fn store_dsp(&self, snap: VocalTelemetrySnapshot, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
        store_f32(&self.vu_db, snap.vu_db);
        store_f32(&self.peak, snap.peak);
        store_f32(&self.pitch_hz, snap.pitch_hz);
        self.pitch_midi.store(snap.pitch_midi, Ordering::Relaxed);
        store_f32(&self.pitch_cents, snap.pitch_cents);
        self.gate_open.store(snap.gate_open, Ordering::Relaxed);
    }

    pub fn store_network(&self, connected: bool, jitter_ms: f32, packet_loss_pct: f32) {
        self.connected.store(connected, Ordering::Relaxed);
        store_f32(&self.jitter_ms, jitter_ms);
        store_f32(&self.packet_loss_pct, packet_loss_pct);
    }

    pub fn load(&self) -> MicTelemetry {
        MicTelemetry {
            connected: self.connected.load(Ordering::Relaxed),
            muted: self.muted.load(Ordering::Relaxed),
            vu_db: load_f32(&self.vu_db),
            peak: load_f32(&self.peak),
            jitter_ms: load_f32(&self.jitter_ms),
            packet_loss_pct: load_f32(&self.packet_loss_pct),
            pitch_hz: load_f32(&self.pitch_hz),
            pitch_midi: self.pitch_midi.load(Ordering::Relaxed),
            pitch_cents: load_f32(&self.pitch_cents),
            gate_open: self.gate_open.load(Ordering::Relaxed),
        }
    }
}

/// Complete shared lock-free parameter & telemetry block shared between UI, feeder thread,
/// iPhone WebSocket server, and the CoreAudio real-time callback.
pub struct AtomicEngineState {
    pub is_playing: AtomicBool,
    pub is_recording: AtomicBool,
    pub flush_music: AtomicBool,
    pub position_frames: AtomicU64,
    pub music_buffered_frames: AtomicU64,
    pub duration_frames: AtomicU64,
    pub seek_requested_frame: AtomicI32, // < 0 means no seek pending
    pub transpose_semitones: AtomicI32,
    pub tempo_pct: AtomicU32,
    pub song_key_root: AtomicU32,
    pub song_key_scale: AtomicU32,
    pub stem_vocals: AtomicU32,
    pub stem_drums: AtomicU32,
    pub stem_bass: AtomicU32,
    pub stem_other: AtomicU32,
    pub stem_guitar: AtomicU32,
    pub stem_piano: AtomicU32,
    pub loop_enabled: AtomicBool,
    pub loop_start_ms: AtomicU64,
    pub loop_end_ms: AtomicU64,
    pub latency_calibration_ms: AtomicU32,
    pub buffer_size: AtomicU32,
    pub mic_params: [AtomicVocalParams; 2],
    pub mic_telemetry: [AtomicMicTelemetry; 2],
}

impl Default for AtomicEngineState {
    fn default() -> Self {
        let mix = StemMix::default();
        Self {
            is_playing: AtomicBool::new(false),
            is_recording: AtomicBool::new(false),
            flush_music: AtomicBool::new(false),
            position_frames: AtomicU64::new(0),
            music_buffered_frames: AtomicU64::new(0),
            duration_frames: AtomicU64::new(0),
            seek_requested_frame: AtomicI32::new(-1),
            transpose_semitones: AtomicI32::new(0),
            tempo_pct: AtomicU32::new(100.0_f32.to_bits()),
            song_key_root: AtomicU32::new(0),
            song_key_scale: AtomicU32::new(1),
            stem_vocals: AtomicU32::new(mix.vocals.to_bits()),
            stem_drums: AtomicU32::new(mix.drums.to_bits()),
            stem_bass: AtomicU32::new(mix.bass.to_bits()),
            stem_other: AtomicU32::new(mix.other.to_bits()),
            stem_guitar: AtomicU32::new(mix.guitar.to_bits()),
            stem_piano: AtomicU32::new(mix.piano.to_bits()),
            loop_enabled: AtomicBool::new(false),
            loop_start_ms: AtomicU64::new(0),
            loop_end_ms: AtomicU64::new(0),
            latency_calibration_ms: AtomicU32::new(0),
            buffer_size: AtomicU32::new(AudioBufferSize::Frames128.as_u32()),
            mic_params: [
                AtomicVocalParams::new(VocalDspParams::from_preset(VocalPreset::Studio)),
                AtomicVocalParams::new(VocalDspParams::from_preset(VocalPreset::Studio)),
            ],
            mic_telemetry: [AtomicMicTelemetry::default(), AtomicMicTelemetry::default()],
        }
    }
}

impl AtomicEngineState {
    #[inline]
    pub fn effective_key(&self) -> MusicalKey {
        let base_root = PitchClass::from_semitone(self.song_key_root.load(Ordering::Relaxed) as i32);
        let scale = match self.song_key_scale.load(Ordering::Relaxed) {
            0 => ScaleType::Chromatic,
            2 => ScaleType::Minor,
            _ => ScaleType::Major,
        };
        let transpose = self.transpose_semitones.load(Ordering::Relaxed).clamp(-6, 6) as i8;
        MusicalKey::new(base_root, scale).transpose(transpose)
    }

    pub fn set_song_key(&self, key: MusicalKey) {
        self.song_key_root.store(key.root as u32, Ordering::Relaxed);
        self.song_key_scale.store(
            match key.scale {
                ScaleType::Chromatic => 0,
                ScaleType::Major => 1,
                ScaleType::Minor => 2,
            },
            Ordering::Relaxed,
        );
    }

    pub fn store_stem_mix(&self, mix: StemMix) {
        store_f32(&self.stem_vocals, mix.vocals.clamp(0.0, 1.5));
        store_f32(&self.stem_drums, mix.drums.clamp(0.0, 1.5));
        store_f32(&self.stem_bass, mix.bass.clamp(0.0, 1.5));
        store_f32(&self.stem_other, mix.other.clamp(0.0, 1.5));
        store_f32(&self.stem_guitar, mix.guitar.clamp(0.0, 1.5));
        store_f32(&self.stem_piano, mix.piano.clamp(0.0, 1.5));
    }

    #[inline]
    pub fn load_stem_mix(&self) -> StemMix {
        StemMix {
            vocals: load_f32(&self.stem_vocals),
            drums: load_f32(&self.stem_drums),
            bass: load_f32(&self.stem_bass),
            other: load_f32(&self.stem_other),
            guitar: load_f32(&self.stem_guitar),
            piano: load_f32(&self.stem_piano),
        }
    }

    pub fn store_ab_loop(&self, region: LoopRegion) {
        self.loop_enabled.store(region.enabled, Ordering::Relaxed);
        self.loop_start_ms.store(region.start_ms, Ordering::Relaxed);
        self.loop_end_ms.store(region.end_ms, Ordering::Relaxed);
    }

    #[inline]
    pub fn load_ab_loop(&self) -> LoopRegion {
        LoopRegion {
            enabled: self.loop_enabled.load(Ordering::Relaxed),
            start_ms: self.loop_start_ms.load(Ordering::Relaxed),
            end_ms: self.loop_end_ms.load(Ordering::Relaxed),
        }
    }

    pub fn set_tempo_pct(&self, pct: f32) {
        store_f32(&self.tempo_pct, pct.clamp(70.0, 130.0));
    }

    #[inline]
    pub fn tempo_pct(&self) -> f32 {
        load_f32(&self.tempo_pct)
    }

    pub fn mic_params(&self, ch: MicChannelId) -> &AtomicVocalParams {
        &self.mic_params[ch.index()]
    }

    pub fn mic_telemetry(&self, ch: MicChannelId) -> &AtomicMicTelemetry {
        &self.mic_telemetry[ch.index()]
    }
}
