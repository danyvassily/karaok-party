-- Initial schema for Karaok Party local-first SQLite database.

CREATE TABLE IF NOT EXISTS tracks (
    id TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    album TEXT,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    source_type TEXT NOT NULL DEFAULT 'local',
    source_path TEXT NOT NULL,
    source_url TEXT,
    content_hash TEXT NOT NULL,
    key_root INTEGER NOT NULL DEFAULT 0,
    key_scale TEXT NOT NULL DEFAULT 'major',
    bpm REAL,
    favorite_transpose INTEGER NOT NULL DEFAULT 0,
    is_favorite INTEGER NOT NULL DEFAULT 0,
    stem_status TEXT NOT NULL DEFAULT 'pending',
    stem_progress INTEGER NOT NULL DEFAULT 0,
    stem_model TEXT NOT NULL DEFAULT 'htdemucs',
    stems_dir TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tracks_content_hash ON tracks(content_hash);
CREATE INDEX IF NOT EXISTS idx_tracks_favorite ON tracks(is_favorite);
CREATE INDEX IF NOT EXISTS idx_tracks_stem_status ON tracks(stem_status);

CREATE TABLE IF NOT EXISTS lyrics (
    track_id TEXT PRIMARY KEY NOT NULL,
    format TEXT NOT NULL DEFAULT 'plain',
    source TEXT NOT NULL DEFAULT 'manual',
    has_word_sync INTEGER NOT NULL DEFAULT 0,
    offset_ms INTEGER NOT NULL DEFAULT 0,
    content TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (track_id) REFERENCES tracks(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS queue_items (
    id TEXT PRIMARY KEY NOT NULL,
    track_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    mode TEXT NOT NULL DEFAULT 'solo',
    singer_1 TEXT NOT NULL,
    singer_2 TEXT,
    status TEXT NOT NULL DEFAULT 'queued',
    added_by TEXT NOT NULL DEFAULT 'mac',
    created_at TEXT NOT NULL,
    FOREIGN KEY (track_id) REFERENCES tracks(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_queue_position ON queue_items(status, position);

CREATE TABLE IF NOT EXISTS recordings (
    id TEXT PRIMARY KEY NOT NULL,
    track_id TEXT,
    title TEXT NOT NULL,
    created_at TEXT NOT NULL,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    instrumental_path TEXT NOT NULL,
    mic1_path TEXT NOT NULL,
    mic2_path TEXT NOT NULL,
    mix_vocal_balance REAL NOT NULL DEFAULT 0.5,
    exported_path TEXT,
    FOREIGN KEY (track_id) REFERENCES tracks(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS app_settings (
    key TEXT PRIMARY KEY NOT NULL,
    value_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
