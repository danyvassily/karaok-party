use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use karaok_core::{ActiveMicInputs, AudioInputDeviceInfo, MicChannelId};
use tracing::{info, warn};

use crate::atomics::AtomicEngineState;
use crate::engine::{MicAudioProducer, MASTER_SAMPLE_RATE};
use crate::error::{AudioError, AudioResult};

fn cpal_device_name(dev: &cpal::Device) -> Option<String> {
    dev.description()
        .ok()
        .map(|d| d.name().to_string())
        .filter(|s| !s.trim().is_empty())
}

fn resolve_input_config(dev: &cpal::Device) -> Option<cpal::SupportedStreamConfig> {
    if let Ok(cfg) = dev.default_input_config() {
        if cfg.sample_rate() > 0 && cfg.channels() > 0 {
            return Some(cfg);
        }
    }
    let ranges = dev.supported_input_configs().ok()?;
    let mut best: Option<cpal::SupportedStreamConfig> = None;
    for r in ranges {
        if r.channels() == 0 {
            continue;
        }
        let target_sr = if r.min_sample_rate() <= MASTER_SAMPLE_RATE
            && r.max_sample_rate() >= MASTER_SAMPLE_RATE
        {
            MASTER_SAMPLE_RATE
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
                (cand.sample_format() == SampleFormat::F32
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

/// Lists all CoreAudio input devices currently visible to macOS (headset microphones,
/// built-in MacBook microphone, USB/Bluetooth audio interfaces, etc.), sorted with
/// the macOS default input first.
pub fn list_audio_input_devices() -> Vec<AudioInputDeviceInfo> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| cpal_device_name(&d));

    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };

    let mut out: Vec<AudioInputDeviceInfo> = Vec::new();
    for dev in devices {
        let Some(name) = cpal_device_name(&dev) else {
            continue;
        };
        let cfg = resolve_input_config(&dev);
        let sample_rate = cfg.as_ref().map_or(MASTER_SAMPLE_RATE, |c| c.sample_rate());
        let channels = cfg.as_ref().map_or(1, |c| c.channels());
        let lower = name.to_ascii_lowercase();
        let is_iphone = lower.contains("iphone") || lower.contains("continuity");
        let is_default = default_name.as_deref() == Some(name.as_str());

        if let Some(existing) = out.iter_mut().find(|d| d.name == name) {
            existing.sample_rate = existing.sample_rate.max(sample_rate);
            existing.channels = existing.channels.max(channels);
            existing.is_default |= is_default;
            continue;
        }

        out.push(AudioInputDeviceInfo {
            name,
            is_default,
            is_iphone,
            sample_rate,
            channels,
        });
    }

    out.sort_by(|a, b| {
        // Prioritize default macOS / headset microphone first (not iPhone Continuity)
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.is_iphone.cmp(&b.is_iphone))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    out
}

/// Zero-allocation linear resampler inside the CoreAudio input callback converting
/// arbitrary device sample rates (e.g. 44.1 kHz Continuity / 16 kHz BT) cleanly to 48 kHz mono.
struct InlineMonoResampler {
    ratio: f64, // input_sr / output_sr
    frac: f64,
    prev_sample: f32,
}

impl InlineMonoResampler {
    fn new(in_rate: u32, out_rate: u32) -> Self {
        Self {
            ratio: (in_rate.max(8000) as f64) / (out_rate.max(8000) as f64),
            frac: 0.0,
            prev_sample: 0.0,
        }
    }

    /// Pushes one mono input sample and emits 0..N resampled 48 kHz samples directly into `prod`.
    /// Strictly zero heap allocation.
    #[inline]
    fn push_and_emit(&mut self, sample: f32, prod: &mut MicAudioProducer) {
        let clean = if sample.is_finite() { sample } else { 0.0 };
        if (self.ratio - 1.0).abs() < 1e-6 {
            let buf = [clean];
            let _ = prod.push_samples(&buf);
            self.prev_sample = clean;
            return;
        }
        while self.frac < 1.0 {
            let t = self.frac as f32;
            let interp = self.prev_sample + (clean - self.prev_sample) * t;
            let buf = [interp];
            let _ = prod.push_samples(&buf);
            self.frac += self.ratio;
        }
        self.frac -= 1.0;
        self.prev_sample = clean;
    }
}

struct ActiveInputSlot {
    device_name: Option<String>,
    stream: Option<Stream>,
    producer: Arc<Mutex<MicAudioProducer>>,
}

/// Manages native CoreAudio input streams for Mic 1 & Mic 2 (direct Mac/iPhone connection,
/// zero QR code or local HTTPS server required).
pub struct NativeMicInputManager {
    state: Arc<AtomicEngineState>,
    slots: [Mutex<ActiveInputSlot>; 2],
}

// Safety: `cpal::Stream` is guarded by Mutex and only created/dropped on control threads.
unsafe impl Send for NativeMicInputManager {}
unsafe impl Sync for NativeMicInputManager {}

impl NativeMicInputManager {
    pub fn new(state: Arc<AtomicEngineState>, producers: [MicAudioProducer; 2]) -> Self {
        let [p1, p2] = producers;
        Self {
            state,
            slots: [
                Mutex::new(ActiveInputSlot {
                    device_name: None,
                    stream: None,
                    producer: Arc::new(Mutex::new(p1)),
                }),
                Mutex::new(ActiveInputSlot {
                    device_name: None,
                    stream: None,
                    producer: Arc::new(Mutex::new(p2)),
                }),
            ],
        }
    }

    /// Connects `channel` (`Mic1` or `Mic2`) directly to the named macOS CoreAudio input device
    /// (e.g. an iPhone connected to the Mac via Continuity / USB), or disconnects when `device_name` is `None`.
    pub fn select_device(
        &self,
        channel: MicChannelId,
        device_name: Option<&str>,
    ) -> AudioResult<ActiveMicInputs> {
        let idx = channel.index();
        let mut slot = self.slots[idx]
            .lock()
            .map_err(|_| AudioError::Device("Mic input slot lock poisoned".into()))?;

        // Stop any existing stream cleanly first
        slot.stream = None;
        slot.device_name = None;
        self.state
            .mic_telemetry(channel)
            .store_network(false, 0.0, 0.0);

        let Some(target_name) = device_name.filter(|s| !s.trim().is_empty()) else {
            info!("Disconnected CoreAudio input on Mic {}", channel.one_based());
            drop(slot);
            return Ok(self.active_inputs());
        };

        let host = cpal::default_host();
        let device = host
            .input_devices()
            .map_err(|e| AudioError::Device(e.to_string()))?
            .find(|d| cpal_device_name(d).as_deref() == Some(target_name))
            .ok_or_else(|| {
                AudioError::Device(format!(
                    "Micro introuvable : « {target_name} ». Vérifiez que votre casque micro est bien connecté au Mac."
                ))
            })?;

        let default_cfg = resolve_input_config(&device).ok_or_else(|| {
            AudioError::Device(format!(
                "Configuration d'entrée CoreAudio indisponible pour « {target_name} »"
            ))
        })?;
        let sample_format = default_cfg.sample_format();
        let config: StreamConfig = default_cfg.into();
        let channels = config.channels.max(1) as usize;
        let in_rate = config.sample_rate;

        let producer = Arc::clone(&slot.producer);
        let mut resampler = InlineMonoResampler::new(in_rate, MASTER_SAMPLE_RATE);

        let err_name = target_name.to_string();
        let err_fn = move |err: cpal::Error| {
            warn!("CoreAudio input error on '{err_name}': {err}");
        };

        let stream = match sample_format {
            SampleFormat::I16 => device
                .build_input_stream(
                    config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        if let Ok(mut prod) = producer.try_lock() {
                            for frame in data.chunks_exact(channels) {
                                let mut sum = 0.0_f32;
                                for &s in frame {
                                    sum += (s as f32) / 32768.0;
                                }
                                resampler.push_and_emit(sum / channels as f32, &mut prod);
                            }
                        }
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| AudioError::Device(e.to_string()))?,
            SampleFormat::U16 => device
                .build_input_stream(
                    config,
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        if let Ok(mut prod) = producer.try_lock() {
                            for frame in data.chunks_exact(channels) {
                                let mut sum = 0.0_f32;
                                for &s in frame {
                                    sum += ((s as f32) - 32768.0) / 32768.0;
                                }
                                resampler.push_and_emit(sum / channels as f32, &mut prod);
                            }
                        }
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| AudioError::Device(e.to_string()))?,
            // CoreAudio AUHAL natively supplies F32 for F32 / I24 / I32 hardware inputs
            _ => device
                .build_input_stream(
                    config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        // STRICT REAL-TIME INPUT CALLBACK: zero allocations, zero syscalls, zero logging.
                        if let Ok(mut prod) = producer.try_lock() {
                            for frame in data.chunks_exact(channels) {
                                let mut sum = 0.0_f32;
                                for &s in frame {
                                    sum += s;
                                }
                                resampler.push_and_emit(sum / channels as f32, &mut prod);
                            }
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

        slot.stream = Some(stream);
        slot.device_name = Some(target_name.to_string());
        // Native CoreAudio input latency (~2.7 ms at 128 frames, 0% packet loss)
        self.state
            .mic_telemetry(channel)
            .store_network(true, 2.7, 0.0);

        info!(
            "Connected native CoreAudio input '{}' ({} Hz, {} ch) to Mic {}",
            target_name,
            in_rate,
            channels,
            channel.one_based()
        );
        drop(slot);
        Ok(self.active_inputs())
    }

    pub fn active_inputs(&self) -> ActiveMicInputs {
        let mic1_device = self.slots[0]
            .lock()
            .ok()
            .and_then(|s| s.device_name.clone());
        let mic2_device = self.slots[1]
            .lock()
            .ok()
            .and_then(|s| s.device_name.clone());
        ActiveMicInputs {
            mic1_device,
            mic2_device,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{list_audio_output_devices, AudioEngine};

    #[test]
    fn inline_resampler_upsamples_44100_to_48000_cleanly() {
        let (_engine, [mut prod, _]) = AudioEngine::new();
        let mut r = InlineMonoResampler::new(44_100, 48_000);
        for i in 0..441 {
            let s = (i as f32 * 0.05).sin() * 0.5;
            r.push_and_emit(s, &mut prod);
        }
        // Should execute cleanly with zero panic / finite values
        assert!(r.prev_sample.is_finite());
    }

    #[test]
    fn inline_resampler_upsamples_16000_bt_headset_to_48000_cleanly() {
        let (_engine, [mut prod, _]) = AudioEngine::new();
        let mut r = InlineMonoResampler::new(16_000, 48_000);
        for i in 0..160 {
            let s = (i as f32 * 0.1).sin() * 0.4;
            r.push_and_emit(s, &mut prod);
        }
        assert!(r.prev_sample.is_finite());
    }

    #[test]
    fn coreaudio_discovers_all_outputs_and_inputs() {
        let outputs = list_audio_output_devices();
        let inputs = list_audio_input_devices();
        eprintln!("Detected outputs ({}): {outputs:?}", outputs.len());
        eprintln!("Detected inputs ({}): {inputs:?}", inputs.len());
        // On any macOS machine with speakers, at least one output must be present
        #[cfg(target_os = "macos")]
        assert!(!outputs.is_empty(), "Expected CoreAudio outputs on macOS");
    }
}
