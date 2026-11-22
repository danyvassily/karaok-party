import { useEffect, useMemo, useState } from "react";
import { computeStageLyricsState, midiToNoteLabel } from "../lib/lyricsEngine";
import type { LyricsDocument, LyricsLine, MicTelemetry } from "../types/karaok";

interface LyricsStageProps {
  lyrics: LyricsDocument | null;
  positionMs: number;
  mic1: MicTelemetry;
  mic2: MicTelemetry;
  showPitchGauge: boolean;
  tvMode?: boolean;
  trackId?: string;
  trackTitle?: string;
  trackArtist?: string;
  onOpenSync?: () => void;
  onFetchWebLyrics?: () => Promise<void>;
  onSearchGoogle?: () => Promise<void>;
  onSavePastedLyrics?: (plainText: string) => Promise<void>;
}

export function LyricsStage({
  lyrics,
  positionMs,
  mic1,
  mic2,
  showPitchGauge,
  tvMode = false,
  trackId,
  trackTitle,
  trackArtist,
  onOpenSync,
  onFetchWebLyrics,
  onSearchGoogle,
  onSavePastedLyrics,
}: LyricsStageProps) {
  const [showPasteBox, setShowPasteBox] = useState(false);
  const [pastedText, setPastedText] = useState("");
  const [busyAction, setBusyAction] = useState<"web" | "save" | null>(null);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);

  useEffect(() => {
    setShowPasteBox(false);
    setPastedText("");
    setStatusMessage(null);
    setBusyAction(null);
  }, [trackId]);

  const stage = useMemo(
    () => computeStageLyricsState(lyrics, positionMs),
    [lyrics, positionMs]
  );

  // Build exact 5-line window: [active - 2, active - 1, active, active + 1, active + 2]
  const slots = useMemo(() => {
    if (!lyrics || lyrics.lines.length === 0) return [];
    const center = stage.isBeforeFirstLine ? 0 : stage.activeIndex;
    return [-2, -1, 0, 1, 2].map((offset) => {
      const idx = center + offset;
      const line: LyricsLine | null =
        idx >= 0 && idx < lyrics.lines.length ? lyrics.lines[idx] : null;
      return { offset, idx, line };
    });
  }, [lyrics, stage.activeIndex, stage.isBeforeFirstLine]);

  if (!lyrics || lyrics.lines.length === 0) {
    return (
      <div className="flex flex-1 flex-col items-center justify-center overflow-y-auto px-8 py-6 text-center">
        <div className="w-full max-w-xl space-y-4">
          <p className="text-3xl font-semibold tracking-tight text-slate-100">
            {trackTitle ? trackTitle : "Prêt à chanter"}
          </p>
          <p className="text-sm text-slate-400">
            {trackTitle
              ? `Aucune parole enregistrée dans votre bibliothèque pour « ${trackTitle} » (${trackArtist ?? "Artiste"}).`
              : "Lancez un morceau depuis la bibliothèque : ses paroles enregistrées s'afficheront automatiquement ici."}
          </p>

          {trackTitle && !tvMode && (
            <div className="space-y-4 pt-1">
              <div className="flex flex-wrap items-center justify-center gap-2.5">
                {onSearchGoogle && (
                  <button
                    type="button"
                    onClick={async () => {
                      setShowPasteBox(true);
                      setStatusMessage(
                        "Google est ouvert : copiez les paroles puis collez-les ci-dessous pour les enregistrer définitivement avec cette musique."
                      );
                      await onSearchGoogle();
                    }}
                    className="inline-flex items-center gap-2 rounded-xl bg-sky-400 px-4 py-2.5 text-sm font-bold text-slate-950 transition hover:bg-sky-300"
                  >
                    Chercher sur Google
                  </button>
                )}

                {onSavePastedLyrics && (
                  <button
                    type="button"
                    onClick={() => setShowPasteBox((v) => !v)}
                    className="inline-flex items-center gap-2 rounded-xl border border-white/15 bg-white/8 px-4 py-2.5 text-sm font-semibold text-slate-100 transition hover:bg-white/15"
                  >
                    Coller / Écrire moi-même
                  </button>
                )}

                {onFetchWebLyrics && (
                  <button
                    type="button"
                    disabled={busyAction !== null}
                    onClick={async () => {
                      setBusyAction("web");
                      setStatusMessage(null);
                      try {
                        await onFetchWebLyrics();
                      } catch (err) {
                        setShowPasteBox(true);
                        setStatusMessage(String(err));
                      } finally {
                        setBusyAction(null);
                      }
                    }}
                    className="inline-flex items-center gap-2 rounded-xl border border-white/10 px-4 py-2.5 text-sm font-medium text-slate-300 transition hover:bg-white/8 disabled:opacity-50"
                  >
                    {busyAction === "web" ? "Recherche en cours..." : "Recherche auto"}
                  </button>
                )}
              </div>

              {statusMessage && (
                <p className="rounded-xl bg-sky-500/10 px-4 py-2.5 text-xs text-sky-200">
                  {statusMessage}
                </p>
              )}

              {showPasteBox && onSavePastedLyrics && (
                <div className="space-y-3 rounded-2xl border border-white/10 bg-[#11131B] p-4 text-left shadow-xl">
                  <div className="flex items-center justify-between">
                    <span className="text-xs font-semibold text-slate-200">
                      Collez ou écrivez les paroles (liées définitivement à « {trackTitle} »)
                    </span>
                    {onOpenSync && (
                      <button
                        type="button"
                        onClick={onOpenSync}
                        className="text-[11px] font-medium text-sky-400 hover:underline"
                      >
                        Éditeur avancé
                      </button>
                    )}
                  </div>
                  <textarea
                    rows={7}
                    value={pastedText}
                    onChange={(e) => setPastedText(e.target.value)}
                    placeholder="Collez ici les paroles copiées depuis Google (ou un texte .lrc)..."
                    className="w-full resize-none rounded-xl border border-white/10 bg-[#07080C] p-3 text-sm leading-relaxed text-slate-100 placeholder:text-slate-500 focus:border-sky-400/60 focus:outline-none"
                  />
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-[11px] text-slate-400">
                      Synchronisation automatique IA + sauvegarde permanente SQLite
                    </span>
                    <button
                      type="button"
                      disabled={busyAction !== null || !pastedText.trim()}
                      onClick={async () => {
                        setBusyAction("save");
                        setStatusMessage(null);
                        try {
                          await onSavePastedLyrics(pastedText);
                        } catch (err) {
                          setStatusMessage(String(err));
                        } finally {
                          setBusyAction(null);
                        }
                      }}
                      className="rounded-xl bg-emerald-400 px-4 py-2 text-xs font-bold text-slate-950 transition hover:bg-emerald-300 disabled:opacity-40"
                    >
                      {busyAction === "save"
                        ? "Sauvegarde & Sync..."
                        : "Enregistrer dans la bibliothèque"}
                    </button>
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="relative flex flex-1 flex-col items-center justify-center overflow-hidden px-8 py-6">
      {/* 3-dot visual countdown before singing resumes */}
      <div className="mb-6 flex h-10 items-center justify-center gap-3" aria-live="polite">
        {stage.countdownDots !== null && (
          <div className="flex items-center gap-3 rounded-full border border-sky-400/30 bg-sky-500/10 px-5 py-2">
            {[3, 2, 1].map((dot) => {
              const active = dot <= (stage.countdownDots ?? 0);
              return (
                <span
                  key={dot}
                  className={`h-3.5 w-3.5 rounded-full transition-all duration-150 ${
                    active
                      ? "scale-110 bg-sky-400 shadow-[0_0_14px_rgba(56,189,248,0.85)]"
                      : "scale-90 bg-slate-700"
                  }`}
                />
              );
            })}
            <span className="ml-1 text-xs font-medium text-sky-300">
              Reprise du chant
            </span>
          </div>
        )}
      </div>

      {/* Exact 5-line centered karaoke lyrics display */}
      <div
        className={`flex w-full max-w-5xl flex-col items-center justify-center text-center ${
          tvMode ? "gap-7" : "gap-5"
        }`}
      >
        {slots.map(({ offset, idx, line }) => {
          if (!line) {
            return (
              <div
                key={`empty-${offset}`}
                className={tvMode ? "h-12" : "h-9"}
                aria-hidden="true"
              />
            );
          }

          const isPast = offset < 0;
          const isUpcoming = offset > 0 || (offset === 0 && stage.isBeforeFirstLine);

          if (isPast) {
            return (
              <p
                key={`line-${idx}`}
                className={`max-w-4xl truncate font-medium text-slate-300/30 transition-all duration-200 ${
                  tvMode ? "text-3xl" : "text-xl md:text-2xl"
                }`}
              >
                {line.text}
              </p>
            );
          }

          if (isUpcoming) {
            return (
              <p
                key={`line-${idx}`}
                className={`max-w-4xl font-medium text-slate-200/65 transition-all duration-200 ${
                  tvMode ? "text-4xl" : "text-2xl md:text-3xl"
                }`}
              >
                {line.text}
              </p>
            );
          }

          // Active line with smooth word-by-word or line progressive sweep
          return (
            <div
              key={`line-${idx}`}
              className={`max-w-5xl font-bold leading-snug tracking-tight transition-transform duration-150 ${
                tvMode ? "text-6xl" : "text-4xl md:text-5xl"
              }`}
            >
              {line.words.length > 0 ? (
                <span className="inline-flex flex-wrap justify-center gap-x-3 gap-y-1">
                  {line.words.map((word, wIdx) => {
                    const activeW = stage.activeWordIndex ?? -1;
                    const fillPct =
                      wIdx < activeW
                        ? 100
                        : wIdx === activeW
                          ? Math.round(stage.wordProgress * 100)
                          : 0;
                    return (
                      <span
                        key={`${idx}-w-${wIdx}`}
                        className="relative inline-block"
                        style={{
                          backgroundImage: `linear-gradient(90deg, #38bdf8 0%, #7dd3fc ${fillPct}%, #f8fafc ${fillPct}%, #f8fafc 100%)`,
                          WebkitBackgroundClip: "text",
                          WebkitTextFillColor: "transparent",
                          filter:
                            wIdx === activeW
                              ? "drop-shadow(0 0 18px rgba(56,189,248,0.45))"
                              : "none",
                        }}
                      >
                        {word.text}
                      </span>
                    );
                  })}
                </span>
              ) : (
                <span
                  style={{
                    backgroundImage: `linear-gradient(90deg, #38bdf8 0%, #7dd3fc ${Math.round(
                      stage.lineProgress * 100
                    )}%, #f8fafc ${Math.round(stage.lineProgress * 100)}%, #f8fafc 100%)`,
                    WebkitBackgroundClip: "text",
                    WebkitTextFillColor: "transparent",
                    filter: "drop-shadow(0 0 18px rgba(56,189,248,0.35))",
                  }}
                >
                  {line.text}
                </span>
              )}
            </div>
          );
        })}
      </div>

      {/* Subtle toggleable Live Pitch Accuracy Gauge */}
      {showPitchGauge && (
        <div className="mt-10 flex flex-wrap items-center justify-center gap-4">
          <PitchPill label="Micro 1" accent="sky" mic={mic1} />
          {mic2.connected && <PitchPill label="Micro 2" accent="coral" mic={mic2} />}
        </div>
      )}
    </div>
  );
}

function PitchPill({
  label,
  accent,
  mic,
}: {
  label: string;
  accent: "sky" | "coral";
  mic: MicTelemetry;
}) {
  const voiced = mic.pitchHz > 65;
  const cents = Math.max(-50, Math.min(50, mic.pitchCents));
  const markerPct = 50 + cents; // 0..100%
  const inTune = voiced && Math.abs(cents) <= 15;

  return (
    <div className="flex items-center gap-3 rounded-full border border-white/10 bg-[#11131B]/90 px-4 py-1.5 backdrop-blur">
      <span
        className={`h-2 w-2 rounded-full ${
          accent === "sky" ? "bg-sky-400" : "bg-rose-400"
        }`}
      />
      <span className="text-xs font-medium text-slate-400">{label}</span>
      <span
        className={`w-10 text-center font-mono text-sm font-bold tabular-nums ${
          inTune ? "text-emerald-400" : voiced ? "text-amber-300" : "text-slate-500"
        }`}
      >
        {voiced ? midiToNoteLabel(mic.pitchMidi) : "--"}
      </span>
      <div className="relative h-2 w-28 overflow-hidden rounded-full bg-white/10">
        <div className="absolute inset-y-0 left-1/2 w-0.5 -translate-x-1/2 bg-emerald-400/70" />
        {voiced && (
          <div
            className={`absolute top-0 h-2 w-2.5 -translate-x-1/2 rounded-full transition-all duration-75 ${
              inTune ? "bg-emerald-400" : "bg-amber-400"
            }`}
            style={{ left: `${markerPct}%` }}
          />
        )}
      </div>
    </div>
  );
}
