use karaok_core::{LyricsDocument, LyricsSource};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

const LRCLIB_BASE_URL: &str = "https://lrclib.net/api";
const LYRICS_OVH_BASE_URL: &str = "https://api.lyrics.ovh/v1";
const LYRIST_BASE_URL: &str = "https://lyrist.vercel.app/api";

#[derive(Debug, Error)]
pub enum LrclibError {
    #[error("Web lyrics HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Aucune parole trouvée automatiquement sur LRCLIB / Web (utilisez le bouton Google)")]
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

#[derive(Debug, Deserialize)]
struct LyricsOvhResponse {
    lyrics: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LyristResponse {
    lyrics: Option<String>,
}

/// Cleans YouTube / filename noise tags like `(Official Video)`, `[Clip Officiel]`,
/// `(Lyrics)`, `(Audio)`, `- Topic`, `VEVO`, `feat. ...` to dramatically improve web lyrics matching.
pub fn clean_search_metadata(title: &str, artist: &str) -> (String, String) {
    let clean_piece = |raw: &str| -> String {
        let mut s = raw.to_string();
        // Strip bracketed & parenthesized noise phrases
        for (open, close) in [('(', ')'), ('[', ']'), ('【', '】')] {
            while let Some(start) = s.find(open) {
                let Some(rel_end) = s[start + open.len_utf8()..].find(close) else {
                    break;
                };
                let end = start + open.len_utf8() + rel_end + close.len_utf8();
                let inner = s[start..end].to_lowercase();
                let is_noise = [
                    "official",
                    "clip",
                    "video",
                    "vidéo",
                    "audio",
                    "lyric",
                    "parole",
                    "karaoke",
                    "karaoké",
                    "hd",
                    "4k",
                    "remaster",
                    "visualizer",
                    "live",
                    "feat",
                    "ft.",
                    "prod.",
                ]
                .iter()
                .any(|kw| inner.contains(kw));
                if is_noise {
                    s.replace_range(start..end, " ");
                } else {
                    break;
                }
            }
        }
        // Strip trailing unparenthesized featuring clauses (e.g. "Stay ft. Mikky Ekko" -> "Stay")
        // Search ASCII case-insensitively on raw UTF-8 bytes so multi-byte uppercase chars (e.g. 'İ')
        // never shift byte offsets when calling `truncate`.
        for marker in [" feat. ", " ft. ", " featuring ", " feat ", " ft "] {
            let m = marker.as_bytes();
            if let Some(pos) = s
                .as_bytes()
                .windows(m.len())
                .position(|w| w.eq_ignore_ascii_case(m))
            {
                s.truncate(pos);
                break;
            }
        }
        // Strip common channel suffixes
        for suffix in [" - Topic", " VEVO", "VEVO", " Official", " Officiel"] {
            if let Some(stripped) = s.strip_suffix(suffix) {
                s = stripped.to_string();
            }
        }
        // Collapse extra whitespace
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    };

    let mut clean_title = clean_piece(title);
    let mut clean_artist = clean_piece(artist);

    // If artist is unknown/generic and title contains "Artist - Title", split it cleanly
    let generic_artist = clean_artist.is_empty()
        || clean_artist.eq_ignore_ascii_case("artiste inconnu")
        || clean_artist.eq_ignore_ascii_case("unknown artist");
    if generic_artist {
        if let Some((a, t)) = clean_title.split_once(" - ") {
            clean_artist = a.trim().to_string();
            clean_title = t.trim().to_string();
        }
    }

    (clean_title, clean_artist)
}

/// Builds a safe Google Search URL (`https://www.google.com/search?q=...`) for manual lookup.
pub fn build_google_lyrics_url(title: &str, artist: &str) -> String {
    let (clean_title, clean_artist) = clean_search_metadata(title, artist);
    let query = if clean_artist.is_empty()
        || clean_artist.eq_ignore_ascii_case("artiste inconnu")
        || clean_artist.eq_ignore_ascii_case("unknown artist")
    {
        format!("{clean_title} paroles lyrics")
    } else {
        format!("{clean_artist} {clean_title} paroles lyrics")
    };
    let mut url = Url::parse("https://www.google.com/search")
        .unwrap_or_else(|_| Url::parse("https://www.google.com").expect("static URL"));
    url.query_pairs_mut().append_pair("q", query.trim());
    url.to_string()
}

/// Async multi-source web lyrics client:
/// 1. LRCLIB exact `/api/get` (synced & Enhanced LRC)
/// 2. LRCLIB fuzzy `/api/search` (cleaned artist+title, then cleaned title)
/// 3. `lyrics.ovh` open lyrics API (plain text -> auto-aligned by Whisper/onset engine)
/// 4. `lyrist.vercel.app` public web lyrics fallback (Genius/Musixmatch metadata -> plain text)
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

