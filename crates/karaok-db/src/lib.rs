//! Local SQLite persistence layer powered by `sqlx` with versioned migrations.

use std::path::Path;
use std::str::FromStr;

use chrono::Utc;
use karaok_core::{
    DemucsModel, LyricsDocument, LyricsFormat, LyricsSource, MusicalKey, PitchClass, QueueItem,
    QueueMode, RecordingEntry, ScaleType, StemStatus, Track, TrackSource,
};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("SQLite error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("Migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Database record not found: {0}")]
    NotFound(String),
}

pub type DbResult<T> = Result<T, DbError>;

/// Persistent SQLite repository for Karaok Party.
#[derive(Clone)]
pub struct KaraokDatabase {
    pool: SqlitePool,
}

impl KaraokDatabase {
    /// Opens (or creates) the SQLite database at `db_path` and runs all embedded migrations.
    pub async fn connect_file(db_path: &Path) -> DbResult<Self> {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let url = format!("sqlite://{}", db_path.to_string_lossy());
        let opts = SqliteConnectOptions::from_str(&url)?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await?;

        Self::from_pool(pool).await
    }

    /// Creates an in-memory SQLite instance for deterministic integration tests.
    pub async fn connect_in_memory() -> DbResult<Self> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")?
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        Self::from_pool(pool).await
    }

    async fn from_pool(pool: SqlitePool) -> DbResult<Self> {
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // -------------------------------------------------------------------------
    // Tracks
    // -------------------------------------------------------------------------

    pub async fn upsert_track(&self, track: &Track) -> DbResult<()> {
        let now = Utc::now().to_rfc3339();
        let source_type = match track.source_type {
            TrackSource::Local => "local",
            TrackSource::Youtube => "youtube",
        };
        let key_scale = match track.musical_key.scale {
            ScaleType::Chromatic => "chromatic",
            ScaleType::Major => "major",
            ScaleType::Minor => "minor",
        };
        let stem_status = stem_status_to_str(track.stem_status);
        let stem_model = match track.stem_model {
            DemucsModel::Htdemucs => "htdemucs",
            DemucsModel::Htdemucs6s => "htdemucs_6s",
        };

        sqlx::query(
            r#"
            INSERT INTO tracks (
                id, title, artist, album, duration_ms, source_type, source_path, source_url,
                content_hash, key_root, key_scale, bpm, favorite_transpose, is_favorite,
                stem_status, stem_progress, stem_model, stems_dir, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
            ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                artist = excluded.artist,
                album = excluded.album,
                duration_ms = excluded.duration_ms,
                key_root = excluded.key_root,
                key_scale = excluded.key_scale,
                bpm = excluded.bpm,
                favorite_transpose = excluded.favorite_transpose,
                is_favorite = excluded.is_favorite,
                stem_status = excluded.stem_status,
                stem_progress = excluded.stem_progress,
                stem_model = excluded.stem_model,
                stems_dir = excluded.stems_dir,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&track.id)
        .bind(&track.title)
        .bind(&track.artist)
        .bind(&track.album)
        .bind(track.duration_ms as i64)
        .bind(source_type)
        .bind(&track.source_path)
        .bind(&track.source_url)
        .bind(&track.content_hash)
        .bind(track.musical_key.root as i64)
        .bind(key_scale)
        .bind(track.bpm)
        .bind(track.favorite_transpose as i64)
        .bind(if track.is_favorite { 1_i64 } else { 0_i64 })
        .bind(stem_status)
        .bind(track.stem_progress as i64)
        .bind(stem_model)
        .bind(&track.stems_dir)
        .bind(&track.created_at)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn list_tracks(&self, search: Option<&str>) -> DbResult<Vec<Track>> {
        let rows = if let Some(q) = search.filter(|s| !s.trim().is_empty()) {
            let pattern = format!("%{}%", q.trim().to_lowercase());
            sqlx::query(
                r#"
                SELECT t.*,
                       CASE WHEN l.track_id IS NOT NULL THEN 1 ELSE 0 END AS has_lyrics,
                       COALESCE(l.has_word_sync, 0) AS has_word_sync
                FROM tracks t
                LEFT JOIN lyrics l ON l.track_id = t.id
                WHERE lower(t.title) LIKE ?1 OR lower(t.artist) LIKE ?1 OR lower(COALESCE(t.album, '')) LIKE ?1
                ORDER BY t.is_favorite DESC, t.created_at DESC
                "#,
            )
            .bind(pattern)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT t.*,
                       CASE WHEN l.track_id IS NOT NULL THEN 1 ELSE 0 END AS has_lyrics,
                       COALESCE(l.has_word_sync, 0) AS has_word_sync
                FROM tracks t
                LEFT JOIN lyrics l ON l.track_id = t.id
                ORDER BY t.is_favorite DESC, t.created_at DESC
                "#,
            )
            .fetch_all(&self.pool)
            .await?
        };

        Ok(rows.iter().map(row_to_track).collect())
    }

    pub async fn get_track(&self, id: &str) -> DbResult<Track> {
        let row = sqlx::query(
            r#"
            SELECT t.*,
                   CASE WHEN l.track_id IS NOT NULL THEN 1 ELSE 0 END AS has_lyrics,
                   COALESCE(l.has_word_sync, 0) AS has_word_sync
            FROM tracks t
            LEFT JOIN lyrics l ON l.track_id = t.id
            WHERE t.id = ?1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("track {id}")))?;

        Ok(row_to_track(&row))
    }

    pub async fn update_track_stem_progress(
        &self,
        track_id: &str,
        status: StemStatus,
        progress: u8,
        stems_dir: Option<&str>,
    ) -> DbResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            UPDATE tracks
            SET stem_status = ?1,
                stem_progress = ?2,
                stems_dir = COALESCE(?3, stems_dir),
                updated_at = ?4
            WHERE id = ?5
            "#,
        )
        .bind(stem_status_to_str(status))
        .bind(progress.min(100) as i64)
        .bind(stems_dir)
        .bind(now)
        .bind(track_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_track_favorite_transpose(&self, track_id: &str, semitones: i8) -> DbResult<()> {
        let clamped = semitones.clamp(-6, 6) as i64;
        sqlx::query("UPDATE tracks SET favorite_transpose = ?1, updated_at = ?2 WHERE id = ?3")
            .bind(clamped)
            .bind(Utc::now().to_rfc3339())
            .bind(track_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn toggle_track_favorite(&self, track_id: &str) -> DbResult<bool> {
        let current: i64 = sqlx::query_scalar("SELECT is_favorite FROM tracks WHERE id = ?1")
            .bind(track_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("track {track_id}")))?;
        let next = if current == 0 { 1_i64 } else { 0_i64 };
        sqlx::query("UPDATE tracks SET is_favorite = ?1, updated_at = ?2 WHERE id = ?3")
            .bind(next)
            .bind(Utc::now().to_rfc3339())
            .bind(track_id)
            .execute(&self.pool)
            .await?;
        Ok(next == 1)
    }

    pub async fn delete_track(&self, track_id: &str) -> DbResult<()> {
        sqlx::query("DELETE FROM tracks WHERE id = ?1")
            .bind(track_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Lyrics
    // -------------------------------------------------------------------------

    pub async fn save_lyrics(&self, track_id: &str, doc: &LyricsDocument) -> DbResult<()> {
        let format_str = match doc.format {
            LyricsFormat::Plain => "plain",
            LyricsFormat::Lrc => "lrc",
            LyricsFormat::EnhancedLrc => "enhanced_lrc",
        };
        let source_str = match doc.source {
            LyricsSource::Manual => "manual",
            LyricsSource::Lrclib => "lrclib",
            LyricsSource::Whisper => "whisper",
        };
        let content = doc.to_enhanced_lrc();
        sqlx::query(
            r#"
            INSERT INTO lyrics (track_id, format, source, has_word_sync, offset_ms, content, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(track_id) DO UPDATE SET
                format = excluded.format,
                source = excluded.source,
                has_word_sync = excluded.has_word_sync,
                offset_ms = excluded.offset_ms,
                content = excluded.content,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(track_id)
        .bind(format_str)
        .bind(source_str)
        .bind(if doc.has_word_sync { 1_i64 } else { 0_i64 })
        .bind(doc.offset_ms)
        .bind(content)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_lyrics(&self, track_id: &str) -> DbResult<Option<LyricsDocument>> {
        let row = sqlx::query(
            "SELECT source, offset_ms, content FROM lyrics WHERE track_id = ?1",
        )
        .bind(track_id)
        .fetch_optional(&self.pool)
        .await?;

        let Some(r) = row else {
            return Ok(None);
        };
        let source_str: String = r.get("source");
        let source = match source_str.as_str() {
            "lrclib" => LyricsSource::Lrclib,
            "whisper" => LyricsSource::Whisper,
            _ => LyricsSource::Manual,
        };
        let content: String = r.get("content");
        let offset_ms: i64 = r.get("offset_ms");
        let mut doc = LyricsDocument::parse(&content, source);
        doc.offset_ms = offset_ms;
        Ok(Some(doc))
    }

    // -------------------------------------------------------------------------
    // Party Queue (Solo & Duo)
    // -------------------------------------------------------------------------

    pub async fn enqueue_track(
        &self,
        track_id: &str,
        mode: QueueMode,
        singer_1: &str,
        singer_2: Option<&str>,
        added_by: &str,
    ) -> DbResult<QueueItem> {
        let next_pos: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM queue_items WHERE status = 'queued'",
        )
        .fetch_one(&self.pool)
        .await?;

        let id = Uuid::new_v4().to_string();
        let mode_str = match mode {
            QueueMode::Solo => "solo",
            QueueMode::Duo => "duo",
        };
        sqlx::query(
            r#"
            INSERT INTO queue_items (id, track_id, position, mode, singer_1, singer_2, status, added_by, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued', ?7, ?8)
            "#,
        )
        .bind(&id)
        .bind(track_id)
        .bind(next_pos)
        .bind(mode_str)
        .bind(singer_1)
        .bind(singer_2)
        .bind(added_by)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;

        let track = self.get_track(track_id).await?;
        Ok(QueueItem {
            id,
            track_id: track_id.to_string(),
            track_title: track.title,
            track_artist: track.artist,
            position: next_pos,
            mode,
            singer_1: singer_1.to_string(),
            singer_2: singer_2.map(|s| s.to_string()),
            status: "queued".to_string(),
            added_by: added_by.to_string(),
        })
    }

    pub async fn list_queue(&self) -> DbResult<Vec<QueueItem>> {
        let rows = sqlx::query(
            r#"
            SELECT q.id, q.track_id, t.title AS track_title, t.artist AS track_artist,
                   q.position, q.mode, q.singer_1, q.singer_2, q.status, q.added_by
            FROM queue_items q
            INNER JOIN tracks t ON t.id = q.track_id
            WHERE q.status IN ('queued', 'playing')
            ORDER BY CASE WHEN q.status = 'playing' THEN 0 ELSE 1 END, q.position ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|r| {
                let mode_str: String = r.get("mode");
                QueueItem {
                    id: r.get("id"),
                    track_id: r.get("track_id"),
                    track_title: r.get("track_title"),
                    track_artist: r.get("track_artist"),
                    position: r.get("position"),
                    mode: if mode_str == "duo" {
                        QueueMode::Duo
                    } else {
                        QueueMode::Solo
                    },
                    singer_1: r.get("singer_1"),
                    singer_2: r.get("singer_2"),
                    status: r.get("status"),
                    added_by: r.get("added_by"),
                }
            })
            .collect())
    }

    pub async fn advance_queue(&self) -> DbResult<Option<QueueItem>> {
        sqlx::query("UPDATE queue_items SET status = 'completed' WHERE status = 'playing'")
            .execute(&self.pool)
            .await?;

        let next_id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM queue_items WHERE status = 'queued' ORDER BY position ASC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;

        if let Some(id) = next_id {
            sqlx::query("UPDATE queue_items SET status = 'playing' WHERE id = ?1")
                .bind(&id)
                .execute(&self.pool)
                .await?;
            let queue = self.list_queue().await?;
            Ok(queue.into_iter().find(|q| q.id == id))
        } else {
            Ok(None)
        }
    }

    pub async fn remove_queue_item(&self, item_id: &str) -> DbResult<()> {
        sqlx::query("DELETE FROM queue_items WHERE id = ?1")
            .bind(item_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Recordings
    // -------------------------------------------------------------------------

    pub async fn insert_recording(&self, rec: &RecordingEntry) -> DbResult<()> {
        sqlx::query(
            r#"
            INSERT INTO recordings (
                id, track_id, title, created_at, duration_ms,
                instrumental_path, mic1_path, mic2_path, mix_vocal_balance, exported_path
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
        )
        .bind(&rec.id)
        .bind(&rec.track_id)
        .bind(&rec.title)
        .bind(&rec.created_at)
        .bind(rec.duration_ms as i64)
        .bind(&rec.instrumental_path)
        .bind(&rec.mic1_path)
        .bind(&rec.mic2_path)
        .bind(rec.mix_vocal_balance)
        .bind(&rec.exported_path)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_recordings(&self) -> DbResult<Vec<RecordingEntry>> {
        let rows = sqlx::query("SELECT * FROM recordings ORDER BY created_at DESC")
            .fetch_all(&self.pool)
            .await?;

        Ok(rows
            .iter()
            .map(|r| RecordingEntry {
                id: r.get("id"),
                track_id: r.get("track_id"),
                title: r.get("title"),
                created_at: r.get("created_at"),
                duration_ms: r.get::<i64, _>("duration_ms").max(0) as u64,
                instrumental_path: r.get("instrumental_path"),
                mic1_path: r.get("mic1_path"),
                mic2_path: r.get("mic2_path"),
                mix_vocal_balance: r.get("mix_vocal_balance"),
                exported_path: r.get("exported_path"),
            })
            .collect())
    }

    pub async fn update_recording_export(
        &self,
        recording_id: &str,
        vocal_balance: f32,
        exported_path: &str,
    ) -> DbResult<()> {
        sqlx::query(
            "UPDATE recordings SET mix_vocal_balance = ?1, exported_path = ?2 WHERE id = ?3",
        )
        .bind(vocal_balance.clamp(0.0, 1.0))
        .bind(exported_path)
        .bind(recording_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn stem_status_to_str(status: StemStatus) -> &'static str {
    match status {
        StemStatus::Pending => "pending",
        StemStatus::Processing => "processing",
        StemStatus::StreamingReady => "streaming_ready",
        StemStatus::Completed => "completed",
        StemStatus::Failed => "failed",
    }
}

fn str_to_stem_status(s: &str) -> StemStatus {
    match s {
        "processing" => StemStatus::Processing,
        "streaming_ready" => StemStatus::StreamingReady,
        "completed" => StemStatus::Completed,
        "failed" => StemStatus::Failed,
        _ => StemStatus::Pending,
    }
}

fn row_to_track(r: &sqlx::sqlite::SqliteRow) -> Track {
    let source_str: String = r.get("source_type");
    let scale_str: String = r.get("key_scale");
    let model_str: String = r.get("stem_model");
    let status_str: String = r.get("stem_status");
    let key_root: i64 = r.get("key_root");

    Track {
        id: r.get("id"),
        title: r.get("title"),
        artist: r.get("artist"),
        album: r.get("album"),
        duration_ms: r.get::<i64, _>("duration_ms").max(0) as u64,
        source_type: if source_str == "youtube" {
            TrackSource::Youtube
        } else {
            TrackSource::Local
        },
        source_path: r.get("source_path"),
        source_url: r.get("source_url"),
        content_hash: r.get("content_hash"),
        musical_key: MusicalKey::new(
            PitchClass::from_semitone(key_root as i32),
            match scale_str.as_str() {
                "chromatic" => ScaleType::Chromatic,
                "minor" => ScaleType::Minor,
                _ => ScaleType::Major,
            },
        ),
        bpm: r.get("bpm"),
        favorite_transpose: r.get::<i64, _>("favorite_transpose").clamp(-6, 6) as i8,
        is_favorite: r.get::<i64, _>("is_favorite") != 0,
        stem_status: str_to_stem_status(&status_str),
        stem_progress: r.get::<i64, _>("stem_progress").clamp(0, 100) as u8,
        stem_model: if model_str == "htdemucs_6s" {
            DemucsModel::Htdemucs6s
        } else {
            DemucsModel::Htdemucs
        },
        stems_dir: r.get("stems_dir"),
        has_lyrics: r.get::<i64, _>("has_lyrics") != 0,
        has_word_sync: r.get::<i64, _>("has_word_sync") != 0,
        created_at: r.get("created_at"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn crud_tracks_lyrics_and_duo_queue() {
        let db = KaraokDatabase::connect_in_memory().await.unwrap();
        let track = Track {
            id: "trk-1".into(),
            title: "Bohemian Rhapsody".into(),
            artist: "Queen".into(),
            album: Some("A Night at the Opera".into()),
            duration_ms: 354_000,
            source_type: TrackSource::Local,
            source_path: "/music/bohemian.flac".into(),
            source_url: None,
            content_hash: "abc123".into(),
            musical_key: MusicalKey::new(PitchClass::ASharp, ScaleType::Major),
            bpm: Some(72.0),
            favorite_transpose: -2,
            is_favorite: true,
            stem_status: StemStatus::Completed,
            stem_progress: 100,
            stem_model: DemucsModel::Htdemucs,
            stems_dir: Some("/stems/abc123".into()),
            has_lyrics: false,
            has_word_sync: false,
            created_at: Utc::now().to_rfc3339(),
        };
        db.upsert_track(&track).await.unwrap();

        let lrc = LyricsDocument::parse(
            "[00:01.00] <00:01.00> Is <00:01.30> this <00:01.60> real",
            LyricsSource::Lrclib,
        );
        db.save_lyrics("trk-1", &lrc).await.unwrap();

        let fetched = db.get_track("trk-1").await.unwrap();
        assert!(fetched.has_lyrics);
        assert!(fetched.has_word_sync);
        assert_eq!(fetched.favorite_transpose, -2);

        let queued = db
            .enqueue_track("trk-1", QueueMode::Duo, "Alice", Some("Bob"), "mic1")
            .await
            .unwrap();
        assert_eq!(queued.mode, QueueMode::Duo);
        assert_eq!(queued.singer_2.as_deref(), Some("Bob"));

        let next = db.advance_queue().await.unwrap().unwrap();
        assert_eq!(next.status, "playing");

        // Verify both track-linked and freestyle (track_id = None) recordings persist cleanly
        let freestyle_rec = RecordingEntry {
            id: "rec-free-1".into(),
            track_id: None,
            title: "Freestyle Take".into(),
            created_at: Utc::now().to_rfc3339(),
            duration_ms: 12_000,
            instrumental_path: "/rec/i.wav".into(),
            mic1_path: "/rec/m1.wav".into(),
            mic2_path: "/rec/m2.wav".into(),
            mix_vocal_balance: 0.6,
            exported_path: None,
        };
        db.insert_recording(&freestyle_rec).await.unwrap();
        db.update_recording_export("rec-free-1", 0.75, "/rec/final.wav")
            .await
            .unwrap();
        let recs = db.list_recordings().await.unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].track_id, None);
        assert_eq!(recs[0].exported_path.as_deref(), Some("/rec/final.wav"));
    }
}
