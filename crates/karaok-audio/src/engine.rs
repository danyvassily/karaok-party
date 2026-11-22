use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use karaok_core::{
    AudioBufferSize, AudioOutputDeviceInfo, LoopRegion, MicChannelId, MusicalKey,
    PlaybackTelemetry, RecordingEntry, StemMix, VocalDspParams,
};
use karaok_dsp::{soft_limit, VocalDspChain};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use tracing::{info, warn};

use crate::atomics::AtomicEngineState;
use crate::decoder::{decode_audio_to_48khz_stereo, DecodedAudio};
use crate::error::{AudioError, AudioResult};
use crate::recorder::{RecordedFrame, RecordingHandle};
use crate::stretcher::PitchTempoProcessor;

/// Master CoreAudio sample rate (matches Safari iOS AudioWorklet 48 kHz natively).
pub const MASTER_SAMPLE_RATE: u32 = 48_000;

/// Maximum latency calibration delay supported for Bluetooth speakers / soundbars (250 ms).
const MAX_CALIBRATION_DELAY_FRAMES: usize = (MASTER_SAMPLE_RATE as usize * 250) / 1000;

/// Lock-free producer handle given to `karaok-mic-server` so incoming iPhone audio streams
/// directly into the CoreAudio real-time callback with zero locks.
pub struct MicAudioProducer {
    producer: HeapProd<f32>,
}

impl MicAudioProducer {
    /// Non-blocking push of resampled 48 kHz mono vocal samples into the CoreAudio callback.
    #[inline]
    pub fn push_samples(&mut self, samples: &[f32]) -> usize {
        self.producer.push_slice(samples)
    }
}

/// Multi-stem track buffers loaded in memory (48 kHz stereo planar).
/// When stems are not yet separated, `other` holds the full original stereo mix and `vocals/drums/bass` are empty.
#[derive(Clone)]
pub struct LoadedStemTrack {
    pub track_id: String,
    pub vocals: Option<DecodedAudio>,
    pub drums: Option<DecodedAudio>,
    pub bass: Option<DecodedAudio>,
    pub other: DecodedAudio,
    pub guitar: Option<DecodedAudio>,
    pub piano: Option<DecodedAudio>,
    pub total_frames: usize,
}

impl LoadedStemTrack {
    pub fn from_single_file(track_id: String, audio: DecodedAudio) -> Self {
        let total_frames = audio.frames();
        Self {
            track_id,
            vocals: None,
            drums: None,
            bass: None,
            other: audio,
            guitar: None,
            piano: None,
            total_frames,
        }
    }

    pub fn from_stems_dir(track_id: String, stems_dir: &Path) -> AudioResult<Self> {
        let vocals = decode_audio_to_48khz_stereo(&stems_dir.join("vocals.wav")).ok();
        let drums = decode_audio_to_48khz_stereo(&stems_dir.join("drums.wav")).ok();
        let bass = decode_audio_to_48khz_stereo(&stems_dir.join("bass.wav")).ok();
        let other = decode_audio_to_48khz_stereo(&stems_dir.join("other.wav"))?;
        let guitar = decode_audio_to_48khz_stereo(&stems_dir.join("guitar.wav")).ok();
        let piano = decode_audio_to_48khz_stereo(&stems_dir.join("piano.wav")).ok();

        let total_frames = other
            .frames()
            .max(vocals.as_ref().map_or(0, DecodedAudio::frames))
            .max(drums.as_ref().map_or(0, DecodedAudio::frames))
            .max(bass.as_ref().map_or(0, DecodedAudio::frames));

        Ok(Self {
            track_id,
            vocals,
            drums,
            bass,
            other,
            guitar,
            piano,
            total_frames,
        })
    }

    /// Mixes frame `idx` according to `StemMix` into stereo `(left, right)`.
    #[inline]
    pub fn sample_frame(&self, idx: usize, mix: &StemMix) -> (f32, f32) {
        let mut l = 0.0_f32;
        let mut r = 0.0_f32;

        if let Some(ref v) = self.vocals {
            if idx < v.frames() {
                l += v.left[idx] * mix.vocals;
                r += v.right[idx] * mix.vocals;
            }
        }
        if let Some(ref d) = self.drums {
            if idx < d.frames() {
                l += d.left[idx] * mix.drums;
                r += d.right[idx] * mix.drums;
            }
        }
        if let Some(ref b) = self.bass {
            if idx < b.frames() {
                l += b.left[idx] * mix.bass;
                r += b.right[idx] * mix.bass;
            }
        }
        if idx < self.other.frames() {
            l += self.other.left[idx] * mix.other;
            r += self.other.right[idx] * mix.other;
        }
        if let Some(ref g) = self.guitar {
            if idx < g.frames() {
                l += g.left[idx] * mix.guitar;
                r += g.right[idx] * mix.guitar;
            }
        }
        if let Some(ref p) = self.piano {
            if idx < p.frames() {
                l += p.left[idx] * mix.piano;
                r += p.right[idx] * mix.piano;
            }
        }
        (l, r)
    }
}

