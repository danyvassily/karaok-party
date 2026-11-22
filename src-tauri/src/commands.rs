use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use karaok_ai::{
    align_plain_lyrics_to_vocals, build_google_lyrics_url, compute_file_sha256,
    import_spotify_playlist_metadata, import_youtube_audio, separate_track_chunked, LrclibClient,
    PlaylistTrackCandidate, WhisperAlignConfig,
};
use karaok_audio::{decode_audio_to_48khz_stereo, mix_and_export_recording, AudioEngine};
use karaok_core::{
    DemucsModel, LoopRegion, LyricsDocument, LyricsFormat, MicChannelId, MicPairingInfo,
    MusicalKey, PlaybackTelemetry, QueueItem, QueueMode, RecordingEntry, StemMix, StemStatus,
    Track, TrackSource, VocalDspParams, VocalPreset,
};
use karaok_db::KaraokDatabase;
use karaok_mic_server::MicServer;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::Mutex;
use tracing::{info, warn};
use uuid::Uuid;

/// Shared application state managed by Tauri v2.
pub struct AppState {
    #[allow(dead_code)]
    pub data_dir: PathBuf,
    pub stems_dir: PathBuf,
    pub downloads_dir: PathBuf,
    pub recordings_dir: PathBuf,
    pub db: KaraokDatabase,
    pub audio: Arc<AudioEngine>,
    pub mic_server: Arc<Mutex<Option<MicServer>>>,
    pub lrclib: LrclibClient,
}

