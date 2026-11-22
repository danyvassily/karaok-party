use serde::{Deserialize, Serialize};

/// Format of stored lyrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LyricsFormat {
    #[default]
    Plain,
    Lrc,
    EnhancedLrc,
}

/// Origin of a lyrics document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LyricsSource {
    #[default]
    Manual,
    Lrclib,
    Whisper,
}

/// A single timed word or syllable inside a line (Enhanced LRC).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsWord {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// A single line of lyrics with line-level timing and optional word-level timing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsLine {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub words: Vec<LyricsWord>,
}

/// Complete parsed lyrics document for a track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LyricsDocument {
    pub format: LyricsFormat,
    pub source: LyricsSource,
    pub has_word_sync: bool,
    pub offset_ms: i64,
    pub lines: Vec<LyricsLine>,
}

/// Snapshot of the 5-line karaoke stage window computed for a given audio clock position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveLyricsWindow {
    pub active_index: Option<usize>,
    /// Line progress in `[0.0, 1.0]` for smooth highlighting.
    pub line_progress: f32,
    /// Word index within the active line (if word-level sync is available).
    pub active_word_index: Option<usize>,
    /// Word progress in `[0.0, 1.0]`.
    pub word_progress: f32,
    /// 3-dot visual countdown (`Some(3)`, `Some(2)`, `Some(1)`) before singing resumes
    /// after an instrumental gap of >= 4 seconds.
    pub countdown_dots: Option<u8>,
    /// Milliseconds until the next line begins (if currently in intro or instrumental bridge).
    pub next_singing_in_ms: Option<u64>,
}

impl LyricsDocument {
    /// Parses standard `.lrc` (`[mm:ss.xx] line`), Enhanced `.lrc`
    /// (`[mm:ss.xx] <mm:ss.xx> Word1 <mm:ss.xx> Word2`), or plain text lyrics.
    pub fn parse(raw: &str, source: LyricsSource) -> Self {
        let mut parsed_lines: Vec<LyricsLine> = Vec::new();
        let mut any_word_sync = false;
        let mut global_offset_ms: i64 = 0;

        for raw_line in raw.lines() {
            let trimmed = raw_line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Metadata offset tag: [offset:+/-ms]
            if let Some(rest) = trimmed.strip_prefix("[offset:") {
                if let Some(val_str) = rest.strip_suffix(']') {
                    if let Ok(val) = val_str.trim().parse::<i64>() {
                        global_offset_ms = val;
                    }
                }
                continue;
            }

            // Skip non-timed metadata tags like [ar:...], [ti:...], [al:...]
            if trimmed.starts_with('[') && !is_line_timestamp(trimmed) {
                continue;
            }

            let (timestamps, body) = extract_leading_timestamps(trimmed);
            if timestamps.is_empty() {
                // Plain untimed line fallback
                parsed_lines.push(LyricsLine {
                    start_ms: 0,
                    end_ms: 0,
                    text: trimmed.to_string(),
                    words: Vec::new(),
                });
                continue;
            }

            let words = parse_enhanced_words(body);
            let clean_text = if !words.is_empty() {
                any_word_sync = true;
                words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                body.trim().to_string()
            };

            // Repeat chorus lines when multiple leading timestamps appear: [00:12.00][01:12.00]Text
            for ts in timestamps {
                let mut line_words = words.clone();
                if !line_words.is_empty() {
                    // Shift word timestamps if line was repeated with a different leading timestamp
                    let delta = ts as i64 - line_words[0].start_ms as i64;
                    if delta.abs() > 5000 {
                        for w in &mut line_words {
                            w.start_ms = (w.start_ms as i64 + delta).max(0) as u64;
                            w.end_ms = (w.end_ms as i64 + delta).max(0) as u64;
                        }
                    }
                }
                parsed_lines.push(LyricsLine {
                    start_ms: ts,
                    end_ms: ts + 3500,
                    text: clean_text.clone(),
                    words: line_words,
                });
            }
        }

        let has_timestamps = parsed_lines.iter().any(|l| l.start_ms > 0)
            || (parsed_lines.len() > 1 && parsed_lines.iter().any(|l| !l.words.is_empty()));

        if has_timestamps {
            parsed_lines.sort_by_key(|l| l.start_ms);
            // Compute tight end_ms for lines and words
            let len = parsed_lines.len();
            for i in 0..len {
                let next_start = if i + 1 < len {
                    Some(parsed_lines[i + 1].start_ms)
                } else {
                    None
                };

                let line_end = match next_start {
                    Some(ns) if ns > parsed_lines[i].start_ms => {
                        // Cap instrumental gaps so a line doesn't stay active for 20 seconds
                        let max_reasonable = parsed_lines[i].start_ms + 8000;
                        ns.min(max_reasonable)
                    }
                    _ => parsed_lines[i].start_ms + 4000,
                };
                parsed_lines[i].end_ms = line_end;

                let word_count = parsed_lines[i].words.len();
                for wi in 0..word_count {
                    let w_end = if wi + 1 < word_count {
                        parsed_lines[i].words[wi + 1].start_ms
                    } else {
                        line_end
                    };
                    let start = parsed_lines[i].words[wi].start_ms;
                    parsed_lines[i].words[wi].end_ms = w_end.max(start + 120);
                }
            }
        }

        let format = if any_word_sync {
            LyricsFormat::EnhancedLrc
        } else if has_timestamps {
            LyricsFormat::Lrc
        } else {
            LyricsFormat::Plain
        };

        Self {
            format,
            source,
            has_word_sync: any_word_sync,
            offset_ms: global_offset_ms,
            lines: parsed_lines,
        }
    }