/// Pre-allocated stereo delay line inside the CoreAudio callback for Bluetooth / Soundbar
/// latency calibration (`0 .. 250 ms`). Zero heap allocations after construction.
struct CalibrationDelayLine {
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    write_idx: usize,
}

impl CalibrationDelayLine {
    fn new() -> Self {
        let len = MAX_CALIBRATION_DELAY_FRAMES + 64;
        Self {
            buf_l: vec![0.0; len],
            buf_r: vec![0.0; len],
            write_idx: 0,
        }
    }

    #[inline]
    fn process(&mut self, in_l: f32, in_r: f32, delay_frames: usize) -> (f32, f32) {
        let len = self.buf_l.len();
        if let Some(sl) = self.buf_l.get_mut(self.write_idx) {
            *sl = in_l;
        }
        if let Some(sr) = self.buf_r.get_mut(self.write_idx) {
            *sr = in_r;
        }

        let clamped = delay_frames.min(MAX_CALIBRATION_DELAY_FRAMES);
        let read_idx = (self.write_idx + len - clamped) % len;
        let out_l = self.buf_l.get(read_idx).copied().unwrap_or(in_l);
        let out_r = self.buf_r.get(read_idx).copied().unwrap_or(in_r);

        self.write_idx += 1;
        if self.write_idx >= len {
            self.write_idx = 0;
        }
        (out_l, out_r)
    }
}

fn cpal_device_name(dev: &cpal::Device) -> Option<String> {
    dev.description()
        .ok()
        .map(|d| d.name().to_string())
        .filter(|s| !s.trim().is_empty())
}

fn resolve_output_config(dev: &cpal::Device) -> Option<cpal::SupportedStreamConfig> {
    if let Ok(cfg) = dev.default_output_config() {
        if cfg.sample_rate() > 0 && cfg.channels() > 0 {
            return Some(cfg);
        }
    }
    let ranges = dev.supported_output_configs().ok()?;
    let mut best: Option<cpal::SupportedStreamConfig> = None;
    for r in ranges {
        if r.channels() == 0 {
            continue;
        }
        let target_sr = if r.min_sample_rate() <= MASTER_SAMPLE_RATE
            && r.max_sample_rate() >= MASTER_SAMPLE_RATE
        {
            MASTER_SAMPLE_RATE
        } else if r.min_sample_rate() <= 44_100 && r.max_sample_rate() >= 44_100 {
            44_100
        } else {
            r.max_sample_rate()
        };
        if target_sr == 0 {
            continue;
        }
        let cand = r.with_sample_rate(target_sr);
        let better = match &best {
            None => true,
            Some(prev) => {
                (cand.channels() >= 2 && prev.channels() < 2)
                    || (cand.sample_format() == SampleFormat::F32
                        && prev.sample_format() != SampleFormat::F32)
                    || cand.sample_rate() > prev.sample_rate()
            }
        };
        if better {
            best = Some(cand);
        }
    }
    best
}

/// Lists all usable CoreAudio output devices visible to macOS (MacBook speakers, Bluetooth
/// speakers like HECATE/RS-620, HDMI TVs, wired headphones, etc.).
pub fn list_audio_output_devices() -> Vec<AudioOutputDeviceInfo> {
    let host = cpal::default_host();
    let default_name = host
        .default_output_device()
        .and_then(|d| cpal_device_name(&d));

    let Ok(devices) = host.output_devices() else {
        return Vec::new();
    };

    let mut out: Vec<AudioOutputDeviceInfo> = Vec::new();
    for dev in devices {
        let Some(name) = cpal_device_name(&dev) else {
            continue;
        };
        let Some(cfg) = resolve_output_config(&dev) else {
            continue;
        };
        let sr = cfg.sample_rate();
        let ch = cfg.channels();
        if sr == 0 || ch == 0 {
            continue;
        }
        let is_default = default_name.as_deref() == Some(name.as_str());
        if let Some(existing) = out.iter_mut().find(|d| d.name == name) {
            // Keep the richer stereo / higher sample-rate endpoint if CoreAudio exposes two with the same name
            if ch > existing.channels || sr > existing.sample_rate || is_default {
                existing.channels = existing.channels.max(ch);
                existing.sample_rate = existing.sample_rate.max(sr);
                existing.is_default |= is_default;
            }
            continue;
        }
        out.push(AudioOutputDeviceInfo {
            name,
            is_default,
            sample_rate: sr,
            channels: ch,
        });
    }

    out.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    out
}

/// Holds the lock-free SPSC handles owned by the CoreAudio output callback while running.
struct OutputConsumers {
    music_cons: HeapCons<f32>,
    mic1_cons: HeapCons<f32>,
    mic2_cons: HeapCons<f32>,
    rec_prod: HeapProd<RecordedFrame>,
}

/// RAII guard living inside the CoreAudio callback closure: zero locks during audio rendering,
/// and automatically returns `OutputConsumers` back to `AudioEngine` when the stream is dropped
/// on the control thread during live output-device hot-switching.
struct RtOutputBundle {
    inner: Option<OutputConsumers>,
    return_slot: Arc<Mutex<Option<OutputConsumers>>>,
}

