use std::sync::{Arc, Mutex};

use karaok_core::{ActiveMicInputs, AudioInputDeviceInfo, MicChannelId};
use tracing::info;
#[cfg(not(target_os = "macos"))]
use tracing::warn;

use crate::atomics::AtomicEngineState;
use crate::engine::{MicAudioProducer, MASTER_SAMPLE_RATE};
use crate::error::{AudioError, AudioResult};

#[cfg(target_os = "macos")]
mod macos_av {
    use std::ffi::{c_char, c_void, CStr, CString};

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct KpAvDeviceInfo {
        pub name: [c_char; 256],
        pub sample_rate: u32,
        pub channels: u16,
        pub is_default: bool,
        pub is_iphone: bool,
    }

    pub type KpAvAudioCallback = extern "C" fn(
        samples: *const f32,
        frames: usize,
        sample_rate: u32,
        channels: u16,
        user_ctx: *mut c_void,
    );

    unsafe extern "C" {
        pub fn kp_av_is_clamshell_closed() -> bool;
        pub fn kp_av_mic_auth_status(request_if_needed: bool) -> i32;
        pub fn kp_av_list_input_devices(out_buf: *mut KpAvDeviceInfo, max_count: usize) -> usize;
        pub fn kp_av_start_capture(
            target_name: *const c_char,
            callback: KpAvAudioCallback,
            user_ctx: *mut c_void,
            out_sample_rate: *mut u32,
            out_channels: *mut u16,
            err_buf: *mut c_char,
            err_buf_len: usize,
        ) -> *mut c_void;
        pub fn kp_av_stop_capture(handle: *mut c_void);
    }

    pub fn cstr_to_string(buf: &[c_char]) -> String {
        // SAFETY: `kp_av_list_input_devices` always NUL-terminates `info.name` via `strlcpy`.
        unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }

    pub fn make_cstring(s: &str) -> Result<CString, String> {
        CString::new(s).map_err(|_| "Nom de périphérique audio invalide".to_string())
    }
}

/// Returns `true` if `name` refers to a built-in MacBook internal microphone across macOS locales
/// (English `MacBook Pro Microphone` / `Built-in Microphone`, French `Micro MacBook Pro`, German `MacBook-Mikrofon`, etc.).
pub fn is_built_in_macbook_mic(name: &str) -> bool {
    let l = name.to_lowercase();
    l.contains("built-in")
        || l.contains("interne")
        || (l.contains("macbook")
            && (l.contains("micro") || l.contains("mikro") || l.contains("mic")))
}