    /// Queries LRCLIB and open web lyrics APIs in priority order, preferring synced `.lrc`
    /// and falling back cleanly to plain web lyrics (which can then be auto-aligned to vocals).
    pub async fn fetch_best_lyrics(
        &self,
        title: &str,
        artist: &str,
        duration_secs: Option<u64>,
    ) -> Result<LyricsDocument, LrclibError> {
        let (clean_title, clean_artist) = clean_search_metadata(title, artist);

        let mut best_plain_fallback: Option<LyricsDocument> = None;

        // 1. LRCLIB exact match (/api/get): accept immediately only when time-synced `.lrc` exists
        if !clean_title.is_empty() && !clean_artist.is_empty() {
            let mut params = vec![
                ("track_name", clean_title.clone()),
                ("artist_name", clean_artist.clone()),
            ];
            if let Some(dur) = duration_secs.filter(|&d| d > 0) {
                params.push(("duration", dur.to_string()));
            }
            if let Ok(resp) = self
                .http
                .get(format!("{LRCLIB_BASE_URL}/get"))
                .query(&params)
                .send()
                .await
            {
                if resp.status().is_success() {
                    if let Ok(record) = resp.json::<LrclibRecord>().await {
                        if record
                            .synced_lyrics
                            .as_deref()
                            .is_some_and(|s| !s.trim().is_empty())
                        {
                            if let Some(doc) = record_to_lyrics_doc(&record) {
                                return Ok(doc);
                            }
                        } else if best_plain_fallback.is_none() {
                            best_plain_fallback = record_to_lyrics_doc(&record);
                        }
                    }
                }
            }
        }

        // 2. LRCLIB fuzzy search (artist + title, then title-only)
        let queries = if !clean_artist.is_empty()
            && !clean_artist.eq_ignore_ascii_case("artiste inconnu")
            && !clean_artist.eq_ignore_ascii_case("unknown artist")
        {
            vec![format!("{clean_artist} {clean_title}"), clean_title.clone()]
        } else {
            vec![clean_title.clone()]
        };

        for q in queries {
            if q.trim().is_empty() {
                continue;
            }
            if let Ok(results) = self.search(&q).await {
                for rec in &results {
                    if rec.synced_lyrics.as_deref().is_some_and(|s| !s.trim().is_empty()) {
                        if let Some(doc) = record_to_lyrics_doc(rec) {
                            return Ok(doc);
                        }
                    }
                    if best_plain_fallback.is_none() {
                        if let Some(doc) = record_to_lyrics_doc(rec) {
                            best_plain_fallback = Some(doc);
                        }
                    }
                }
            }
        }

        if let Some(doc) = best_plain_fallback {
            return Ok(doc);
        }

        // 3. Open Web API fallback: lyrics.ovh
        if !clean_artist.is_empty() && !clean_title.is_empty() {
            if let Some(plain) = self.fetch_lyrics_ovh(&clean_artist, &clean_title).await {
                return Ok(LyricsDocument::parse(&plain, LyricsSource::Lrclib));
            }
        }

        // 4. Open Web API fallback: Lyrist (Genius/web catalog)
        if !clean_title.is_empty() {
            if let Some(plain) = self.fetch_lyrist(&clean_title, &clean_artist).await {
                return Ok(LyricsDocument::parse(&plain, LyricsSource::Lrclib));
            }
        }

        Err(LrclibError::NotFound)
    }

    async fn fetch_lyrics_ovh(&self, artist: &str, title: &str) -> Option<String> {
        let mut url = Url::parse(LYRICS_OVH_BASE_URL).ok()?;
        url.path_segments_mut().ok()?.push(artist).push(title);
        let resp = self.http.get(url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let body: LyricsOvhResponse = resp.json().await.ok()?;
        body.lyrics
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    async fn fetch_lyrist(&self, title: &str, artist: &str) -> Option<String> {
        let mut url = Url::parse(LYRIST_BASE_URL).ok()?;
        {
            let mut segs = url.path_segments_mut().ok()?;
            segs.push(title);
            if !artist.is_empty() {
                segs.push(artist);
            }
        }
        let resp = self.http.get(url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let body: LyristResponse = resp.json().await.ok()?;
        body.lyrics
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_youtube_noise_from_title_and_artist() {
        let (t, a) = clean_search_metadata(
            "Stromae - Papaoutai (Official Video) [HD]",
            "Artiste inconnu",
        );
        assert_eq!(a, "Stromae");
        assert_eq!(t, "Papaoutai");

        let (t2, a2) = clean_search_metadata("Formidable (Clip Officiel)", "StromaeVEVO");
        assert_eq!(a2, "Stromae");
        assert_eq!(t2, "Formidable");

        let (t3, a3) = clean_search_metadata("Stay (Lyrics) ft. Mikky Ekko", "Rihanna");
        assert_eq!(a3, "Rihanna");
        assert_eq!(t3, "Stay");

        let (t4, a4) = clean_search_metadata("İzmir ft. Someone", "İstanbul");
        assert_eq!(a4, "İstanbul");
        assert_eq!(t4, "İzmir");
    }

    #[test]
    fn builds_safe_google_lyrics_search_url() {
        let url = build_google_lyrics_url("Bohemian Rhapsody (Official Video)", "Queen");
        assert!(url.starts_with("https://www.google.com/search?q="));
        assert!(url.contains("Queen"));
        assert!(url.contains("Bohemian"));
        assert!(url.contains("paroles"));
    }
}
