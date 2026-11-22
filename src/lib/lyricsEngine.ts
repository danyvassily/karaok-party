import type { LyricsDocument, MusicalKey, PitchClass } from "../types/karaok";

const PITCH_LABELS: Record<PitchClass, string> = {
  C: "C",
  CSharp: "C#",
  D: "D",
  DSharp: "D#",
  E: "E",
  F: "F",
  FSharp: "F#",
  G: "G",
  GSharp: "G#",
  A: "A",
  ASharp: "A#",
  B: "B",
};

const NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

export function formatMusicalKey(key: MusicalKey): string {
  const root = PITCH_LABELS[key.root] ?? "C";
  const scale = key.scale === "minor" ? "Min" : key.scale === "chromatic" ? "Chrom." : "Maj";
  return `${root} ${scale}`;
}

export function midiToNoteLabel(midi: number): string {
  if (!midi || midi <= 0) return "--";
  const note = NOTE_NAMES[((midi % 12) + 12) % 12];
  const octave = Math.floor(midi / 12) - 1;
  return `${note}${octave}`;
}

export function formatTimeMs(ms: number): string {
  const safe = Math.max(0, Math.floor(ms));
  const mins = Math.floor(safe / 60000);
  const secs = Math.floor((safe % 60000) / 1000);
  return `${String(mins).padStart(2, "0")}:${String(secs).padStart(2, "0")}`;
}

export function formatLrcTimestamp(ms: number): string {
  const safe = Math.max(0, Math.floor(ms));
  const mins = Math.floor(safe / 60000);
  const secs = Math.floor((safe % 60000) / 1000);
  const centis = Math.floor((safe % 1000) / 10);
  return `${String(mins).padStart(2, "0")}:${String(secs).padStart(2, "0")}.${String(centis).padStart(2, "0")}`;
}

export interface StageLyricsState {
  activeIndex: number;
  isBeforeFirstLine: boolean;
  lineProgress: number;
  activeWordIndex: number | null;
  wordProgress: number;
  countdownDots: number | null;
}

/**
 * Computes the exact 5-line window cursor, word-by-word fill progress,
 * and 3-dot instrumental reprise countdown synchronized to CoreAudio's clock.
 */
export function computeStageLyricsState(
  doc: LyricsDocument | null,
  positionMs: number
): StageLyricsState {
  if (!doc || doc.lines.length === 0) {
    return {
      activeIndex: 0,
      isBeforeFirstLine: true,
      lineProgress: 0,
      activeWordIndex: null,
      wordProgress: 0,
      countdownDots: null,
    };
  }

  const effectiveMs = Math.max(0, positionMs + doc.offsetMs);
  const firstStart = doc.lines[0].startMs;

  if (effectiveMs < firstStart) {
    const until = firstStart - effectiveMs;
    const countdownDots =
      firstStart >= 3000 && until > 0 && until <= 3000
        ? until > 2000
          ? 3
          : until > 1000
            ? 2
            : 1
        : null;
    return {
      activeIndex: 0,
      isBeforeFirstLine: true,
      lineProgress: 0,
      activeWordIndex: null,
      wordProgress: 0,
      countdownDots,
    };
  }

  let activeIdx = 0;
  for (let i = 0; i < doc.lines.length; i++) {
    if (doc.lines[i].startMs <= effectiveMs) {
      activeIdx = i;
    } else {
      break;
    }
  }

  const line = doc.lines[activeIdx];
  const duration = Math.max(1, line.endMs - line.startMs);
  const lineProgress = Math.max(0, Math.min(1, (effectiveMs - line.startMs) / duration));

  let countdownDots: number | null = null;
  const nextLine = doc.lines[activeIdx + 1];
  if (nextLine && effectiveMs >= line.endMs && effectiveMs < nextLine.startMs) {
    const gap = nextLine.startMs - line.endMs;
    const until = nextLine.startMs - effectiveMs;
    if (gap >= 3500 && until > 0 && until <= 3000) {
      countdownDots = until > 2000 ? 3 : until > 1000 ? 2 : 1;
    }
  }

  let activeWordIndex: number | null = null;
  let wordProgress = lineProgress;
  if (line.words.length > 0) {
    let wi = 0;
    for (let i = 0; i < line.words.length; i++) {
      if (line.words[i].startMs <= effectiveMs) {
        wi = i;
      } else {
        break;
      }
    }
    activeWordIndex = wi;
    const word = line.words[wi];
    const wDur = Math.max(1, word.endMs - word.startMs);
    wordProgress = Math.max(0, Math.min(1, (effectiveMs - word.startMs) / wDur));
  }

  return {
    activeIndex: activeIdx,
    isBeforeFirstLine: false,
    lineProgress,
    activeWordIndex,
    wordProgress,
    countdownDots,
  };
}

/**
 * Serializes a LyricsDocument back into Enhanced LRC (`[mm:ss.xx] <mm:ss.xx> Word ...`).
 */
export function serializeEnhancedLrc(doc: LyricsDocument): string {
  const rows: string[] = [];
  if (doc.offsetMs !== 0) {
    rows.push(`[offset:${doc.offsetMs}]`);
  }
  for (const line of doc.lines) {
    const head = `[${formatLrcTimestamp(line.startMs)}]`;
    if (line.words.length > 0) {
      const body = line.words
        .map((w) => `<${formatLrcTimestamp(w.startMs)}> ${w.text}`)
        .join(" ");
      rows.push(`${head} ${body}`);
    } else {
      rows.push(`${head} ${line.text}`);
    }
  }
  return rows.join("\n");
}
