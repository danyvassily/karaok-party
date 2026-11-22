import { useEffect, useState } from "react";
import { CloudDownload, ExternalLink, Save, Sparkles, Timer, Wand2, X } from "lucide-react";
import { formatLrcTimestamp, serializeEnhancedLrc } from "../lib/lyricsEngine";
import type { LyricsDocument, LyricsLine, Track } from "../types/karaok";

interface SyncLyricsEditorProps {
  track: Track | null;
  lyrics: LyricsDocument | null;
  positionMs: number;
  onClose: () => void;
  onFetchLrclib: () => Promise<void>;
  onSearchGoogle: () => Promise<void>;
  onAutoAlignWhisper: (plainLyrics: string) => Promise<void>;
  onShiftOffset: (deltaMs: number) => Promise<void>;
  onSaveLyrics: (doc: LyricsDocument) => Promise<void>;
}

export function SyncLyricsEditor({
  track,
  lyrics,
  positionMs,
  onClose,
  onFetchLrclib,
  onSearchGoogle,
  onAutoAlignWhisper,
  onShiftOffset,
  onSaveLyrics,
}: SyncLyricsEditorProps) {
  const [plainText, setPlainText] = useState("");
  const [tapLines, setTapLines] = useState<LyricsLine[]>(lyrics?.lines ?? []);
  const [tapCursor, setTapCursor] = useState(0);
  const [tapMode, setTapMode] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);

  useEffect(() => {
    if (lyrics) {
      setTapLines(lyrics.lines);
      if (!plainText) {
        setPlainText(lyrics.lines.map((l) => l.text).join("\n"));
      }
    }
  }, [lyrics]);

  // Global keyboard shortcuts inside Sync mode:
  // `[` => -100 ms, `]` => +100 ms, `Space` (in Tap-to-Sync mode) => stamp current line!
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (
        e.target instanceof HTMLTextAreaElement ||
        e.target instanceof HTMLInputElement
      ) {
        return;
      }
      if (e.key === "[") {
        e.preventDefault();
        void onShiftOffset(-100);
      } else if (e.key === "]") {
        e.preventDefault();
        void onShiftOffset(100);
      } else if (e.code === "Space" && tapMode) {
        e.preventDefault();
        setTapLines((prev) => {
          if (tapCursor >= prev.length) return prev;
          const next = [...prev];
          const stamped = Math.max(0, Math.floor(positionMs));
          next[tapCursor] = {
            ...next[tapCursor],
            startMs: stamped,
            endMs: stamped + 3500,
          };
          if (tapCursor > 0) {
            next[tapCursor - 1] = {
              ...next[tapCursor - 1],
              endMs: Math.max(next[tapCursor - 1].startMs + 300, stamped),
            };
          }
          return next;
        });
        setTapCursor((c) => c + 1);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [tapMode, tapCursor, positionMs, onShiftOffset]);

  if (!track) {
    return (
      <div className="flex flex-1 items-center justify-center p-8 text-slate-400">
        Sélectionnez d'abord un morceau pour synchroniser ses paroles.
      </div>
    );
  }

  const prepareTapFromPlain = () => {
    const lines: LyricsLine[] = plainText
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean)
      .map((text, idx) => ({
        startMs: idx * 3500,
        endMs: idx * 3500 + 3000,
        text,
        words: [],
      }));
    setTapLines(lines);
    setTapCursor(0);
    setTapMode(true);
    setFeedback("Mode Tap-to-Sync actif : lancez la lecture et appuyez sur ESPACE à chaque ligne.");
  };

  const handleSave = async () => {
    setBusy("save");
    try {
      const hasWords = tapLines.some((l) => l.words.length > 0);
      const doc: LyricsDocument = {
        format: hasWords ? "enhanced_lrc" : "lrc",
        source: lyrics?.source ?? "manual",
        hasWordSync: hasWords,
        offsetMs: lyrics?.offsetMs ?? 0,
        lines: tapLines,
      };
      await onSaveLyrics(doc);
      setFeedback("Paroles enregistrées au format Enhanced LRC.");
    } catch (err) {
      setFeedback(String(err));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="flex flex-1 flex-col overflow-hidden bg-[#0A0C12] p-6">
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3 border-b border-white/8 pb-4">
        <div>
          <h2 className="text-base font-bold text-slate-50">
            Éditeur de Synchronisation — {track.title}
          </h2>
          <p className="text-xs text-slate-400">
            Raccourcis en direct :{" "}
            <kbd className="rounded bg-white/10 px-1.5 py-0.5 font-mono text-[10px]">[</kbd> -100 ms ·{" "}
            <kbd className="rounded bg-white/10 px-1.5 py-0.5 font-mono text-[10px]">]</kbd> +100 ms ·{" "}
            <kbd className="rounded bg-white/10 px-1.5 py-0.5 font-mono text-[10px]">Espace</kbd> Tap-to-Sync
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            disabled={busy !== null}
            onClick={async () => {
              setBusy("lrclib");
              setFeedback(null);
              try {
                await onFetchLrclib();
                setFeedback("Paroles récupérées depuis LRCLIB.");
              } catch (err) {
                setFeedback(String(err));
              } finally {
                setBusy(null);
              }
            }}
            className="flex items-center gap-1.5 rounded-xl border border-white/10 bg-white/5 px-3 py-2 text-xs font-semibold text-slate-100 hover:bg-white/10 disabled:opacity-50"
          >
            <CloudDownload className="h-3.5 w-3.5 text-sky-400" />
            {busy === "lrclib" ? "Recherche Web..." : "Chercher Web + LRCLIB"}
          </button>

          <button
            type="button"
            disabled={busy !== null}
            onClick={async () => {
              setFeedback(null);
              try {
                await onSearchGoogle();
                setFeedback(
                  "Recherche Google ouverte dans votre navigateur. Copiez-collez les paroles à gauche puis cliquez sur « Aligner mot-à-mot »."
                );
              } catch (err) {
                setFeedback(String(err));
              }
            }}
            className="flex items-center gap-1.5 rounded-xl border border-amber-400/35 bg-amber-400/12 px-3 py-2 text-xs font-semibold text-amber-200 hover:bg-amber-400/20 disabled:opacity-50"
          >
            <ExternalLink className="h-3.5 w-3.5" />
            Chercher sur Google
          </button>

          <button
            type="button"
            disabled={busy !== null || !plainText.trim()}
            onClick={async () => {
              setBusy("whisper");
              setFeedback(null);
              try {
                await onAutoAlignWhisper(plainText);
                setFeedback("Alignement mot-à-mot généré sur la piste vocale !");
              } catch (err) {
                setFeedback(String(err));
              } finally {
                setBusy(null);
              }
            }}
            className="flex items-center gap-1.5 rounded-xl bg-sky-400 px-3 py-2 text-xs font-bold text-slate-950 hover:bg-sky-300 disabled:opacity-50"
          >
            <Sparkles className="h-3.5 w-3.5" />
            {busy === "whisper" ? "Alignement IA..." : "Aligner mot-à-mot (Whisper)"}
          </button>

          <button
            type="button"
            onClick={handleSave}
            disabled={busy !== null || tapLines.length === 0}
            className="flex items-center gap-1.5 rounded-xl bg-emerald-400 px-3 py-2 text-xs font-bold text-slate-950 hover:bg-emerald-300 disabled:opacity-50"
          >
            <Save className="h-3.5 w-3.5" />
            Sauvegarder .LRC
          </button>

          <button
            type="button"
            onClick={onClose}
            className="rounded-xl p-2 text-slate-400 hover:bg-white/10 hover:text-slate-100"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
      </div>

      {/* Offset & Tap-to-Sync Toolbar */}
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3 rounded-xl border border-white/8 bg-[#11131B] px-4 py-2.5">
        <div className="flex items-center gap-2">
          <Timer className="h-4 w-4 text-amber-400" />
          <span className="text-xs font-semibold text-slate-200">Décalage global :</span>
          <button
            type="button"
            onClick={() => void onShiftOffset(-100)}
            className="rounded-lg bg-white/8 px-2.5 py-1 font-mono text-xs font-bold text-slate-100 hover:bg-white/15"
          >
            -100 ms [
          </button>
          <span className="min-w-[74px] text-center font-mono text-xs font-bold text-amber-300 tabular-nums">
            {(lyrics?.offsetMs ?? 0) >= 0 ? `+${lyrics?.offsetMs ?? 0}` : lyrics?.offsetMs} ms
          </span>
          <button
            type="button"
            onClick={() => void onShiftOffset(100)}
            className="rounded-lg bg-white/8 px-2.5 py-1 font-mono text-xs font-bold text-slate-100 hover:bg-white/15"
          >
            +100 ms ]
          </button>
        </div>

        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => (tapMode ? setTapMode(false) : prepareTapFromPlain())}
            className={`flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-bold transition ${
              tapMode
                ? "bg-amber-400 text-slate-950"
                : "bg-white/10 text-slate-100 hover:bg-white/15"
            }`}
          >
            <Wand2 className="h-3.5 w-3.5" />
            {tapMode ? `Tap-to-Sync actif (ligne ${tapCursor + 1}/${tapLines.length})` : "Démarrer Tap-to-Sync"}
          </button>
        </div>
      </div>

      {feedback && (
        <p className="mb-3 rounded-lg bg-sky-500/10 px-3 py-2 text-xs text-sky-200">
          {feedback}
        </p>
      )}

      {/* Split View: Raw Plain Lyrics Input vs Live Timed Enhanced LRC List */}
      <div className="grid flex-1 grid-cols-1 gap-4 overflow-hidden lg:grid-cols-2">
        <div className="flex flex-col overflow-hidden rounded-xl border border-white/8 bg-[#11131B]">
          <div className="border-b border-white/8 px-3.5 py-2 text-xs font-semibold text-slate-300">
            Paroles brutes (coller le texte ici pour alignement IA ou Tap-to-Sync)
          </div>
          <textarea
            value={plainText}
            onChange={(e) => setPlainText(e.target.value)}
            placeholder="Collez les paroles ligne par ligne..."
            className="flex-1 resize-none bg-transparent p-3.5 text-sm leading-relaxed text-slate-100 placeholder:text-slate-600 focus:outline-none"
          />
        </div>

        <div className="flex flex-col overflow-hidden rounded-xl border border-white/8 bg-[#11131B]">
          <div className="flex items-center justify-between border-b border-white/8 px-3.5 py-2 text-xs font-semibold text-slate-300">
            <span>Aperçu minuté ({tapLines.length} lignes)</span>
            <span className="font-mono text-[11px] text-sky-300 tabular-nums">
              Curseur : [{formatLrcTimestamp(positionMs)}]
            </span>
          </div>
          <div className="flex-1 space-y-1 overflow-y-auto p-3">
            {tapLines.length === 0 ? (
              <p className="py-10 text-center text-xs text-slate-500">
                Aucune ligne synchronisée. Utilisez LRCLIB, Whisper ou Tap-to-Sync.
              </p>
            ) : (
              tapLines.map((line, idx) => (
                <div
                  key={`sync-line-${idx}`}
                  onClick={() => setTapCursor(idx)}
                  className={`flex cursor-pointer items-center gap-3 rounded-lg px-2.5 py-1.5 text-xs ${
                    tapMode && idx === tapCursor
                      ? "border border-amber-400/60 bg-amber-400/15 text-amber-100"
                      : "hover:bg-white/5 text-slate-200"
                  }`}
                >
                  <span className="font-mono text-[11px] text-sky-400 tabular-nums">
                    [{formatLrcTimestamp(line.startMs)}]
                  </span>
                  <span className="flex-1 truncate">{line.text}</span>
                  {line.words.length > 0 && (
                    <span className="rounded bg-sky-400/15 px-1.5 py-0.5 text-[10px] text-sky-300">
                      {line.words.length} mots
                    </span>
                  )}
                </div>
              ))
            )}
          </div>
          {lyrics && (
            <div className="border-t border-white/8 bg-black/30 px-3.5 py-2 font-mono text-[10px] text-slate-500 truncate">
              {serializeEnhancedLrc(lyrics).slice(0, 140)}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