/// Returns `true` if the MacBook lid is currently closed (clamshell mode, which
/// hardware-mutes `Micro MacBook Pro` on Apple Silicon Macs).
pub fn is_macbook_clamshell_closed() -> bool {
    #[cfg(target_os = "macos")]
    unsafe {
        macos_av::kp_av_is_clamshell_closed()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Requests macOS microphone TCC permission if not yet determined, returning the OS status code
/// (`3` = authorized, `2` = denied, `1` = restricted, `0` = not determined).
pub fn ensure_microphone_permission(prompt_if_needed: bool) -> i32 {
    #[cfg(target_os = "macos")]
    unsafe {
        macos_av::kp_av_mic_auth_status(prompt_if_needed)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = prompt_if_needed;
        3
    }
}

/// Lists all audio input devices currently visible to macOS AVFoundation (Bluetooth headsets
/// like Nothing Headphone (a), Continuity/USB iPhone Microphone, USB headsets, built-in mic, etc.).
pub fn list_audio_input_devices() -> Vec<AudioInputDeviceInfo> {
    #[cfg(target_os = "macos")]
    {
        let mut raw = [macos_av::KpAvDeviceInfo {
            name: [0; 256],
            sample_rate: MASTER_SAMPLE_RATE,
            channels: 1,
            is_default: false,
            is_iphone: false,
        }; 32];

        let raw_count =
            unsafe { macos_av::kp_av_list_input_devices(raw.as_mut_ptr(), raw.len()) };
        let count = raw_count.min(raw.len());
        let mut out: Vec<AudioInputDeviceInfo> = Vec::with_capacity(count);
        for item in &raw[..count] {
            let name = macos_av::cstr_to_string(&item.name);
            if name.trim().is_empty() {
                continue;
            }
            if let Some(existing) = out.iter_mut().find(|d| d.name == name) {
                existing.sample_rate = existing.sample_rate.max(item.sample_rate);
                existing.channels = existing.channels.max(item.channels.max(1));
                existing.is_default |= item.is_default;
                existing.is_iphone |= item.is_iphone;
                continue;
            }
            out.push(AudioInputDeviceInfo {
                name,
                is_default: item.is_default,
                is_iphone: item.is_iphone,
                sample_rate: item.sample_rate.max(8000),
                channels: item.channels.max(1),
            });
        }

        out.sort_by(|a, b| {
            b.is_default
                .cmp(&a.is_default)
                .then_with(|| a.is_iphone.cmp(&b.is_iphone))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        out
    }

    #[cfg(not(target_os = "macos"))]
    {
        use cpal::traits::{DeviceTrait, HostTrait};
        let host = cpal::default_host();
        let Ok(devices) = host.input_devices() else {
            return Vec::new();
        };
        devices
            .filter_map(|d| {
                let name = d.description().ok()?.name().to_string();
                let cfg = d.default_input_config().ok()?;
                Some(AudioInputDeviceInfo {
                    name,
                    is_default: false,
                    is_iphone: false,
                    sample_rate: cfg.sample_rate(),
                    channels: cfg.channels(),
                })
            })
            .collect()
    }
}

/// Zero-allocation linear resampler inside the audio input callback converting
/// arbitrary device sample rates (e.g. 16 kHz Bluetooth HFP / 44.1 kHz / 48 kHz) cleanly to 48 kHz mono.
pub struct InlineMonoResampler {
    in_rate: u32,
    out_rate: u32,
    ratio: f64, // input_sr / output_sr
    frac: f64,
    prev_sample: f32,
}

impl InlineMonoResampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        let in_r = in_rate.max(8000);
        let out_r = out_rate.max(8000);
        Self {
            in_rate: in_r,
            out_rate: out_r,
            ratio: (in_r as f64) / (out_r as f64),
            frac: 0.0,
            prev_sample: 0.0,
        }
    }

    #[inline]
    pub fn set_input_rate(&mut self, in_rate: u32) {
        let in_r = in_rate.max(8000);
        if in_r != self.in_rate {
            self.in_rate = in_r;
            self.ratio = (in_r as f64) / (self.out_rate as f64);
        }
    }

    /// Pushes one mono input sample and emits 0..N resampled 48 kHz samples directly into `prod`.
    /// Strictly zero heap allocation.
    #[inline]
    pub fn push_and_emit(&mut self, sample: f32, prod: &mut MicAudioProducer) {
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

#[cfg(target_os = "macos")]
struct AvCaptureCallbackCtx {
    producer: Arc<Mutex<MicAudioProducer>>,
    resampler: InlineMonoResampler,
}

#[cfg(target_os = "macos")]
extern "C" fn on_av_audio_samples(
    samples: *const f32,
    frames: usize,
    sample_rate: u32,
    channels: u16,
    user_ctx: *mut std::ffi::c_void,
) {
    // STRICT REAL-TIME INPUT CALLBACK: zero allocations, zero syscalls, zero blocking locks.
    if samples.is_null() || frames == 0 || user_ctx.is_null() {
        return;
    }
    let ch = (channels as usize).max(1);
    let total_floats = frames.saturating_mul(ch);
    // SAFETY: `av_mic.m` passes a valid `frames * ch` aligned `f32` buffer and keeps `user_ctx`
    // alive until `kp_av_stop_capture` completes its `dispatch_sync` barrier.
    let data = unsafe { std::slice::from_raw_parts(samples, total_floats) };
    let ctx = unsafe { &mut *(user_ctx as *mut AvCaptureCallbackCtx) };
    ctx.resampler.set_input_rate(sample_rate);

    if let Ok(mut prod) = ctx.producer.try_lock() {
        if ch == 1 {
            for &s in data {
                ctx.resampler.push_and_emit(s, &mut prod);
            }
        } else {
            let inv_ch = 1.0 / ch as f32;
            for frame in data.chunks_exact(ch) {
                let mut sum = 0.0_f32;
                for &s in frame {
                    sum += s;
                }
                ctx.resampler.push_and_emit(sum * inv_ch, &mut prod);
            }
        }
    }
}

#[cfg(target_os = "macos")]
struct AvCaptureStream {
    handle: *mut std::ffi::c_void,
    ctx_ptr: *mut AvCaptureCallbackCtx,
}

#[cfg(target_os = "macos")]
unsafe impl Send for AvCaptureStream {}

#[cfg(target_os = "macos")]
impl Drop for AvCaptureStream {
    fn drop(&mut self) {
        // 1. Stop session & barrier-drain the serial dispatch queue BEFORE freeing `ctx_ptr`.
        if !self.handle.is_null() {
            unsafe { macos_av::kp_av_stop_capture(self.handle) };
            self.handle = std::ptr::null_mut();
        }
        if !self.ctx_ptr.is_null() {
            unsafe { drop(Box::from_raw(self.ctx_ptr)) };
            self.ctx_ptr = std::ptr::null_mut();
        }
    }
}

struct ActiveInputSlot {
    device_name: Option<String>,
    #[cfg(target_os = "macos")]
    stream: Option<AvCaptureStream>,
    #[cfg(not(target_os = "macos"))]
    stream: Option<cpal::Stream>,
    producer: Arc<Mutex<MicAudioProducer>>,
}

/// Manages native low-latency microphone capture streams for Mic 1 & Mic 2 via Apple `AVFoundation`
/// (`AVCaptureSession`), supporting Bluetooth headsets, USB mics, Continuity iPhone Mic, and Mac mics.
pub struct NativeMicInputManager {
    state: Arc<AtomicEngineState>,
    slots: [Mutex<ActiveInputSlot>; 2],
}

// Safety: active streams are guarded by `Mutex` and stopped/dropped strictly on control threads.
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

    /// Connects `channel` (`Mic1` or `Mic2`) to the named microphone device, or disconnects when `None`.
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
            info!("Disconnected input on Mic {}", channel.one_based());
            drop(slot);
            return Ok(self.active_inputs());
        };

        #[cfg(target_os = "macos")]
        {
            let auth = ensure_microphone_permission(true);
            if auth == 1 || auth == 2 {
                return Err(AudioError::Device(
                    "Autorisation Microphone refusée dans Réglages Système → Confidentialité et sécurité → Micro."
                        .into(),
                ));
            }

            let c_target =
                macos_av::make_cstring(target_name).map_err(AudioError::Device)?;
            let producer = Arc::clone(&slot.producer);
            let ctx = Box::new(AvCaptureCallbackCtx {
                producer,
                resampler: InlineMonoResampler::new(MASTER_SAMPLE_RATE, MASTER_SAMPLE_RATE),
            });
            let ctx_ptr = Box::into_raw(ctx);

            let mut out_sr: u32 = MASTER_SAMPLE_RATE;
            let mut out_ch: u16 = 1;
            let mut err_buf = [0 as std::ffi::c_char; 512];

            let handle = unsafe {
                macos_av::kp_av_start_capture(
                    c_target.as_ptr(),
                    on_av_audio_samples,
                    ctx_ptr as *mut std::ffi::c_void,
                    &mut out_sr,
                    &mut out_ch,
                    err_buf.as_mut_ptr(),
                    err_buf.len(),
                )
            };

            if handle.is_null() {
                // Capture failed: callback delegate already cleared + drained inside `av_mic.m`.
                unsafe { drop(Box::from_raw(ctx_ptr)) };
                let msg = macos_av::cstr_to_string(&err_buf);
                return Err(AudioError::Device(if msg.is_empty() {
                    format!("Impossible d'activer le micro « {target_name} ».")
                } else {
                    msg
                }));
            }

            slot.stream = Some(AvCaptureStream { handle, ctx_ptr });
            slot.device_name = Some(target_name.to_string());
            self.state
                .mic_telemetry(channel)
                .store_network(true, 3.0, 0.0);

            info!(
                "Connected AVFoundation input '{}' ({} Hz, {} ch) to Mic {}",
                target_name,
                out_sr,
                out_ch,
                channel.one_based()
            );
            drop(slot);
            Ok(self.active_inputs())
        }

        #[cfg(not(target_os = "macos"))]
        {
            use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
            use cpal::{SampleFormat, StreamConfig};
            let host = cpal::default_host();
            let device = host
                .input_devices()
                .map_err(|e| AudioError::Device(e.to_string()))?
                .find(|d| d.description().ok().map(|x| x.name().to_string()).as_deref() == Some(target_name))
                .ok_or_else(|| AudioError::Device(format!("Micro introuvable : « {target_name} »")))?;
            let cfg = device
                .default_input_config()
                .map_err(|e| AudioError::Device(e.to_string()))?;
            let fmt = cfg.sample_format();
            let config: StreamConfig = cfg.into();
            let ch = config.channels.max(1) as usize;
            let mut resampler = InlineMonoResampler::new(config.sample_rate, MASTER_SAMPLE_RATE);
            let producer = Arc::clone(&slot.producer);
            let stream = match fmt {
                SampleFormat::F32 => device
                    .build_input_stream(
                        config,
                        move |data: &[f32], _| {
                            if let Ok(mut p) = producer.try_lock() {
                                for frame in data.chunks_exact(ch) {
                                    let s = frame.iter().sum::<f32>() / ch as f32;
                                    resampler.push_and_emit(s, &mut p);
                                }
                            }
                        },
                        move |e| warn!("Input error: {e}"),
                        None,
                    )
                    .map_err(|e| AudioError::Device(e.to_string()))?,
                _ => {
                    return Err(AudioError::Device(format!("Unsupported format: {fmt:?}")));
                }
            };
            stream.play().map_err(|e| AudioError::Device(e.to_string()))?;
            slot.stream = Some(stream);
            slot.device_name = Some(target_name.to_string());
            self.state
                .mic_telemetry(channel)
                .store_network(true, 3.0, 0.0);
            drop(slot);
            Ok(self.active_inputs())
        }
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
        #[cfg(target_os = "macos")]
        {
            assert!(!outputs.is_empty(), "Expected CoreAudio outputs on macOS");
            assert!(!inputs.is_empty(), "Expected AVFoundation inputs on macOS");
        }
    }

    #[test]
    fn macbook_internal_mic_detection_is_locale_agnostic() {
        assert!(is_built_in_macbook_mic("Micro MacBook Pro"));
        assert!(is_built_in_macbook_mic("MacBook Pro Microphone"));
        assert!(is_built_in_macbook_mic("Built-in Microphone"));
        assert!(is_built_in_macbook_mic("MacBook-Mikrofon"));
        assert!(!is_built_in_macbook_mic("Nothing Headphone (a)"));
        assert!(!is_built_in_macbook_mic("GLASS5 mini"));
        assert!(!is_built_in_macbook_mic("Micro de « iPhone de Dany »"));
    }

    #[test]
    #[ignore = "Requires live macOS audio hardware probe"]
    fn probe_live_avfoundation_capture_all_mics() {
        let inputs = list_audio_input_devices();
        eprintln!("MacBook clamshell closed = {}", is_macbook_clamshell_closed());
        for dev in inputs {
            let (engine, mic_prods) = AudioEngine::new();
            let _ = engine.select_output_device(None);
            let mgr = NativeMicInputManager::new(engine.state(), mic_prods);
            match mgr.select_device(MicChannelId::Mic1, Some(&dev.name)) {
                Ok(_) => {
                    std::thread::sleep(std::time::Duration::from_millis(1200));
                    let t = engine.telemetry();
                    eprintln!(
                        "AVF '{}' ({} Hz, iphone={}): connected={}, vu_db={:.1}, peak={:.5}, gate_open={}",
                        dev.name,
                        dev.sample_rate,
                        dev.is_iphone,
                        t.mic1.connected,
                        t.mic1.vu_db,
                        t.mic1.peak,
                        t.mic1.gate_open
                    );
                }
                Err(e) => eprintln!("AVF '{}': ERR {e}", dev.name),
            }
        }
    }
}
