use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleRate, Stream, StreamConfig};
use karaok_core::{
    AudioBufferSize, LoopRegion, MicChannelId, MusicalKey, PlaybackTelemetry, RecordingEntry,
    StemMix, VocalDspParams,
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

/// Complete real-time CoreAudio engine for Karaok Party.
pub struct AudioEngine {
    state: Arc<AtomicEngineState>,
    loaded_track: Arc<Mutex<Option<Arc<LoadedStemTrack>>>>,
    current_track_id: Arc<Mutex<Option<String>>>,
    active_recording: Mutex<Option<RecordingHandle>>,
    /// Pre-allocated permanent SPSC consumer returned here whenever recording is idle,
    /// so the CoreAudio callback NEVER locks or drops its permanent `HeapProd<RecordedFrame>`.
    rec_consumer: Mutex<Option<HeapCons<RecordedFrame>>>,
    rec_producer: Mutex<Option<HeapProd<RecordedFrame>>>,
    shutdown: Arc<AtomicBool>,
    _feeder_thread: Option<JoinHandle<()>>,
    output_stream: Mutex<Option<Stream>>,
    // Held only until CoreAudio stream starts
    mic_consumers: Mutex<Option<[HeapCons<f32>; 2]>>,
    music_consumer: Mutex<Option<HeapCons<f32>>>,
}

// Safety: `cpal::Stream` is guarded by Mutex and only created/dropped on control threads.
unsafe impl Send for AudioEngine {}
unsafe impl Sync for AudioEngine {}

impl AudioEngine {
    /// Initializes the lock-free audio graph and returns `(AudioEngine, [Mic1Producer, Mic2Producer])`.
    /// Call `start_coreaudio_stream()` to attach to the macOS CoreAudio output device.
    pub fn new() -> (Self, [MicAudioProducer; 2]) {
        let state = Arc::new(AtomicEngineState::default());
        let loaded_track: Arc<Mutex<Option<Arc<LoadedStemTrack>>>> = Arc::new(Mutex::new(None));
        let shutdown = Arc::new(AtomicBool::new(false));

        // Lock-free SPSC ring buffers (all pre-allocated once at startup):
        // Music stereo ring buffer: ~200 ms headroom at 48 kHz (2 channels)
        let music_rb = HeapRb::<f32>::new(MASTER_SAMPLE_RATE as usize * 2 / 5);
        let (music_prod, music_cons) = music_rb.split();

        // Mic 1 & Mic 2 mono ring buffers: ~340 ms headroom each for smooth Continuity Wi-Fi bursts
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

        let engine = Self {
            state,
            loaded_track,
            current_track_id: Arc::new(Mutex::new(None)),
            active_recording: Mutex::new(None),
            rec_consumer: Mutex::new(Some(rec_cons)),
            rec_producer: Mutex::new(Some(rec_prod)),
            shutdown,
            _feeder_thread: feeder_thread,
            output_stream: Mutex::new(None),
            mic_consumers: Mutex::new(Some([mic1_cons, mic2_cons])),
            music_consumer: Mutex::new(Some(music_cons)),
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

    /// Starts the CoreAudio output stream at 48 kHz with `buffer_size` (64, 128, or 256 frames).
    /// Returns `Ok(())` on macOS hardware, or `Err` if no output audio device is available.
    pub fn start_coreaudio_stream(&self, buffer_size: AudioBufferSize) -> AudioResult<()> {
        self.state
            .buffer_size
            .store(buffer_size.as_u32(), Ordering::Relaxed);

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| AudioError::Device("No default CoreAudio output device found".into()))?;

        let config = StreamConfig {
            channels: 2,
            sample_rate: SampleRate(MASTER_SAMPLE_RATE),
            buffer_size: BufferSize::Fixed(buffer_size.as_u32()),
        };

        let mut music_cons = self
            .music_consumer
            .lock()
            .map_err(|_| AudioError::Device("Lock poisoned".into()))?
            .take()
            .ok_or_else(|| AudioError::Device("Audio stream already started".into()))?;

        let [mut mic1_cons, mut mic2_cons] = self
            .mic_consumers
            .lock()
            .map_err(|_| AudioError::Device("Lock poisoned".into()))?
            .take()
            .ok_or_else(|| AudioError::Device("Mic consumers already bound".into()))?;

        let mut rec_prod = self
            .rec_producer
            .lock()
            .map_err(|_| AudioError::Device("Lock poisoned".into()))?
            .take()
            .ok_or_else(|| AudioError::Device("Recording producer already bound".into()))?;

        let state = Arc::clone(&self.state);

        // Pre-allocate ALL DSP state BEFORE entering the CoreAudio real-time thread!
        let mut mic1_dsp = VocalDspChain::new(MASTER_SAMPLE_RATE as f32);
        let mut mic2_dsp = VocalDspChain::new(MASTER_SAMPLE_RATE as f32);
        let mut calibration_delay = CalibrationDelayLine::new();
        // ~10.6 ms cushion (512 samples) prevents zero-stuffed crackling on iPhone Continuity bursts
        let mut mic1_primed = false;
        let mut mic2_primed = false;
        let mut mic1_last = 0.0_f32;
        let mut mic2_last = 0.0_f32;

        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    // STRICT REAL-TIME CALLBACK:
                    // Zero heap allocations, zero deallocations, zero locks/try_lock, zero syscalls, zero unwrap().
                    if state.flush_music.swap(false, Ordering::AcqRel) {
                        while music_cons.try_pop().is_some() {}
                    }

                    let is_playing = state.is_playing.load(Ordering::Relaxed);
                    let effective_key = state.effective_key();
                    let mic1_params = state.mic_params[0].load();
                    let mic2_params = state.mic_params[1].load();
                    let is_recording = state.is_recording.load(Ordering::Relaxed);
                    let delay_ms = state
                        .latency_calibration_ms
                        .load(Ordering::Relaxed)
                        .min(250) as usize;
                    let delay_frames = (delay_ms * MASTER_SAMPLE_RATE as usize) / 1000;

                    if !mic1_primed && mic1_cons.occupied_len() >= 512 {
                        mic1_primed = true;
                    }
                    if !mic2_primed && mic2_cons.occupied_len() >= 512 {
                        mic2_primed = true;
                    }
                    // Keep input latency tight (< 40 ms = 1920 samples) if clock drifts slightly fast
                    if mic1_cons.occupied_len() > 2400 {
                        let drop_n = mic1_cons.occupied_len() - 512;
                        for _ in 0..drop_n {
                            let _ = mic1_cons.try_pop();
                        }
                    }
                    if mic2_cons.occupied_len() > 2400 {
                        let drop_n = mic2_cons.occupied_len() - 512;
                        for _ in 0..drop_n {
                            let _ = mic2_cons.try_pop();
                        }
                    }

                    for frame in data.chunks_exact_mut(2) {
                        let (raw_music_l, raw_music_r) = if is_playing {
                            (
                                music_cons.try_pop().unwrap_or(0.0),
                                music_cons.try_pop().unwrap_or(0.0),
                            )
                        } else {
                            (0.0, 0.0)
                        };

                        let (instru_l, instru_r) =
                            calibration_delay.process(raw_music_l, raw_music_r, delay_frames);

                        let raw_m1 = if mic1_primed {
                            if let Some(s) = mic1_cons.try_pop() {
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

                        let raw_m2 = if mic2_primed {
                            if let Some(s) = mic2_cons.try_pop() {
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

                        let (m1_l, m1_r) =
                            mic1_dsp.process_sample(raw_m1, effective_key, &mic1_params);
                        let (m2_l, m2_r) =
                            mic2_dsp.process_sample(raw_m2, effective_key, &mic2_params);

                        if is_recording {
                            let _ = rec_prod.try_push(RecordedFrame {
                                instru_l,
                                instru_r,
                                mic1_l: m1_l,
                                mic1_r: m1_r,
                                mic2_l: m2_l,
                                mic2_r: m2_r,
                            });
                        }

                        frame[0] = soft_limit(instru_l + m1_l + m2_l);
                        frame[1] = soft_limit(instru_r + m1_r + m2_r);
                    }

                    // Report exact ringbuf occupancy so lyrics clock aligns to speaker output
                    state
                        .music_buffered_frames
                        .store((music_cons.occupied_len() / 2) as u64, Ordering::Relaxed);

                    // Publish lock-free telemetry once per audio callback (~375 Hz at 128 frames)
                    state.mic_telemetry[0].store_dsp(mic1_dsp.telemetry(), mic1_params.muted);
                    state.mic_telemetry[1].store_dsp(mic2_dsp.telemetry(), mic2_params.muted);
                },
                |err| {
                    warn!("CoreAudio stream error: {err}");
                },
                None,
            )
            .map_err(|e| AudioError::Device(e.to_string()))?;

        stream
            .play()
            .map_err(|e| AudioError::Device(e.to_string()))?;

        if let Ok(mut guard) = self.output_stream.lock() {
            *guard = Some(stream);
        }

        info!(
            "CoreAudio stream started at {} Hz ({} frames buffer)",
            MASTER_SAMPLE_RATE,
            buffer_size.as_u32()
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
}