#[tauri::command]
pub async fn list_tracks(
    search: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Track>, String> {
    state
        .db
        .list_tracks(search.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn import_local_file(
    path: String,
    title: Option<String>,
    artist: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Track, String> {
    let src_path = PathBuf::from(&path);
    if !src_path.exists() {
        return Err(format!("Fichier introuvable : {path}"));
    }

    let path_clone = src_path.clone();
    let (content_hash, duration_ms) = tokio::task::spawn_blocking(move || {
        let hash = compute_file_sha256(&path_clone).map_err(|e| e.to_string())?;
        let decoded = decode_audio_to_48khz_stereo(&path_clone).map_err(|e| e.to_string())?;
        Ok::<_, String>((hash, decoded.duration_ms()))
    })
    .await
    .map_err(|e| e.to_string())??;

    let file_stem = src_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Sans titre")
        .to_string();
    let (inferred_artist, inferred_title) = match file_stem.split_once(" - ") {
        Some((a, t)) => (a.trim().to_string(), t.trim().to_string()),
        None => ("Artiste inconnu".to_string(), file_stem),
    };

    let track_id = Uuid::new_v4().to_string();
    let track = Track {
        id: track_id.clone(),
        title: title.unwrap_or(inferred_title),
        artist: artist.unwrap_or(inferred_artist),
        album: None,
        duration_ms,
        source_type: TrackSource::Local,
        source_path: path,
        source_url: None,
        content_hash,
        musical_key: MusicalKey::default(),
        bpm: None,
        favorite_transpose: 0,
        is_favorite: false,
        stem_status: StemStatus::Pending,
        stem_progress: 0,
        stem_model: DemucsModel::Htdemucs,
        stems_dir: None,
        has_lyrics: false,
        has_word_sync: false,
        created_at: Utc::now().to_rfc3339(),
    };

    state.db.upsert_track(&track).await.map_err(|e| e.to_string())?;

    // Best-effort automatic multi-source Web/LRCLIB fetch + AI alignment in background
    let db = state.db.clone();
    let lrclib = state.lrclib.clone();
    let t_title = track.title.clone();
    let t_artist = track.artist.clone();
    let t_dur = Some(track.duration_ms / 1000);
    let tid = track_id.clone();
    let app_clone = app.clone();
    tokio::spawn(async move {
        if let Ok(raw_doc) = lrclib.fetch_best_lyrics(&t_title, &t_artist, t_dur).await {
            let doc = auto_align_if_plain(raw_doc, &tid, &db).await;
            if db.save_lyrics(&tid, &doc).await.is_ok() {
                let _ = app_clone.emit("lyrics-updated", &tid);
            }
        }
    });

    Ok(track)
}

#[tauri::command]
pub async fn import_youtube_url(
    url: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Track, String> {
    let downloads_dir = state.downloads_dir.clone();
    let imported = tokio::task::spawn_blocking(move || import_youtube_audio(&url, &downloads_dir))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    let path_str = imported.audio_path.to_string_lossy().into_owned();
    let hash_path = imported.audio_path.clone();
    let content_hash = tokio::task::spawn_blocking(move || compute_file_sha256(&hash_path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    let track = Track {
        id: Uuid::new_v4().to_string(),
        title: imported.title,
        artist: imported.artist,
        album: Some("YouTube".into()),
        duration_ms: imported.duration_ms,
        source_type: TrackSource::Youtube,
        source_path: path_str,
        source_url: Some(imported.source_url),
        content_hash,
        musical_key: MusicalKey::default(),
        bpm: None,
        favorite_transpose: 0,
        is_favorite: false,
        stem_status: StemStatus::Pending,
        stem_progress: 0,
        stem_model: DemucsModel::Htdemucs,
        stems_dir: None,
        has_lyrics: false,
        has_word_sync: false,
        created_at: Utc::now().to_rfc3339(),
    };

    state.db.upsert_track(&track).await.map_err(|e| e.to_string())?;

    // Kick off automatic multi-source Web/LRCLIB lookup + AI alignment
    let db = state.db.clone();
    let lrclib = state.lrclib.clone();
    let tid = track.id.clone();
    let t_title = track.title.clone();
    let t_artist = track.artist.clone();
    let dur = Some(track.duration_ms / 1000);
    let app_clone = app.clone();
    tokio::spawn(async move {
        if let Ok(raw_doc) = lrclib.fetch_best_lyrics(&t_title, &t_artist, dur).await {
            let doc = auto_align_if_plain(raw_doc, &tid, &db).await;
            if db.save_lyrics(&tid, &doc).await.is_ok() {
                let _ = app_clone.emit("lyrics-updated", &tid);
            }
        }
    });

    Ok(track)
}

#[tauri::command]
pub async fn import_spotify_playlist(
    playlist_url: String,
) -> Result<Vec<PlaylistTrackCandidate>, String> {
    import_spotify_playlist_metadata(&playlist_url)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn toggle_favorite(track_id: String, state: State<'_, AppState>) -> Result<bool, String> {
    state
        .db
        .toggle_track_favorite(&track_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_track(track_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state
        .db
        .delete_track(&track_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn separate_track_stems(
    track_id: String,
    model: Option<DemucsModel>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let track = state
        .db
        .get_track(&track_id)
        .await
        .map_err(|e| e.to_string())?;
    let selected_model = model.unwrap_or(track.stem_model);

    state
        .db
        .update_track_stem_progress(&track_id, StemStatus::Processing, 1, None)
        .await
        .map_err(|e| e.to_string())?;

    let stems_root = state.stems_dir.clone();
    let db = state.db.clone();
    let audio = Arc::clone(&state.audio);
    let src = PathBuf::from(track.source_path);
    let hash = track.content_hash;
    let tid = track_id.clone();

    tokio::task::spawn_blocking(move || {
        let rt_handle = tokio::runtime::Handle::current();
        let res = separate_track_chunked(
            &tid,
            &src,
            &stems_root,
            &hash,
            selected_model,
            |ev| {
                let _ = app.emit("stem-progress", &ev);
                let db_c = db.clone();
                let ev_c = ev.clone();
                rt_handle.spawn(async move {
                    let _ = db_c
                        .update_track_stem_progress(
                            &ev_c.track_id,
                            ev_c.status,
                            ev_c.progress_pct,
                            Some(&ev_c.stems_dir),
                        )
                        .await;
                });
                if ev.status == StemStatus::StreamingReady || ev.status == StemStatus::Completed {
                    let _ = audio.hot_swap_stems(&ev.track_id, &PathBuf::from(&ev.stems_dir));
                }
            },
        );

        if let Err(err) = res {
            warn!("Stem separation failed for {tid}: {err}");
            let db_c = db.clone();
            let tid_c = tid.clone();
            rt_handle.spawn(async move {
                let _ = db_c
                    .update_track_stem_progress(&tid_c, StemStatus::Failed, 0, None)
                    .await;
            });
        }
    });

    Ok(())
}

// -----------------------------------------------------------------------------
// Playback & Studio Controls
// -----------------------------------------------------------------------------

#[tauri::command]
pub async fn load_and_play_track(
    track_id: String,
    autoplay: Option<bool>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Track, String> {
    let track = state
        .db
        .get_track(&track_id)
        .await
        .map_err(|e| e.to_string())?;
    let src = PathBuf::from(&track.source_path);
    let stems = track.stems_dir.as_ref().map(PathBuf::from);
    let audio = Arc::clone(&state.audio);
    let tid = track.id.clone();
    let key = track.musical_key;
    let fav_transpose = track.favorite_transpose;

    tokio::task::spawn_blocking(move || {
        audio.load_track(tid, &src, stems.as_deref(), key, fav_transpose)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    if autoplay.unwrap_or(true) {
        state.audio.play();
    }
    let _ = app.emit("active-track-changed", &track);
    Ok(track)
}

#[tauri::command]
pub fn set_playing(playing: bool, state: State<'_, AppState>) {
    if playing {
        state.audio.play();
    } else {
        state.audio.pause();
    }
}

#[tauri::command]
pub fn seek_ms(position_ms: u64, state: State<'_, AppState>) {
    state.audio.seek_ms(position_ms);
}

#[tauri::command]
pub fn rewind_5s(state: State<'_, AppState>) {
    state.audio.rewind_5s();
}

#[tauri::command]
pub async fn set_transpose(
    semitones: i8,
    track_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.audio.set_transpose(semitones);
    if let Some(tid) = track_id {
        let _ = state.db.set_track_favorite_transpose(&tid, semitones).await;
    }
    Ok(())
}

#[tauri::command]
pub fn set_tempo_pct(tempo_pct: f32, state: State<'_, AppState>) {
    state.audio.set_tempo_pct(tempo_pct);
}

#[tauri::command]
pub fn set_stem_mix(mix: StemMix, state: State<'_, AppState>) {
    state.audio.set_stem_mix(mix);
}

#[tauri::command]
pub fn set_ab_loop(region: LoopRegion, state: State<'_, AppState>) {
    state.audio.set_ab_loop(region);
}

#[tauri::command]
pub fn set_latency_calibration_ms(ms: u32, state: State<'_, AppState>) {
    state.audio.set_latency_calibration_ms(ms);
}

#[tauri::command]
pub fn get_telemetry(state: State<'_, AppState>) -> PlaybackTelemetry {
    state.audio.telemetry()
}

// -----------------------------------------------------------------------------
// Wireless iPhone Microphones & Vocal DSP
// -----------------------------------------------------------------------------

#[tauri::command]
pub async fn get_mic_pairing_info(
    state: State<'_, AppState>,
) -> Result<Option<MicPairingInfo>, String> {
    let guard = state.mic_server.lock().await;
    Ok(guard.as_ref().map(|s| s.pairing_info().clone()))
}

#[tauri::command]
pub fn set_mic_preset(
    channel: MicChannelId,
    preset: VocalPreset,
    state: State<'_, AppState>,
) -> VocalDspParams {
    let params = VocalDspParams::from_preset(preset);
    state.audio.set_mic_params(channel, params);
    params
}

#[tauri::command]
pub fn set_mic_params(
    channel: MicChannelId,
    params: VocalDspParams,
    state: State<'_, AppState>,
) {
    state.audio.set_mic_params(channel, params);
}

#[tauri::command]
pub fn get_mic_params(channel: MicChannelId, state: State<'_, AppState>) -> VocalDspParams {
    state.audio.state().mic_params(channel).load()
}

// -----------------------------------------------------------------------------
// Lyrics & Sync Editor
// -----------------------------------------------------------------------------

#[tauri::command]
pub async fn get_lyrics(
    track_id: String,
    state: State<'_, AppState>,
) -> Result<Option<LyricsDocument>, String> {
    state
        .db
        .get_lyrics(&track_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_lyrics(
    track_id: String,
    doc: LyricsDocument,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LyricsDocument, String> {
    state
        .db
        .save_lyrics(&track_id, &doc)
        .await
        .map_err(|e| e.to_string())?;
    let _ = app.emit("lyrics-updated", &track_id);
    Ok(doc)
}

async fn auto_align_if_plain(
    doc: LyricsDocument,
    track_id: &str,
    db: &KaraokDatabase,
) -> LyricsDocument {
    if doc.format != LyricsFormat::Plain || doc.lines.is_empty() {
        return doc;
    }
    let Ok(track) = db.get_track(track_id).await else {
        return doc;
    };
    let plain_text = doc
        .lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let audio_path = track
        .stems_dir
        .as_ref()
        .map(|d| PathBuf::from(d).join("vocals.wav"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(&track.source_path));
    if !audio_path.exists() {
        return doc;
    }
    match tokio::task::spawn_blocking(move || {
        align_plain_lyrics_to_vocals(&audio_path, &plain_text, &WhisperAlignConfig::default())
    })
    .await
    {
        Ok(Ok(aligned)) => aligned,
        _ => doc,
    }
}

#[tauri::command]
pub async fn fetch_lrclib_lyrics(
    track_id: String,
    title: String,
    artist: String,
    duration_secs: Option<u64>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LyricsDocument, String> {
    let raw_doc = state
        .lrclib
        .fetch_best_lyrics(&title, &artist, duration_secs)
        .await
        .map_err(|e| e.to_string())?;
    let doc = auto_align_if_plain(raw_doc, &track_id, &state.db).await;

    state
        .db
        .save_lyrics(&track_id, &doc)
        .await
        .map_err(|e| e.to_string())?;
    let _ = app.emit("lyrics-updated", &track_id);
    Ok(doc)
}

#[tauri::command]
pub fn open_google_lyrics_search(title: String, artist: String) -> Result<String, String> {
    let url = build_google_lyrics_url(&title, &artist);
    std::process::Command::new("open")
        .arg(&url)
        .spawn()
        .map_err(|e| format!("Impossible d'ouvrir le navigateur : {e}"))?;
    Ok(url)
}

#[tauri::command]
pub async fn auto_align_lyrics_whisper(
    track_id: String,
    plain_lyrics: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LyricsDocument, String> {
    let track = state
        .db
        .get_track(&track_id)
        .await
        .map_err(|e| e.to_string())?;

    // Prefer isolated vocals.wav if stems exist, otherwise original audio
    let audio_path = track
        .stems_dir
        .as_ref()
        .map(|d| PathBuf::from(d).join("vocals.wav"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(&track.source_path));

    let doc = tokio::task::spawn_blocking(move || {
        align_plain_lyrics_to_vocals(&audio_path, &plain_lyrics, &WhisperAlignConfig::default())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    state
        .db
        .save_lyrics(&track_id, &doc)
        .await
        .map_err(|e| e.to_string())?;
    let _ = app.emit("lyrics-updated", &track_id);
    Ok(doc)
}

#[tauri::command]
pub async fn shift_lyrics_offset(
    track_id: String,
    delta_ms: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LyricsDocument, String> {
    let mut doc = state
        .db
        .get_lyrics(&track_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Aucune parole chargée pour ce morceau".to_string())?;
    doc.shift_offset(delta_ms);
    state
        .db
        .save_lyrics(&track_id, &doc)
        .await
        .map_err(|e| e.to_string())?;
    let _ = app.emit("lyrics-updated", &track_id);
    Ok(doc)
}

// -----------------------------------------------------------------------------
// Party Queue (Solo & Duo)
// -----------------------------------------------------------------------------

#[tauri::command]
pub async fn list_queue(state: State<'_, AppState>) -> Result<Vec<QueueItem>, String> {
    state.db.list_queue().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn enqueue_track(
    track_id: String,
    mode: QueueMode,
    singer_1: String,
    singer_2: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<QueueItem, String> {
    let item = state
        .db
        .enqueue_track(&track_id, mode, &singer_1, singer_2.as_deref(), "mac")
        .await
        .map_err(|e| e.to_string())?;
    let _ = app.emit("queue-updated", ());
    Ok(item)
}

#[tauri::command]
pub async fn advance_queue(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<QueueItem>, String> {
    let next = state.db.advance_queue().await.map_err(|e| e.to_string())?;
    let _ = app.emit("queue-updated", ());
    Ok(next)
}

#[tauri::command]
pub async fn remove_queue_item(
    item_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .db
        .remove_queue_item(&item_id)
        .await
        .map_err(|e| e.to_string())?;
    let _ = app.emit("queue-updated", ());
    Ok(())
}

// -----------------------------------------------------------------------------
// Multitrack Recording & SingSnap Post-Mix Export
// -----------------------------------------------------------------------------

#[tauri::command]
pub fn start_recording(title: String, state: State<'_, AppState>) -> Result<(), String> {
    state
        .audio
        .start_recording(&state.recordings_dir, title)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn stop_recording(state: State<'_, AppState>) -> Result<RecordingEntry, String> {
    let audio = Arc::clone(&state.audio);
    let entry = tokio::task::spawn_blocking(move || audio.stop_recording())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    state
        .db
        .insert_recording(&entry)
        .await
        .map_err(|e| e.to_string())?;
    Ok(entry)
}

#[tauri::command]
pub async fn list_recordings(state: State<'_, AppState>) -> Result<Vec<RecordingEntry>, String> {
    state.db.list_recordings().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn preview_recording_mix(
    recording_id: String,
    vocal_balance: f32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let recs = state.db.list_recordings().await.map_err(|e| e.to_string())?;
    let rec = recs
        .into_iter()
        .find(|r| r.id == recording_id)
        .ok_or_else(|| "Enregistrement introuvable".to_string())?;

    let i_path = PathBuf::from(&rec.instrumental_path);
    let m1_path = PathBuf::from(&rec.mic1_path);
    let m2_path = PathBuf::from(&rec.mic2_path);
    let preview_wav = state
        .recordings_dir
        .join(format!("preview_{}.wav", rec.id));
    let audio = Arc::clone(&state.audio);

    tokio::task::spawn_blocking(move || {
        let mixed = mix_and_export_recording(&i_path, &m1_path, &m2_path, vocal_balance, &preview_wav)
            .map_err(|e| e.to_string())?;
        audio
            .load_track(
                format!("preview-{}", rec.id),
                &mixed,
                None,
                MusicalKey::default(),
                0,
            )
            .map_err(|e| e.to_string())?;
        audio.play();
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(())
}

#[tauri::command]
pub async fn export_recording_mix(
    recording_id: String,
    vocal_balance: f32,
    output_path: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let recs = state.db.list_recordings().await.map_err(|e| e.to_string())?;
    let rec = recs
        .into_iter()
        .find(|r| r.id == recording_id)
        .ok_or_else(|| "Enregistrement introuvable".to_string())?;

    let i_path = PathBuf::from(&rec.instrumental_path);
    let m1_path = PathBuf::from(&rec.mic1_path);
    let m2_path = PathBuf::from(&rec.mic2_path);
    let out = PathBuf::from(&output_path);

    let exported = tokio::task::spawn_blocking(move || {
        mix_and_export_recording(&i_path, &m1_path, &m2_path, vocal_balance, &out)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let exported_str = exported.to_string_lossy().into_owned();
    state
        .db
        .update_recording_export(&recording_id, vocal_balance, &exported_str)
        .await
        .map_err(|e| e.to_string())?;
    Ok(exported_str)
}

// -----------------------------------------------------------------------------
// Dedicated Second Screen / TV Fullscreen Window
// -----------------------------------------------------------------------------

#[tauri::command]
pub fn toggle_tv_window(app: AppHandle) -> Result<bool, String> {
    if let Some(win) = app.get_webview_window("tv") {
        let _ = win.close();
        return Ok(false);
    }

    let win = WebviewWindowBuilder::new(&app, "tv", WebviewUrl::App("tv.html".into()))
        .title("Karaok Party — Écran TV")
        .inner_size(1280.0, 720.0)
        .resizable(true)
        .build()
        .map_err(|e| e.to_string())?;

    info!("Opened Karaok Party Second Screen / TV window");
    let _ = win.show();
    Ok(true)
}
