mod commands;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use karaok_ai::LrclibClient;
use karaok_audio::AudioEngine;
use karaok_core::{AudioBufferSize, QueueMode};
use karaok_db::KaraokDatabase;
use karaok_mic_server::server::DEFAULT_MIC_SERVER_PORT;
use karaok_mic_server::MicServer;
use tauri::{Emitter, Manager};
use tokio::sync::Mutex;
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
                .unwrap_or_else(|_| "karaok_party=info,karaok_audio=info,karaok_mic_server=info".into()),
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
            let certs_dir = data_dir.join("certs");
            let db_path = data_dir.join("karaok_party.sqlite");

            for dir in [&data_dir, &stems_dir, &downloads_dir, &recordings_dir, &certs_dir] {
                let _ = std::fs::create_dir_all(dir);
            }

            let db = tauri::async_runtime::block_on(KaraokDatabase::connect_file(&db_path))
                .expect("Failed to initialize SQLite database");

            let (audio_engine, mic_producers) = AudioEngine::new();
            if let Err(err) = audio_engine.start_coreaudio_stream(AudioBufferSize::Frames128) {
                warn!("CoreAudio stream could not start automatically: {err}");
            }
            let audio = Arc::new(audio_engine);
            let mic_server_slot = Arc::new(Mutex::new(None));

            // Start local HTTPS/WSS iPhone mic server + remote queue listener
            let mic_slot_clone = Arc::clone(&mic_server_slot);
            let audio_state_clone = audio.state();
            let db_clone = db.clone();
            let app_handle_mic = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                match MicServer::start(
                    &certs_dir,
                    DEFAULT_MIC_SERVER_PORT,
                    audio_state_clone,
                    mic_producers,
                )
                .await
                {
                    Ok((server, mut queue_rx)) => {
                        info!("Mic server ready: {}", server.pairing_info().server_url);
                        *mic_slot_clone.lock().await = Some(server);
                        while let Some(req) = queue_rx.recv().await {
                            // Auto-match local library track and add to Party Queue
                            if let Ok(matches) = db_clone.list_tracks(Some(&req.query)).await {
                                if let Some(first) = matches.first() {
                                    let _ = db_clone
                                        .enqueue_track(
                                            &first.id,
                                            QueueMode::Solo,
                                            &req.singer_name,
                                            None,
                                            &format!("mic{}", req.mic.one_based()),
                                        )
                                        .await;
                                    let _ = app_handle_mic.emit("queue-updated", ());
                                } else {
                                    let _ = app_handle_mic.emit("remote-queue-query", &req.query);
                                }
                            }
                        }
                    }
                    Err(err) => warn!("Failed to start iPhone Mic HTTPS server: {err}"),
                }
            });

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
                mic_server: mic_server_slot,
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
            commands::get_mic_pairing_info,
            commands::set_mic_preset,
            commands::set_mic_params,
            commands::get_mic_params,
            commands::get_lyrics,
            commands::save_lyrics,
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
