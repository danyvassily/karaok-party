use karaok_core::{LyricsDocument, LyricsSource};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const LRCLIB_BASE_URL: &str = "https://lrclib.net/api";

#[derive(Debug, Error)]
pub enum LrclibError {
    #[error("LRCLIB HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("No lyrics found on LRCLIB")]
    NotFound,
}

/// Raw LRCLIB API response payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LrclibRecord {
    pub id: i64,
    pub track_name: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub duration: Option<f64>,
    pub instrumental: bool,
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>,
}

/// Async client for the open LRCLIB lyrics API (`https://lrclib.net/api/get` & `/api/search`).
#[derive(Clone)]
pub struct LrclibClient {
    http: Client,
}

impl Default for LrclibClient {
    fn default() -> Self {
        Self::new()
    }
}

impl LrclibClient {
    pub fn new() -> Self {
        let http = Client::builder()
            .user_agent("KaraokParty/0.1.0 (https://github.com/danyvassily/karaok-party)")
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { http }
    }

    /// Queries `/api/get` for an exact track match, falling back to `/api/search` if needed.
    pub async fn fetch_best_lyrics(
        &self,
        title: &str,
        artist: &str,
        duration_secs: Option<u64>,
    ) -> Result<LyricsDocument, LrclibError> {
        let mut params = vec![
            ("track_name", title.to_string()),
            ("artist_name", artist.to_string()),
        ];
        if let Some(dur) = duration_secs.filter(|&d| d > 0) {
            params.push(("duration", dur.to_string()));
        }

        let resp = self
            .http
            .get(format!("{LRCLIB_BASE_URL}/get"))
            .query(&params)
            .send()
            .await?;

        if resp.status().is_success() {
            if let Ok(record) = resp.json::<LrclibRecord>().await {
                if let Some(doc) = record_to_lyrics_doc(&record) {
                    return Ok(doc);
                }
            }
        }

        // Fallback to fuzzy search
        let results = self.search(&format!("{artist} {title}")).await?;
        for rec in results {
            if rec.synced_lyrics.is_some() {
                if let Some(doc) = record_to_lyrics_doc(&rec) {
                    return Ok(doc);
                }
            }
        }
        Err(LrclibError::NotFound)
    }

    /// Queries `/api/search?q=...` and returns matching LRCLIB records.
    pub async fn search(&self, query: &str) -> Result<Vec<LrclibRecord>, LrclibError> {
        let resp = self
            .http
            .get(format!("{LRCLIB_BASE_URL}/search"))
            .query(&[("q", query)])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Ok(Vec::new());
        }
        Ok(resp.json::<Vec<LrclibRecord>>().await.unwrap_or_default())
    }
}

fn record_to_lyrics_doc(record: &LrclibRecord) -> Option<LyricsDocument> {
    if let Some(ref synced) = record.synced_lyrics {
        if !synced.trim().is_empty() {
            return Some(LyricsDocument::parse(synced, LyricsSource::Lrclib));
        }
    }
    if let Some(ref plain) = record.plain_lyrics {
        if !plain.trim().is_empty() {
            return Some(LyricsDocument::parse(plain, LyricsSource::Lrclib));
        }
    }
    None
}
