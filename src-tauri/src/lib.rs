mod commands;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use karaok_ai::LrclibClient;
use karaok_audio::{list_audio_input_devices, AudioEngine, NativeMicInputManager};
use karaok_core::{AudioBufferSize, MicChannelId};
use karaok_db::KaraokDatabase;
use tauri::{Emitter, Manager};
use tracing::{info, warn};

use crate::commands::AppState;

fn app_support_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("KaraokParty")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "karaok_party=info,karaok_audio=info".into()),
        )
        .try_init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let data_dir = app_support_dir();
            let stems_dir = data_dir.join("stems");
            let downloads_dir = data_dir.join("downloads");
            let recordings_dir = data_dir.join("recordings");
            let db_path = data_dir.join("karaok_party.sqlite");

            for dir in [&data_dir, &stems_dir, &downloads_dir, &recordings_dir] {
                let _ = std::fs::create_dir_all(dir);
            }

            let db = tauri::async_runtime::block_on(KaraokDatabase::connect_file(&db_path))
                .expect("Failed to initialize SQLite database");

            let (audio_engine, mic_producers) = AudioEngine::new();
            if let Err(err) = audio_engine.start_coreaudio_stream(AudioBufferSize::Frames128) {
                warn!("CoreAudio stream could not start automatically: {err}");
            }
            let audio = Arc::new(audio_engine);
            let mic_inputs = Arc::new(NativeMicInputManager::new(audio.state(), mic_producers));

            // Auto-connect any iPhone already connected to the Mac (Continuity / USB) to Mic 1!
            let devices = list_audio_input_devices();
            if let Some(iphone) = devices.iter().find(|d| d.is_iphone) {
                match mic_inputs.select_device(MicChannelId::Mic1, Some(&iphone.name)) {
                    Ok(_) => info!("Auto-connected iPhone microphone '{}' to Mic 1", iphone.name),
                    Err(err) => warn!("Could not auto-connect iPhone '{}': {err}", iphone.name),
                }
            }

            // 60 Hz real-time telemetry emitter synchronized across Main + TV windows
            let telemetry_audio = Arc::clone(&audio);
            let app_handle_tick = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(16));
                loop {
                    interval.tick().await;
                    let snap = telemetry_audio.telemetry();
                    let _ = app_handle_tick.emit("karaok-telemetry", &snap);
                }
            });

            app.manage(AppState {
                data_dir,
                stems_dir,
                downloads_dir,
                recordings_dir,
                db,
                audio,
                mic_inputs,
                lrclib: LrclibClient::new(),
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_tracks,
            commands::import_local_file,
            commands::import_youtube_url,
            commands::import_spotify_playlist,
            commands::toggle_favorite,
            commands::delete_track,
            commands::separate_track_stems,
            commands::load_and_play_track,
            commands::set_playing,
            commands::seek_ms,
            commands::rewind_5s,
            commands::set_transpose,
            commands::set_tempo_pct,
            commands::set_stem_mix,
            commands::set_ab_loop,
            commands::set_latency_calibration_ms,
            commands::get_telemetry,
            commands::list_audio_input_devices,
            commands::select_mic_input_device,
            commands::get_active_mic_inputs,
            commands::set_mic_preset,
            commands::set_mic_params,
            commands::get_mic_params,
            commands::get_lyrics,
            commands::save_lyrics,
            commands::save_plain_lyrics_for_track,
            commands::fetch_lrclib_lyrics,
            commands::open_google_lyrics_search,
            commands::auto_align_lyrics_whisper,
            commands::shift_lyrics_offset,
            commands::list_queue,
            commands::enqueue_track,
            commands::advance_queue,
            commands::remove_queue_item,
            commands::start_recording,
            commands::stop_recording,
            commands::list_recordings,
            commands::preview_recording_mix,
            commands::export_recording_mix,
            commands::toggle_tv_window,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Karaok Party Tauri application");
}