impl Drop for RtOutputBundle {
    fn drop(&mut self) {
        if let Some(bundle) = self.inner.take() {
            if let Ok(mut slot) = self.return_slot.lock() {
                *slot = Some(bundle);
            }
        }
    }
}

/// Complete real-time CoreAudio engine for Karaok Party with live output-device switching.
pub struct AudioEngine {
    state: Arc<AtomicEngineState>,
    loaded_track: Arc<Mutex<Option<Arc<LoadedStemTrack>>>>,
    current_track_id: Arc<Mutex<Option<String>>>,
    active_recording: Mutex<Option<RecordingHandle>>,
    /// Pre-allocated permanent SPSC consumer returned here whenever recording is idle,
    /// so the CoreAudio callback NEVER locks or drops its permanent `HeapProd<RecordedFrame>`.
    rec_consumer: Mutex<Option<HeapCons<RecordedFrame>>>,
    output_consumers: Arc<Mutex<Option<OutputConsumers>>>,
    active_output_device: Mutex<Option<String>>,
    shutdown: Arc<AtomicBool>,
    _feeder_thread: Option<JoinHandle<()>>,
    output_stream: Mutex<Option<Stream>>,
}

// Safety: `cpal::Stream` is guarded by Mutex and only created/dropped on control threads.
unsafe impl Send for AudioEngine {}
unsafe impl Sync for AudioEngine {}

impl AudioEngine {
    /// Initializes the lock-free audio graph and returns `(AudioEngine, [Mic1Producer, Mic2Producer])`.
    /// Call `start_coreaudio_stream()` or `select_output_device()` to attach to a macOS CoreAudio output.
    pub fn new() -> (Self, [MicAudioProducer; 2]) {
        let state = Arc::new(AtomicEngineState::default());
        let loaded_track: Arc<Mutex<Option<Arc<LoadedStemTrack>>>> = Arc::new(Mutex::new(None));
        let shutdown = Arc::new(AtomicBool::new(false));

        // Lock-free SPSC ring buffers (all pre-allocated once at startup):
        // Music stereo ring buffer: ~500 ms headroom at 48 kHz (2 channels) for rock-solid zero-underrun playback
        let music_rb = HeapRb::<f32>::new(MASTER_SAMPLE_RATE as usize);
        let (music_prod, music_cons) = music_rb.split();

        // Mic 1 & Mic 2 mono ring buffers: ~340 ms headroom each
        let mic1_rb = HeapRb::<f32>::new(16_384);
        let mic2_rb = HeapRb::<f32>::new(16_384);
        let (mic1_prod, mic1_cons) = mic1_rb.split();
        let (mic2_prod, mic2_cons) = mic2_rb.split();

        // Permanent recording SPSC ring buffer (~1 second capacity): never dropped in RT callback
        let rec_rb = HeapRb::<RecordedFrame>::new(MASTER_SAMPLE_RATE as usize);
        let (rec_prod, rec_cons) = rec_rb.split();

        let feeder_state = Arc::clone(&state);
        let feeder_track = Arc::clone(&loaded_track);
        let feeder_shutdown = Arc::clone(&shutdown);

        let feeder_thread = thread::Builder::new()
            .name("karaok-stem-feeder".into())
            .spawn(move || {
                run_stem_feeder_loop(feeder_state, feeder_track, music_prod, feeder_shutdown);
            })
            .ok();

        let consumers = OutputConsumers {
            music_cons,
            mic1_cons,
            mic2_cons,
            rec_prod,
        };

        let engine = Self {
            state,
            loaded_track,
            current_track_id: Arc::new(Mutex::new(None)),
            active_recording: Mutex::new(None),
            rec_consumer: Mutex::new(Some(rec_cons)),
            output_consumers: Arc::new(Mutex::new(Some(consumers))),
            active_output_device: Mutex::new(None),
            shutdown,
            _feeder_thread: feeder_thread,
            output_stream: Mutex::new(None),
        };

        (
            engine,
            [
                MicAudioProducer { producer: mic1_prod },
                MicAudioProducer { producer: mic2_prod },
            ],
        )
    }

    pub fn state(&self) -> Arc<AtomicEngineState> {
        Arc::clone(&self.state)
    }

    pub fn active_output_device(&self) -> Option<String> {
        self.active_output_device.lock().ok().and_then(|g| g.clone())
    }

    /// Starts the CoreAudio output stream on the macOS default output device (adapting cleanly
    /// to its native sample rate, channel count, and buffer size).
    pub fn start_coreaudio_stream(&self, buffer_size: AudioBufferSize) -> AudioResult<()> {
        self.state
            .buffer_size
            .store(buffer_size.as_u32(), Ordering::Relaxed);
        self.select_output_device(None)
    }

