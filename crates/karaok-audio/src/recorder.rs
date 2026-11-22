use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use chrono::Utc;
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use karaok_core::RecordingEntry;
use karaok_dsp::soft_limit;
use ringbuf::traits::Consumer;
use ringbuf::HeapCons;
use uuid::Uuid;

use crate::engine::MASTER_SAMPLE_RATE;
use crate::error::{AudioError, AudioResult};

/// Single synchronized multitrack frame tapped from the real-time CoreAudio callback.
#[derive(Debug, Clone, Copy, Default)]
pub struct RecordedFrame {
    pub instru_l: f32,
    pub instru_r: f32,
    pub mic1_l: f32,
    pub mic1_r: f32,
    pub mic2_l: f32,
    pub mic2_r: f32,
}

/// Active background WAV writer consuming frames lock-free from the permanent CoreAudio
/// recording ring buffer, returning ownership of the consumer upon `stop()` so the RT thread
/// never drops or reallocates the ring buffer.
pub struct RecordingHandle {
    id: String,
    track_id: Option<String>,
    title: String,
    created_at: String,
    instrumental_path: PathBuf,
    mic1_path: PathBuf,
    mic2_path: PathBuf,
    stop_flag: Arc<AtomicBool>,
    frames_written: Arc<std::sync::atomic::AtomicU64>,
    worker: Option<JoinHandle<AudioResult<HeapCons<RecordedFrame>>>>,
}

impl RecordingHandle {
    pub fn start(
        recordings_dir: &Path,
        track_id: Option<String>,
        title: String,
        mut consumer: HeapCons<RecordedFrame>,
    ) -> AudioResult<Self> {
        // Drain any stale pre-start frames before recording begins
        while consumer.try_pop().is_some() {}

        let id = Uuid::new_v4().to_string();
        let session_dir = recordings_dir.join(&id);
        std::fs::create_dir_all(&session_dir)?;

        let instrumental_path = session_dir.join("instrumental.wav");
        let mic1_path = session_dir.join("mic1.wav");
        let mic2_path = session_dir.join("mic2.wav");

        let spec = WavSpec {
            channels: 2,
            sample_rate: MASTER_SAMPLE_RATE,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };

        let mut w_instru = WavWriter::create(&instrumental_path, spec)?;
        let mut w_mic1 = WavWriter::create(&mic1_path, spec)?;
        let mut w_mic2 = WavWriter::create(&mic2_path, spec)?;

        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop_flag);
        let frames_written = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let frames_clone = Arc::clone(&frames_written);

        let worker = thread::Builder::new()
            .name("karaok-wav-recorder".into())
            .spawn(move || -> AudioResult<HeapCons<RecordedFrame>> {
                let mut count = 0_u64;
                loop {
                    let mut drained = false;
                    while let Some(f) = consumer.try_pop() {
                        drained = true;
                        w_instru.write_sample(f.instru_l)?;
                        w_instru.write_sample(f.instru_r)?;
                        w_mic1.write_sample(f.mic1_l)?;
                        w_mic1.write_sample(f.mic1_r)?;
                        w_mic2.write_sample(f.mic2_l)?;
                        w_mic2.write_sample(f.mic2_r)?;
                        count += 1;
                    }
                    frames_clone.store(count, Ordering::Relaxed);
                    if stop_clone.load(Ordering::Acquire) && !drained {
                        break;
                    }
                    if !drained {
                        thread::sleep(Duration::from_millis(5));
                    }
                }
                w_instru.finalize()?;
                w_mic1.finalize()?;
                w_mic2.finalize()?;
                Ok(consumer)
            })?;

        Ok(Self {
            id,
            track_id,
            title,
            created_at: Utc::now().to_rfc3339(),
            instrumental_path,
            mic1_path,
            mic2_path,
            stop_flag,
            frames_written,
            worker: Some(worker),
        })
    }

    /// Stops recording, finalizes all three WAV tracks, and returns `(RecordingEntry, HeapCons)`
    /// so the permanent SPSC consumer returns cleanly to `AudioEngine`.
    pub fn stop(mut self) -> AudioResult<(RecordingEntry, HeapCons<RecordedFrame>)> {
        self.stop_flag.store(true, Ordering::Release);
        let consumer = self
            .worker
            .take()
            .ok_or_else(|| AudioError::Recorder("Recorder worker already stopped".into()))?
            .join()
            .map_err(|_| AudioError::Recorder("Recorder thread panicked".into()))??;

        let frames = self.frames_written.load(Ordering::Relaxed);
        let duration_ms = (frames * 1000) / MASTER_SAMPLE_RATE as u64;

        Ok((
            RecordingEntry {
                id: self.id,
                track_id: self.track_id,
                title: self.title,
                created_at: self.created_at,
                duration_ms,
                instrumental_path: self.instrumental_path.to_string_lossy().into_owned(),
                mic1_path: self.mic1_path.to_string_lossy().into_owned(),
                mic2_path: self.mic2_path.to_string_lossy().into_owned(),
                mix_vocal_balance: 0.55,
                exported_path: None,
            },
            consumer,
        ))
    }
}

