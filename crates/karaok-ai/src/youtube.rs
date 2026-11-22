use std::path::{Path, PathBuf};
use std::process::Command;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("Invalid or unsupported URL: {0}")]
    InvalidUrl(String),

    #[error("Subprocess '{tool}' failed: {message}")]
    Subprocess { tool: &'static str, message: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

/// Metadata & local file path produced by `yt-dlp` + `ffmpeg` extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedYoutubeTrack {
    pub title: String,
    pub artist: String,
    pub duration_ms: u64,
    pub source_url: String,
    pub audio_path: PathBuf,
}

/// Metadata-only entry extracted from a public Spotify playlist (zero DRM circumvention).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistTrackCandidate {
    pub title: String,
    pub artist: String,
    pub search_query: String,
}

/// Validates that an incoming URL uses HTTPS and belongs to YouTube / YouTube Music.
pub fn validate_youtube_url(raw: &str) -> Result<Url, ImportError> {
    let parsed = Url::parse(raw.trim()).map_err(|_| ImportError::InvalidUrl(raw.to_string()))?;
    if parsed.scheme() != "https" {
        return Err(ImportError::InvalidUrl("Only https:// URLs are allowed".into()));
    }
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    let allowed = host == "youtube.com"
        || host == "www.youtube.com"
        || host == "m.youtube.com"
        || host == "music.youtube.com"
        || host == "youtu.be";
    if !allowed {
        return Err(ImportError::InvalidUrl(format!(
            "Unsupported host '{host}' (expected youtube.com or youtu.be)"
        )));
    }
    Ok(parsed)
}

/// Safely extracts high-quality 48 kHz WAV audio and metadata from a YouTube URL using
/// `yt-dlp` and `ffmpeg` via direct `Command::arg(...)` invocation (zero shell injection).
pub fn import_youtube_audio(
    raw_url: &str,
    downloads_dir: &Path,
) -> Result<ImportedYoutubeTrack, ImportError> {
    let url = validate_youtube_url(raw_url)?;
    std::fs::create_dir_all(downloads_dir)?;

    // 1. Fetch clean JSON metadata without downloading video
    let meta_out = Command::new("yt-dlp")
        .arg("--dump-json")
        .arg("--no-playlist")
        .arg("--")
        .arg(url.as_str())
        .output()
        .map_err(|e| ImportError::Subprocess {
            tool: "yt-dlp",
            message: format!("Could not launch yt-dlp (install with `brew install yt-dlp`): {e}"),
        })?;

    if !meta_out.status.success() {
        return Err(ImportError::Subprocess {
            tool: "yt-dlp",
            message: String::from_utf8_lossy(&meta_out.stderr).trim().to_string(),
        });
    }

    let meta_json: serde_json::Value = serde_json::from_slice(&meta_out.stdout).unwrap_or_default();
    let raw_title = meta_json
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("YouTube Track")
        .to_string();
    let uploader = meta_json
        .get("artist")
        .or_else(|| meta_json.get("uploader"))
        .or_else(|| meta_json.get("channel"))
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown Artist")
        .to_string();
    let duration_secs = meta_json
        .get("duration")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let video_id = meta_json
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("yt_track");

    let safe_id: String = video_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let base_name = if safe_id.is_empty() { "yt_track" } else { &safe_id };
    let output_template = downloads_dir.join(format!("{base_name}.%(ext)s"));
    let final_wav = downloads_dir.join(format!("{base_name}.wav"));

    // 2. Extract highest quality audio normalized to 48 kHz WAV via ffmpeg postprocessor
    let dl_out = Command::new("yt-dlp")
        .arg("-x")
        .arg("--audio-format")
        .arg("wav")
        .arg("--audio-quality")
        .arg("0")
        .arg("--postprocessor-args")
        .arg("ffmpeg:-ar 48000 -ac 2")
        .arg("--no-playlist")
        .arg("-o")
        .arg(&output_template)
        .arg("--")
        .arg(url.as_str())
        .output()
        .map_err(|e| ImportError::Subprocess {
            tool: "yt-dlp",
            message: e.to_string(),
        })?;

    if !dl_out.status.success() || !final_wav.exists() {
        return Err(ImportError::Subprocess {
            tool: "yt-dlp",
            message: String::from_utf8_lossy(&dl_out.stderr).trim().to_string(),
        });
    }

    let (parsed_artist, parsed_title) = split_artist_title(&raw_title, &uploader);

    Ok(ImportedYoutubeTrack {
        title: parsed_title,
        artist: parsed_artist,
        duration_ms: (duration_secs * 1000.0).round() as u64,
        source_url: url.to_string(),
        audio_path: final_wav,
    })
}

/// Imports public track titles & artists from a public Spotify playlist URL (metadata ONLY —
/// never touches DRM streams) so Karaok Party can match local files or queue YouTube lookups.
pub async fn import_spotify_playlist_metadata(
    playlist_url: &str,
) -> Result<Vec<PlaylistTrackCandidate>, ImportError> {
    let parsed = Url::parse(playlist_url.trim())
        .map_err(|_| ImportError::InvalidUrl(playlist_url.to_string()))?;
    if parsed.scheme() != "https" || parsed.host_str() != Some("open.spotify.com") {
        return Err(ImportError::InvalidUrl(
            "Expected https://open.spotify.com/playlist/...".into(),
        ));
    }

    // Use Spotify's public embed page which renders public track metadata JSON cleanly
    let path = parsed.path().trim_start_matches('/');
    let embed_url = format!("https://open.spotify.com/embed/{path}");
    let client = Client::builder()
        .user_agent("KaraokParty/0.1.0")
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let html = client.get(&embed_url).send().await?.text().await?;

    let mut candidates = Vec::new();
    // Extract `__NEXT_DATA__` JSON embedded in Spotify public embed pages
    if let Some(start) = html.find("__NEXT_DATA__") {
        if let Some(json_start) = html[start..].find('>') {
            let after = &html[start + json_start + 1..];
            if let Some(json_end) = after.find("</script>") {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&after[..json_end]) {
                    collect_spotify_tracks(&val, &mut candidates);
                }
            }
        }
    }

    Ok(candidates)
}

fn collect_spotify_tracks(val: &serde_json::Value, out: &mut Vec<PlaylistTrackCandidate>) {
    match val {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Array(track_list)) = map.get("trackList") {
                for item in track_list {
                    let title = item
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .trim();
                    let artist = item
                        .get("subtitle")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .trim();
                    if !title.is_empty() {
                        out.push(PlaylistTrackCandidate {
                            title: title.to_string(),
                            artist: artist.to_string(),
                            search_query: format!("{artist} {title}").trim().to_string(),
                        });
                    }
                }
            }
            for child in map.values() {
                collect_spotify_tracks(child, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for child in arr {
                collect_spotify_tracks(child, out);
            }
        }
        _ => {}
    }
}

fn split_artist_title(raw_title: &str, fallback_artist: &str) -> (String, String) {
    if let Some((left, right)) = raw_title.split_once(" - ") {
        let a = left.trim();
        let t = right.trim();
        if !a.is_empty() && !t.is_empty() {
            return (a.to_string(), t.to_string());
        }
    }
    (fallback_artist.trim().to_string(), raw_title.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_youtube_urls_strictly() {
        assert!(validate_youtube_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ").is_ok());
        assert!(validate_youtube_url("https://youtu.be/dQw4w9WgXcQ").is_ok());
        assert!(validate_youtube_url("http://www.youtube.com/watch?v=abc").is_err());
        assert!(validate_youtube_url("https://evil.example.com/watch?v=abc").is_err());
    }
}