    /// Serializes this document to Enhanced LRC (or standard LRC if no word timings exist).
    pub fn to_enhanced_lrc(&self) -> String {
        let mut out = String::new();
        if self.offset_ms != 0 {
            out.push_str(&format!("[offset:{}]\n", self.offset_ms));
        }
        for line in &self.lines {
            out.push('[');
            out.push_str(&format_timestamp(line.start_ms));
            out.push(']');
            if !line.words.is_empty() {
                for word in &line.words {
                    out.push(' ');
                    out.push('<');
                    out.push_str(&format_timestamp(word.start_ms));
                    out.push('>');
                    out.push(' ');
                    out.push_str(&word.text);
                }
            } else {
                out.push(' ');
                out.push_str(&line.text);
            }
            out.push('\n');
        }
        out
    }

    /// Applies a global synchronization shift (`+100 ms` or `-100 ms` via `]` and `[`).
    pub fn shift_offset(&mut self, delta_ms: i64) {
        self.offset_ms = (self.offset_ms + delta_ms).clamp(-30_000, 30_000);
    }

    /// Computes the active line, active word, and 3-dot countdown for `position_ms`.
    pub fn active_window(&self, position_ms: u64) -> ActiveLyricsWindow {
        if self.lines.is_empty() {
            return ActiveLyricsWindow {
                active_index: None,
                line_progress: 0.0,
                active_word_index: None,
                word_progress: 0.0,
                countdown_dots: None,
                next_singing_in_ms: None,
            };
        }

        let effective_ms = (position_ms as i64 + self.offset_ms).max(0) as u64;

        // Before the very first line
        let first_start = self.lines[0].start_ms;
        if effective_ms < first_start {
            let until = first_start - effective_ms;
            let dots = compute_countdown(until, first_start >= 3000);
            return ActiveLyricsWindow {
                active_index: None,
                line_progress: 0.0,
                active_word_index: None,
                word_progress: 0.0,
                countdown_dots: dots,
                next_singing_in_ms: Some(until),
            };
        }

        // Find the last line whose start_ms <= effective_ms
        let mut active_idx = 0usize;
        for (idx, line) in self.lines.iter().enumerate() {
            if line.start_ms <= effective_ms {
                active_idx = idx;
            } else {
                break;
            }
        }

        let line = &self.lines[active_idx];
        let duration = line.end_ms.saturating_sub(line.start_ms).max(1);
        let elapsed = effective_ms.saturating_sub(line.start_ms);
        let line_progress = (elapsed as f32 / duration as f32).clamp(0.0, 1.0);

        // Check if we are in an instrumental bridge between line.end_ms and next_line.start_ms
        let mut countdown_dots = None;
        let mut next_singing_in_ms = None;
        if let Some(next_line) = self.lines.get(active_idx + 1) {
            let gap = next_line.start_ms.saturating_sub(line.end_ms);
            if effective_ms >= line.end_ms && effective_ms < next_line.start_ms {
                let until = next_line.start_ms - effective_ms;
                next_singing_in_ms = Some(until);
                countdown_dots = compute_countdown(until, gap >= 3500);
            }
        }

        // Compute active word progress if Enhanced LRC words exist
        let (active_word_index, word_progress) = if !line.words.is_empty() {
            let mut w_idx = 0usize;
            for (i, w) in line.words.iter().enumerate() {
                if w.start_ms <= effective_ms {
                    w_idx = i;
                } else {
                    break;
                }
            }
            let w = &line.words[w_idx];
            let w_dur = w.end_ms.saturating_sub(w.start_ms).max(1);
            let w_elapsed = effective_ms.saturating_sub(w.start_ms);
            (Some(w_idx), (w_elapsed as f32 / w_dur as f32).clamp(0.0, 1.0))
        } else {
            (None, line_progress)
        };

        ActiveLyricsWindow {
            active_index: Some(active_idx),
            line_progress,
            active_word_index,
            word_progress,
            countdown_dots,
            next_singing_in_ms,
        }
    }
}

