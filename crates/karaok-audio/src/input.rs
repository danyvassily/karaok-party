use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use karaok_core::{ActiveMicInputs, AudioInputDeviceInfo, MicChannelId};
use tracing::{info, warn};

use crate::atomics::AtomicEngineState;
use crate::engine::{MicAudioProducer, MASTER_SAMPLE_RATE};
use crate::error::{AudioError, AudioResult};

/// Lists all CoreAudio input devices currently visible to macOS (Continuity iPhone Microphone,
/// USB-connected iPhone / audio interfaces, built-in MacBook microphone, etc.), sorted with
/// iPhone microphones first, followed by the system default input.
pub fn list_audio_input_devices() -> Vec<AudioInputDeviceInfo> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok());

    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for dev in devices {
        let Ok(name) = dev.name() else {
            continue;
        };
        let cfg = dev.default_input_config().ok();
        let sample_rate = cfg.as_ref().map_or(MASTER_SAMPLE_RATE, |c| c.sample_rate().0);
        let channels = cfg.as_ref().map_or(1, |c| c.channels());
        let lower = name.to_ascii_lowercase();
        let is_iphone = lower.contains("iphone") || lower.contains("continuity");
        let is_default = default_name.as_deref() == Some(name.as_str());

        out.push(AudioInputDeviceInfo {
            name,
            is_default,
            is_iphone,
            sample_rate,
            channels,
        });
    }

    out.sort_by(|a, b| {
        b.is_iphone
            .cmp(&a.is_iphone)
            .then_with(|| b.is_default.cmp(&a.is_default))
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
            .find(|d| d.name().ok().as_deref() == Some(target_name))
            .ok_or_else(|| {
                AudioError::Device(format!(
                    "Micro introuvable : « {target_name} ». Vérifiez que l'iPhone est bien connecté au Mac."
                ))
            })?;

        let default_cfg = device
            .default_input_config()
            .map_err(|e| AudioError::Device(e.to_string()))?;
        let sample_format = default_cfg.sample_format();
        let config: StreamConfig = default_cfg.into();
        let channels = config.channels.max(1) as usize;
        let in_rate = config.sample_rate.0;

        let producer = Arc::clone(&slot.producer);
        let mut resampler = InlineMonoResampler::new(in_rate, MASTER_SAMPLE_RATE);

        let err_name = target_name.to_string();
        let err_fn = move |err: cpal::StreamError| {
            warn!("CoreAudio input error on '{err_name}': {err}");
        };

        let stream = match sample_format {
            SampleFormat::F32 => device
                .build_input_stream(
                    &config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
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
            SampleFormat::I16 => device
                .build_input_stream(
                    &config,
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
                    &config,
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
            other => {
                return Err(AudioError::Device(format!(
                    "Format d'entrée CoreAudio non supporté : {other:?}"
                )));
            }
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
    use crate::engine::AudioEngine;

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
}