    /// Hot-switches the active CoreAudio output device (e.g. `Haut-parleurs MacBook Pro`,
    /// `HECATE G1000 II`, `RS-620`, HDMI TV, `Écouteurs externes`, or `None` for system default).
    ///
    /// Automatically matches the target speaker's native sample rate (44.1 kHz, 48 kHz, etc.)
    /// and channel count using zero-allocation linear resampling from our 48 kHz internal bus.
    pub fn select_output_device(&self, device_name: Option<&str>) -> AudioResult<()> {
        // 1. Resolve target device & configuration BEFORE stopping the currently playing stream
        // so an unavailable/disconnected device never interrupts playback.
        let host = cpal::default_host();
        let device = if let Some(target) = device_name.filter(|s| !s.trim().is_empty()) {
            host.output_devices()
                .map_err(|e| AudioError::Device(e.to_string()))?
                .find(|d| cpal_device_name(d).as_deref() == Some(target))
                .ok_or_else(|| {
                    AudioError::Device(format!("Sortie audio introuvable : « {target} »"))
                })?
        } else {
            host.default_output_device()
                .ok_or_else(|| AudioError::Device("Aucune sortie audio CoreAudio détectée".into()))?
        };

        let resolved_name =
            cpal_device_name(&device).unwrap_or_else(|| "Sortie par défaut".into());
        let default_cfg = resolve_output_config(&device).ok_or_else(|| {
            AudioError::Device(format!(
                "Configuration CoreAudio indisponible pour « {resolved_name} »"
            ))
        })?;
        let sample_format = default_cfg.sample_format();
        let config: StreamConfig = default_cfg.into();
        let out_rate = config.sample_rate.max(8000);
        let out_channels = config.channels.max(1) as usize;

        // 2. Stop & drop the previous stream cleanly on the control thread so `RtOutputBundle`
        // returns its lock-free SPSC handles back into `self.output_consumers`.
        {
            let mut stream_guard = self
                .output_stream
                .lock()
                .map_err(|_| AudioError::Device("Output stream lock poisoned".into()))?;
            *stream_guard = None;
        }

        let consumers = self
            .output_consumers
            .lock()
            .map_err(|_| AudioError::Device("Output consumers lock poisoned".into()))?
            .take()
            .ok_or_else(|| AudioError::Device("Audio consumers unavailable".into()))?;

        let mut rt_bundle = RtOutputBundle {
            inner: Some(consumers),
            return_slot: Arc::clone(&self.output_consumers),
        };
        let state = Arc::clone(&self.state);

        // Pre-allocate ALL DSP & output resampling state BEFORE entering CoreAudio RT thread!
        let mut mic1_dsp = VocalDspChain::new(MASTER_SAMPLE_RATE as f32);
        let mut mic2_dsp = VocalDspChain::new(MASTER_SAMPLE_RATE as f32);
        let mut calibration_delay = CalibrationDelayLine::new();
        let mut mic1_primed = false;
        let mut mic2_primed = false;
        let mut mic1_last = 0.0_f32;
        let mut mic2_last = 0.0_f32;

        // Zero-alloc inline output resampler (internal 48 kHz -> speaker native rate e.g. 44.1 / 48 kHz)
        let rate_step: f64 = (MASTER_SAMPLE_RATE as f64) / (out_rate as f64);
        let mut phase: f64 = 0.0;
        let mut prev_l = 0.0_f32;
        let mut prev_r = 0.0_f32;
        let mut curr_l = 0.0_f32;
        let mut curr_r = 0.0_f32;

        let mut render_block = move |out_buf_l: &mut [f32], out_buf_r: &mut [f32]| {
            // STRICT REAL-TIME RENDER BLOCK: zero allocations, zero locks, zero syscalls, zero unwrap().
            let frames = out_buf_l.len().min(out_buf_r.len());
            let Some(ref mut c) = rt_bundle.inner else {
                out_buf_l[..frames].fill(0.0);
                out_buf_r[..frames].fill(0.0);
                return;
            };

            if state.flush_music.swap(false, Ordering::AcqRel) {
                while c.music_cons.try_pop().is_some() {}
            }

            let is_playing = state.is_playing.load(Ordering::Relaxed);
            let effective_key = state.effective_key();
            let mic1_connected = state.mic_telemetry[0].connected.load(Ordering::Relaxed);
            let mic2_connected = state.mic_telemetry[1].connected.load(Ordering::Relaxed);
            let mic1_params = state.mic_params[0].load();
            let mic2_params = state.mic_params[1].load();
            let is_recording = state.is_recording.load(Ordering::Relaxed);
            let delay_ms = state
                .latency_calibration_ms
                .load(Ordering::Relaxed)
                .min(250) as usize;
            let delay_frames = (delay_ms * MASTER_SAMPLE_RATE as usize) / 1000;

            if mic1_connected {
                if !mic1_primed && c.mic1_cons.occupied_len() >= 256 {
                    mic1_primed = true;
                }
                if c.mic1_cons.occupied_len() > 2400 {
                    let drop_n = c.mic1_cons.occupied_len() - 384;
                    for _ in 0..drop_n {
                        let _ = c.mic1_cons.try_pop();
                    }
                }
            } else {
                mic1_primed = false;
                while c.mic1_cons.try_pop().is_some() {}
            }

            if mic2_connected {
                if !mic2_primed && c.mic2_cons.occupied_len() >= 256 {
                    mic2_primed = true;
                }
                if c.mic2_cons.occupied_len() > 2400 {
                    let drop_n = c.mic2_cons.occupied_len() - 384;
                    for _ in 0..drop_n {
                        let _ = c.mic2_cons.try_pop();
                    }
                }
            } else {
                mic2_primed = false;
                while c.mic2_cons.try_pop().is_some() {}
            }

            let mut produce_48k = || -> (f32, f32) {
                let (raw_music_l, raw_music_r) = if is_playing {
                    (
                        c.music_cons.try_pop().unwrap_or(0.0),
                        c.music_cons.try_pop().unwrap_or(0.0),
                    )
                } else {
                    (0.0, 0.0)
                };

                let (instru_l, instru_r) =
                    calibration_delay.process(raw_music_l, raw_music_r, delay_frames);

                let (m1_l, m1_r) = if mic1_connected && !mic1_params.muted {
                    let raw_m1 = if mic1_primed {
                        if let Some(s) = c.mic1_cons.try_pop() {
                            mic1_last = s;
                            s
                        } else {
                            mic1_primed = false;
                            mic1_last *= 0.9;
                            mic1_last
                        }
                    } else {
                        mic1_last *= 0.9;
                        mic1_last
                    };
                    mic1_dsp.process_sample(raw_m1, effective_key, &mic1_params)
                } else {
                    (0.0, 0.0)
                };

                let (m2_l, m2_r) = if mic2_connected && !mic2_params.muted {
                    let raw_m2 = if mic2_primed {
                        if let Some(s) = c.mic2_cons.try_pop() {
                            mic2_last = s;
                            s
                        } else {
                            mic2_primed = false;
                            mic2_last *= 0.9;
                            mic2_last
                        }
                    } else {
                        mic2_last *= 0.9;
                        mic2_last
                    };
                    mic2_dsp.process_sample(raw_m2, effective_key, &mic2_params)
                } else {
                    (0.0, 0.0)
                };

                if is_recording {
                    let _ = c.rec_prod.try_push(RecordedFrame {
                        instru_l,
                        instru_r,
                        mic1_l: m1_l,
                        mic1_r: m1_r,
                        mic2_l: m2_l,
                        mic2_r: m2_r,
                    });
                }

                (
                    soft_limit(instru_l + m1_l + m2_l),
                    soft_limit(instru_r + m1_r + m2_r),
                )
            };

            let exact_48k = (rate_step - 1.0).abs() < 1e-6;
            for i in 0..frames {
                let (l, r) = if exact_48k {
                    produce_48k()
                } else {
                    phase += rate_step;
                    while phase >= 1.0 {
                        phase -= 1.0;
                        prev_l = curr_l;
                        prev_r = curr_r;
                        let (nl, nr) = produce_48k();
                        curr_l = nl;
                        curr_r = nr;
                    }
                    let t = phase as f32;
                    (prev_l + (curr_l - prev_l) * t, prev_r + (curr_r - prev_r) * t)
                };
                out_buf_l[i] = l;
                out_buf_r[i] = r;
            }

            // Publish lock-free telemetry ONCE per CoreAudio callback (not 48,000x/sec!)
            state
                .music_buffered_frames
                .store((c.music_cons.occupied_len() / 2) as u64, Ordering::Relaxed);
            state.mic_telemetry[0].store_dsp(mic1_dsp.telemetry(), mic1_params.muted);
            state.mic_telemetry[1].store_dsp(mic2_dsp.telemetry(), mic2_params.muted);
        };

        let mut scratch_l = [0.0_f32; 4096];
        let mut scratch_r = [0.0_f32; 4096];

        let err_dev = resolved_name.clone();
        let err_fn = move |err: cpal::Error| {
            warn!("CoreAudio output error on '{err_dev}': {err}");
        };

        let stream = match sample_format {
            SampleFormat::I16 => device
                .build_output_stream(
                    config,
                    move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                        let total_frames = data.len() / out_channels;
                        let mut offset = 0usize;
                        while offset < total_frames {
                            let n = (total_frames - offset).min(scratch_l.len());
                            render_block(&mut scratch_l[..n], &mut scratch_r[..n]);
                            let to_i16 =
                                |s: f32| (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
                            for i in 0..n {
                                let base = (offset + i) * out_channels;
                                let l = scratch_l[i];
                                let r = scratch_r[i];
                                if out_channels == 1 {
                                    data[base] = to_i16(0.5 * (l + r));
                                } else {
                                    data[base] = to_i16(l);
                                    data[base + 1] = to_i16(r);
                                    for ch in 2..out_channels {
                                        data[base + ch] = 0;
                                    }
                                }
                            }
                            offset += n;
                        }
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| AudioError::Device(e.to_string()))?,
            SampleFormat::U16 => device
                .build_output_stream(
                    config,
                    move |data: &mut [u16], _: &cpal::OutputCallbackInfo| {
                        let total_frames = data.len() / out_channels;
                        let mut offset = 0usize;
                        while offset < total_frames {
                            let n = (total_frames - offset).min(scratch_l.len());
                            render_block(&mut scratch_l[..n], &mut scratch_r[..n]);
                            let to_u16 = |s: f32| {
                                ((s.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32).round() as u16
                            };
                            for i in 0..n {
                                let base = (offset + i) * out_channels;
                                let l = scratch_l[i];
                                let r = scratch_r[i];
                                if out_channels == 1 {
                                    data[base] = to_u16(0.5 * (l + r));
                                } else {
                                    data[base] = to_u16(l);
                                    data[base + 1] = to_u16(r);
                                    for ch in 2..out_channels {
                                        data[base + ch] = u16::MAX / 2;
                                    }
                                }
                            }
                            offset += n;
                        }
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| AudioError::Device(e.to_string()))?,
            // CoreAudio AUHAL natively accepts F32 for F32 / I24 / I32 / F64 hardware endpoints
            _ => device
                .build_output_stream(
                    config,
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        let total_frames = data.len() / out_channels;
                        let mut offset = 0usize;
                        while offset < total_frames {
                            let n = (total_frames - offset).min(scratch_l.len());
                            render_block(&mut scratch_l[..n], &mut scratch_r[..n]);
                            for i in 0..n {
                                let base = (offset + i) * out_channels;
                                let l = scratch_l[i];
                                let r = scratch_r[i];
                                if out_channels == 1 {
                                    data[base] = 0.5 * (l + r);
                                } else {
                                    data[base] = l;
                                    data[base + 1] = r;
                                    for ch in 2..out_channels {
                                        data[base + ch] = 0.0;
                                    }
                                }
                            }
                            offset += n;
                        }
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| AudioError::Device(e.to_string()))?,
        };

        stream
            .play()
            .map_err(|e| AudioError::Device(e.to_string()))?;

        if let Ok(mut guard) = self.output_stream.lock() {
            *guard = Some(stream);
        }
        if let Ok(mut name_guard) = self.active_output_device.lock() {
            *name_guard = Some(resolved_name.clone());
        }

        info!(
            "CoreAudio output active on '{}' ({} Hz, {} ch)",
            resolved_name, out_rate, out_channels
        );
        Ok(())
    }

    /// Loads a local audio file or a separated stems directory into the active player deck.
    pub fn load_track(
        &self,
        track_id: String,
        source_path: &Path,
        stems_dir: Option<&Path>,
        musical_key: MusicalKey,
        favorite_transpose: i8,
    ) -> AudioResult<u64> {
        let _ = favorite_transpose;
        // Only load stems if manifest matches clean v2 crossover cache (avoids old noisy v1 stems!)
        let valid_v2_stems = stems_dir.filter(|dir| {
            dir.join("other.wav").exists()
                && std::fs::read_to_string(dir.join("manifest.json"))
                    .ok()
                    .is_some_and(|raw| raw.contains("\"cacheVersion\": 2"))
        });

        let loaded = if let Some(dir) = valid_v2_stems {
            LoadedStemTrack::from_stems_dir(track_id.clone(), dir)?
        } else {
            let decoded = decode_audio_to_48khz_stereo(source_path)?;
            LoadedStemTrack::from_single_file(track_id.clone(), decoded)
        };

        let duration_frames = loaded.total_frames as u64;
        let duration_ms = (duration_frames * 1000) / MASTER_SAMPLE_RATE as u64;

        self.state.is_playing.store(false, Ordering::Release);
        self.state.flush_music.store(true, Ordering::Release);
        self.state.position_frames.store(0, Ordering::Relaxed);
        self.state
            .duration_frames
            .store(duration_frames, Ordering::Relaxed);
        self.state.seek_requested_frame.store(0, Ordering::Release);
        self.state.set_song_key(musical_key);
        self.state.transpose_semitones.store(0, Ordering::Relaxed);

        if let Ok(mut guard) = self.loaded_track.lock() {
            *guard = Some(Arc::new(loaded));
        }
        if let Ok(mut guard) = self.current_track_id.lock() {
            *guard = Some(track_id);
        }

        Ok(duration_ms)
    }

    /// Hot-swaps separated stems into the currently playing deck without resetting position.
    pub fn hot_swap_stems(&self, track_id: &str, stems_dir: &Path) -> AudioResult<()> {
        let current_matches = self
            .current_track_id
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .is_some_and(|id| id == track_id);
        if !current_matches {
            return Ok(());
        }
        let loaded = LoadedStemTrack::from_stems_dir(track_id.to_string(), stems_dir)?;
        if let Ok(mut guard) = self.loaded_track.lock() {
            *guard = Some(Arc::new(loaded));
        }
        Ok(())
    }

    pub fn play(&self) {
        self.state.is_playing.store(true, Ordering::Release);
    }

    pub fn pause(&self) {
        self.state.is_playing.store(false, Ordering::Release);
    }

    pub fn seek_ms(&self, position_ms: u64) {
        let max_frames = self.state.duration_frames.load(Ordering::Relaxed);
        let target = ((position_ms * MASTER_SAMPLE_RATE as u64) / 1000).min(max_frames);
        self.state.position_frames.store(target, Ordering::Relaxed);
        self.state
            .seek_requested_frame
            .store(target.min(i32::MAX as u64) as i32, Ordering::Release);
        self.state.flush_music.store(true, Ordering::Release);
    }

    pub fn rewind_5s(&self) {
        let cur_frames = self.state.position_frames.load(Ordering::Relaxed);
        let cur_ms = (cur_frames * 1000) / MASTER_SAMPLE_RATE as u64;
        self.seek_ms(cur_ms.saturating_sub(5000));
    }

    pub fn set_transpose(&self, semitones: i8) {
        self.state
            .transpose_semitones
            .store(semitones.clamp(-6, 6) as i32, Ordering::Relaxed);
    }

    pub fn set_tempo_pct(&self, tempo_pct: f32) {
        self.state.set_tempo_pct(tempo_pct);
    }

    pub fn set_stem_mix(&self, mix: StemMix) {
        self.state.store_stem_mix(mix);
    }

    pub fn set_ab_loop(&self, region: LoopRegion) {
        self.state.store_ab_loop(region);
    }

    pub fn set_latency_calibration_ms(&self, ms: u32) {
        self.state
            .latency_calibration_ms
            .store(ms.min(250), Ordering::Relaxed);
    }

    pub fn set_mic_params(&self, ch: MicChannelId, params: VocalDspParams) {
        self.state.mic_params(ch).store(&params);
    }

    pub fn start_recording(&self, recordings_dir: &Path, title: String) -> AudioResult<()> {
        let mut guard = self
            .active_recording
            .lock()
            .map_err(|_| AudioError::Recorder("Recorder lock poisoned".into()))?;
        if guard.is_some() {
            return Err(AudioError::Recorder("Recording is already running".into()));
        }

        // Only link to a real library track_id (None for freestyle or preview playback)
        let track_id = self
            .current_track_id
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .filter(|id| !id.starts_with("preview-"));

        let cons = self
            .rec_consumer
            .lock()
            .map_err(|_| AudioError::Recorder("Recorder consumer lock poisoned".into()))?
            .take()
            .ok_or_else(|| AudioError::Recorder("Recording consumer unavailable".into()))?;

        let handle = RecordingHandle::start(recordings_dir, track_id, title, cons)?;
        *guard = Some(handle);
        self.state.is_recording.store(true, Ordering::Release);
        Ok(())
    }

    pub fn stop_recording(&self) -> AudioResult<RecordingEntry> {
        self.state.is_recording.store(false, Ordering::Release);
        let mut guard = self
            .active_recording
            .lock()
            .map_err(|_| AudioError::Recorder("Recorder lock poisoned".into()))?;
        let handle = guard
            .take()
            .ok_or_else(|| AudioError::Recorder("No active recording".into()))?;
        let (entry, returned_cons) = handle.stop()?;
        if let Ok(mut slot) = self.rec_consumer.lock() {
            *slot = Some(returned_cons);
        }
        Ok(entry)
    }

    /// Reads a complete lock-free 60 Hz telemetry snapshot for Main + TV windows.
    pub fn telemetry(&self) -> PlaybackTelemetry {
        let feeder_pos = self.state.position_frames.load(Ordering::Relaxed);
        let buffered = self.state.music_buffered_frames.load(Ordering::Relaxed);
        let pos_frames = feeder_pos.saturating_sub(buffered);
        let dur_frames = self.state.duration_frames.load(Ordering::Relaxed);
        let track_id = self
            .current_track_id
            .lock()
            .ok()
            .and_then(|g| g.clone());

        PlaybackTelemetry {
            track_id,
            is_playing: self.state.is_playing.load(Ordering::Relaxed),
            is_recording: self.state.is_recording.load(Ordering::Relaxed),
            position_ms: (pos_frames * 1000) / MASTER_SAMPLE_RATE as u64,
            duration_ms: (dur_frames * 1000) / MASTER_SAMPLE_RATE as u64,
            transpose_semitones: self
                .state
                .transpose_semitones
                .load(Ordering::Relaxed)
                .clamp(-6, 6) as i8,
            tempo_pct: self.state.tempo_pct(),
            effective_key: self.state.effective_key(),
            stem_mix: self.state.load_stem_mix(),
            ab_loop: self.state.load_ab_loop(),
            latency_calibration_ms: self.state.latency_calibration_ms.load(Ordering::Relaxed),
            buffer_size: AudioBufferSize::from_u32(self.state.buffer_size.load(Ordering::Relaxed)),
            mic1: self.state.mic_telemetry[0].load(),
            mic2: self.state.mic_telemetry[1].load(),
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
    }
}

/// Background feeder thread: reads the active multi-stem track, applies `signalsmith-stretch`
/// pitch transposition (`-6..+6`) & tempo scaling (`70%..130%`), handles A-B looping, and pushes
/// stereo 48 kHz samples into the lock-free SPSC `ringbuf` consumed by CoreAudio.
fn run_stem_feeder_loop(
    state: Arc<AtomicEngineState>,
    loaded_track: Arc<Mutex<Option<Arc<LoadedStemTrack>>>>,
    mut prod: HeapProd<f32>,
    shutdown: Arc<AtomicBool>,
) {
    let mut stretcher = PitchTempoProcessor::new(MASTER_SAMPLE_RATE);
    let mut cursor: usize = 0;
    let out_block_frames: usize = 512;
    let mut in_l = Vec::with_capacity(1024);
    let mut in_r = Vec::with_capacity(1024);
    let mut stretched_interleaved = Vec::with_capacity(out_block_frames * 2);

    while !shutdown.load(Ordering::Acquire) {
        // Apply pending seek immediately
        let seek_req = state.seek_requested_frame.swap(-1, Ordering::AcqRel);
        if seek_req >= 0 {
            cursor = seek_req as usize;
            stretcher.reset();
        }

        if !state.is_playing.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(4));
            continue;
        }

        // Wait until ring buffer has space for the next stereo block
        if prod.vacant_len() < out_block_frames * 2 {
            thread::sleep(Duration::from_millis(2));
            continue;
        }

        let track_opt = loaded_track.lock().ok().and_then(|g| g.clone());
        let Some(track) = track_opt else {
            thread::sleep(Duration::from_millis(8));
            continue;
        };

        if track.total_frames == 0 {
            state.is_playing.store(false, Ordering::Release);
            continue;
        }

        let mix = state.load_stem_mix();
        // Transposition disabled (pure 1:1 studio playback quality without pitch-shift artifacts)
        let tempo_ratio = (state.tempo_pct() / 100.0).clamp(0.70, 1.30);
        let ab = state.load_ab_loop();

        let loop_start = ((ab.start_ms * MASTER_SAMPLE_RATE as u64) / 1000) as usize;
        let loop_end = ((ab.end_ms * MASTER_SAMPLE_RATE as u64) / 1000) as usize;

        stretcher.set_semitones(0.0);
        let in_frames = ((out_block_frames as f32) * tempo_ratio).round().max(64.0) as usize;

        in_l.clear();
        in_r.clear();
        for _ in 0..in_frames {
            if ab.enabled && loop_end > loop_start + 4800 && cursor >= loop_end {
                cursor = loop_start;
            }
            if cursor >= track.total_frames {
                if ab.enabled && loop_end > loop_start {
                    cursor = loop_start;
                } else {
                    in_l.push(0.0);
                    in_r.push(0.0);
                    continue;
                }
            }
            let (l, r) = track.sample_frame(cursor, &mix);
            in_l.push(l);
            in_r.push(r);
            cursor += 1;
        }

        stretcher.process_stereo(&in_l, &in_r, out_block_frames, &mut stretched_interleaved);
        prod.push_slice(&stretched_interleaved);

        state
            .position_frames
            .store(cursor.min(track.total_frames) as u64, Ordering::Relaxed);

        if cursor >= track.total_frames && !ab.enabled {
            state.is_playing.store(false, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibration_delay_line_delays_accurately() {
        let mut dl = CalibrationDelayLine::new();
        // Push impulse at frame 0 with delay = 10 frames
        let (l0, r0) = dl.process(1.0, -1.0, 10);
        assert_eq!(l0, 0.0);
        assert_eq!(r0, 0.0);
        for _ in 1..10 {
            let (l, r) = dl.process(0.0, 0.0, 10);
            assert_eq!(l, 0.0);
            assert_eq!(r, 0.0);
        }
        let (l10, r10) = dl.process(0.0, 0.0, 10);
        assert_eq!(l10, 1.0);
        assert_eq!(r10, -1.0);
    }

    #[test]
    fn engine_state_updates_effective_key_on_transpose() {
        let (engine, _mics) = AudioEngine::new();
        engine.state().set_song_key(MusicalKey::new(
            karaok_core::PitchClass::G,
            karaok_core::ScaleType::Major,
        ));
        engine.set_transpose(-2);
        let t = engine.telemetry();
        assert_eq!(t.transpose_semitones, -2);
        assert_eq!(t.effective_key.root, karaok_core::PitchClass::F);
    }

    #[test]
    fn output_hot_switch_preserves_consumers_and_switches_speakers() {
        let outputs = list_audio_output_devices();
        let Some(alt_speaker) = outputs
            .iter()
            .find(|d| !d.is_default && d.channels >= 2 && d.sample_rate >= 44_100)
        else {
            return;
        };

        let (engine, _mics) = AudioEngine::new();
        engine
            .select_output_device(None)
            .expect("start on default output");
        assert!(engine.active_output_device().is_some());

        // Switching to a nonexistent output must fail WITHOUT losing the running stream or consumers
        assert!(engine
            .select_output_device(Some("__NonExistent_Karaok_Device__"))
            .is_err());

        // Switching to a real non-default speaker (e.g. Haut-parleurs MacBook Pro / HECATE G1000 II)
        engine
            .select_output_device(Some(&alt_speaker.name))
            .expect("hot-switch to non-default speaker");
        assert_eq!(
            engine.active_output_device().as_deref(),
            Some(alt_speaker.name.as_str())
        );
    }
}