fn compute_countdown(until_ms: u64, long_gap: bool) -> Option<u8> {
    if !long_gap || until_ms == 0 || until_ms > 3000 {
        return None;
    }
    if until_ms > 2000 {
        Some(3)
    } else if until_ms > 1000 {
        Some(2)
    } else {
        Some(1)
    }
}

fn is_line_timestamp(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.len() >= 8 && bytes[0] == b'[' && bytes[1].is_ascii_digit() && bytes[2].is_ascii_digit()
}

fn parse_timestamp_str(ts: &str) -> Option<u64> {
    let (min_str, sec_part) = ts.split_once(':')?;
    let minutes: u64 = min_str.trim().parse().ok()?;
    let (sec_str, frac_str) = match sec_part.split_once('.') {
        Some((s, f)) => (s, f),
        None => (sec_part, "0"),
    };
    let seconds: u64 = sec_str.trim().parse().ok()?;
    let frac_trimmed = frac_str.trim();
    if !frac_trimmed.is_ascii() {
        return None;
    }
    let millis: u64 = match frac_trimmed.len() {
        0 => 0,
        1 => frac_trimmed.parse::<u64>().ok()? * 100,
        2 => frac_trimmed.parse::<u64>().ok()? * 10,
        _ => frac_trimmed.get(..3)?.parse::<u64>().ok()?,
    };
    Some(minutes * 60_000 + seconds * 1000 + millis)
}

fn extract_leading_timestamps(line: &str) -> (Vec<u64>, &str) {
    let mut timestamps = Vec::new();
    let mut rest = line;
    while rest.starts_with('[') {
        let Some(end_bracket) = rest.find(']') else {
            break;
        };
        let inside = &rest[1..end_bracket];
        if let Some(ms) = parse_timestamp_str(inside) {
            timestamps.push(ms);
            rest = &rest[end_bracket + 1..];
        } else {
            break;
        }
    }
    (timestamps, rest)
}

fn parse_enhanced_words(body: &str) -> Vec<LyricsWord> {
    let mut words = Vec::new();
    let mut rest = body.trim();
    while let Some(open) = rest.find('<') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('>') else {
            break;
        };
        let ts_str = &after_open[..close];
        let Some(start_ms) = parse_timestamp_str(ts_str) else {
            break;
        };
        let after_close = &after_open[close + 1..];
        let next_open = after_close.find('<').unwrap_or(after_close.len());
        let word_text = after_close[..next_open].trim();
        if !word_text.is_empty() {
            words.push(LyricsWord {
                text: word_text.to_string(),
                start_ms,
                end_ms: start_ms + 300,
            });
        }
        rest = &after_close[next_open..];
    }
    words
}

/// Formats milliseconds as `mm:ss.xx`.
pub fn format_timestamp(ms: u64) -> String {
    let minutes = ms / 60_000;
    let seconds = (ms % 60_000) / 1000;
    let centis = (ms % 1000) / 10;
    format!("{minutes:02}:{seconds:02}.{centis:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_enhanced_lrc_and_roundtrips() {
        let raw = "\
[offset:100]
[00:10.00] <00:10.00> Shine <00:10.50> bright <00:11.00> tonight
[00:22.00] <00:22.00> Sing <00:22.40> loud
";
        let doc = LyricsDocument::parse(raw, LyricsSource::Lrclib);
        assert_eq!(doc.format, LyricsFormat::EnhancedLrc);
        assert!(doc.has_word_sync);
        assert_eq!(doc.offset_ms, 100);
        assert_eq!(doc.lines.len(), 2);
        assert_eq!(doc.lines[0].words.len(), 3);
        assert_eq!(doc.lines[0].words[1].text, "bright");
        assert_eq!(doc.lines[0].words[1].start_ms, 10_500);

        // Verify countdown before first line (effective 7500ms -> 2500ms before 10000ms -> 3 dots)
        let pre = doc.active_window(7_400);
        assert_eq!(pre.active_index, None);
        assert_eq!(pre.countdown_dots, Some(3));

        // Verify word-level active highlight (effective 10600ms -> "bright")
        let mid = doc.active_window(10_500);
        assert_eq!(mid.active_index, Some(0));
        assert_eq!(mid.active_word_index, Some(1));

        // Roundtrip
        let serialized = doc.to_enhanced_lrc();
        let reparsed = LyricsDocument::parse(&serialized, LyricsSource::Manual);
        assert_eq!(reparsed.lines.len(), 2);
        assert_eq!(reparsed.lines[0].text, "Shine bright tonight");
    }

    #[test]
    fn shifts_offset_with_clamping() {
        let mut doc = LyricsDocument::default();
        doc.shift_offset(100);
        doc.shift_offset(-200);
        assert_eq!(doc.offset_ms, -100);
    }
}