/// Mixes recorded instrumental + Mic 1 + Mic 2 tracks with SingSnap-style adjustable
/// `vocal_balance` (`0.0` = all music, `0.5` = balanced, `1.0` = vocals prominent) and exports
/// a mastered 16-bit PCM WAV file (and optional MP3 if `output_path` ends with `.mp3` and `ffmpeg` is installed).
pub fn mix_and_export_recording(
    instrumental_path: &Path,
    mic1_path: &Path,
    mic2_path: &Path,
    vocal_balance: f32,
    output_path: &Path,
) -> AudioResult<PathBuf> {
    let mut r_instru = WavReader::open(instrumental_path)?;
    let mut r_mic1 = WavReader::open(mic1_path)?;
    let mut r_mic2 = WavReader::open(mic2_path)?;

    let instru: Vec<f32> = r_instru.samples::<f32>().filter_map(Result::ok).collect();
    let mic1: Vec<f32> = r_mic1.samples::<f32>().filter_map(Result::ok).collect();
    let mic2: Vec<f32> = r_mic2.samples::<f32>().filter_map(Result::ok).collect();

    let len = instru.len().max(mic1.len()).max(mic2.len());
    let bal = vocal_balance.clamp(0.0, 1.0);
    let music_gain = (1.0 - bal * 0.55).clamp(0.25, 1.15);
    let vocal_gain = (0.35 + bal * 1.15).clamp(0.25, 1.5);

    let wants_mp3 = output_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp3"));

    let wav_target = if wants_mp3 {
        output_path.with_extension("tmp.wav")
    } else {
        output_path.to_path_buf()
    };

    if let Some(parent) = wav_target.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let spec = WavSpec {
        channels: 2,
        sample_rate: MASTER_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(&wav_target, spec)?;

    for i in 0..len {
        let s_i = instru.get(i).copied().unwrap_or(0.0);
        let s_m1 = mic1.get(i).copied().unwrap_or(0.0);
        let s_m2 = mic2.get(i).copied().unwrap_or(0.0);
        let mixed = soft_limit(s_i * music_gain + (s_m1 + s_m2) * vocal_gain);
        let pcm16 = (mixed.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        writer.write_sample(pcm16)?;
    }
    writer.finalize()?;

    if wants_mp3 {
        let status = std::process::Command::new("ffmpeg")
            .arg("-y")
            .arg("-i")
            .arg(&wav_target)
            .arg("-codec:a")
            .arg("libmp3lame")
            .arg("-b:a")
            .arg("256k")
            .arg(output_path)
            .status();

        match status {
            Ok(st) if st.success() => {
                let _ = std::fs::remove_file(&wav_target);
                Ok(output_path.to_path_buf())
            }
            _ => {
                // Graceful fallback to WAV if ffmpeg is unavailable
                let fallback_wav = output_path.with_extension("wav");
                let _ = std::fs::rename(&wav_target, &fallback_wav);
                Ok(fallback_wav)
            }
        }
    } else {
        Ok(wav_target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::traits::{Producer, Split};
    use ringbuf::HeapRb;

    #[test]
    fn records_multitrack_wav_and_exports_balanced_mix() {
        let tmp = std::env::temp_dir().join(format!("karaok-rec-test-{}", Uuid::new_v4()));
        let rb = HeapRb::<RecordedFrame>::new(4096);
        let (mut prod, cons) = rb.split();

        let handle = RecordingHandle::start(&tmp, Some("trk-1".into()), "Take 1".into(), cons).unwrap();
        for i in 0..1024 {
            let phase = i as f32 * 0.05;
            let _ = prod.try_push(RecordedFrame {
                instru_l: 0.3 * phase.sin(),
                instru_r: 0.3 * phase.sin(),
                mic1_l: 0.4 * (phase * 2.0).sin(),
                mic1_r: 0.4 * (phase * 2.0).sin(),
                mic2_l: 0.0,
                mic2_r: 0.0,
            });
        }
        let (entry, _returned_cons) = handle.stop().unwrap();
        assert!(Path::new(&entry.instrumental_path).exists());
        assert!(Path::new(&entry.mic1_path).exists());
        assert!(Path::new(&entry.mic2_path).exists());

        let out_wav = tmp.join("final_mix.wav");
        let exported = mix_and_export_recording(
            Path::new(&entry.instrumental_path),
            Path::new(&entry.mic1_path),
            Path::new(&entry.mic2_path),
            0.7,
            &out_wav,
        )
        .unwrap();
        assert!(exported.exists());
        let reader = WavReader::open(&exported).unwrap();
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_rate, MASTER_SAMPLE_RATE);
        assert_eq!(reader.len(), 2048);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
