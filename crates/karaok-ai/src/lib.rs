//! Local-first AI pipeline for Karaok Party:
//! - Chunked streaming HTDemucs v4 stem separation (7.8s windows, 25s fast-start, resume cache)
//! - LRCLIB synchronized & Enhanced LRC lyrics fetcher
//! - Forced word-by-word alignment (`whisper-rs` Metal + pure-Rust vocal onset aligner)
//! - Safe YouTube audio importer (`yt-dlp` + `ffmpeg`) & Spotify public playlist metadata importer

pub mod demucs;
pub mod lrclib;
pub mod whisper_align;
pub mod youtube;

pub use demucs::{
    compute_file_sha256, separate_track_chunked, DemucsManifest, StemProgressEvent,
    CHUNK_DURATION_SECS, FAST_START_SECS,
};
pub use lrclib::{build_google_lyrics_url, clean_search_metadata, LrclibClient, LrclibRecord};
pub use whisper_align::{align_plain_lyrics_to_vocals, WhisperAlignConfig};
pub use youtube::{
    import_spotify_playlist_metadata, import_youtube_audio, ImportedYoutubeTrack,
    PlaylistTrackCandidate,
};
