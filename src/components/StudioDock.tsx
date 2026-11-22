import { useState } from "react";
import {
  Circle,
  Mic,
  Pause,
  Play,
  Repeat,
  RotateCcw,
  Sliders,
  Square,
  Tv,
  Volume2,
} from "lucide-react";
import type {
  DemucsModel,
  LoopRegion,
  PlaybackTelemetry,
  StemMix,
  Track,
} from "../types/karaok";
import { formatTimeMs } from "../lib/lyricsEngine";

interface StudioDockProps {
  telemetry: PlaybackTelemetry;
  activeTrack: Track | null;
  tvOpen: boolean;
  micDrawerOpen: boolean;
  onPlayPause: () => void;
  onSeekMs: (ms: number) => void;
  onRewind5s: () => void;
  onSetTempoPct: (pct: number) => void;
  onSetStemMix: (mix: StemMix) => void;
  onSeparateStems: (trackId: string, model: DemucsModel) => void;
  onSetAbLoop: (region: LoopRegion) => void;
  onToggleRecording: () => void;
  onToggleMicDrawer: () => void;
  onToggleTvWindow: () => void;
}

export function StudioDock({
  telemetry,
  activeTrack,
  tvOpen,
  micDrawerOpen,
  onPlayPause,
  onSeekMs,
  onRewind5s,
  onSetTempoPct,
  onSetStemMix,
  onSeparateStems,
  onSetAbLoop,
  onToggleRecording,
  onToggleMicDrawer,
  onToggleTvWindow,
}: StudioDockProps) {
  const [stemMixerOpen, setStemMixerOpen] = useState(false);
  const stemsReady =
    activeTrack?.stemStatus === "completed" || activeTrack?.stemStatus === "streaming_ready";
  const stemsProcessing = activeTrack?.stemStatus === "processing";

  const progressPct =
    telemetry.durationMs > 0
      ? Math.min(100, (telemetry.positionMs / telemetry.durationMs) * 100)
      : 0;

  const handleAbLoopClick = () => {
    const ab = telemetry.abLoop;
    if (!ab.enabled && ab.startMs === 0) {
      // Mark Point A
      onSetAbLoop({ enabled: false, startMs: telemetry.positionMs, endMs: 0 });
    } else if (!ab.enabled && ab.startMs > 0) {
      // Mark Point B and enable loop
      const end = Math.max(telemetry.positionMs, ab.startMs + 2000);
      onSetAbLoop({ enabled: true, startMs: ab.startMs, endMs: end });
    } else {
      // Clear A-B loop
      onSetAbLoop({ enabled: false, startMs: 0, endMs: 0 });
    }
  };

  const abLabel = telemetry.abLoop.enabled
    ? "A-B Actif"
    : telemetry.abLoop.startMs > 0
      ? "Définir B"
      : "Boucle A-B";

  const updateStem = (key: keyof StemMix, val: number) => {
    onSetStemMix({ ...telemetry.stemMix, [key]: val });
  };

  return (
    <footer className="relative z-20 border-t border-white/8 bg-[#0D0F16]/95 px-5 py-3 backdrop-blur-xl">
      {/* 4-Stem Mixer Popover */}
      {stemMixerOpen && (
        <div className="absolute bottom-full right-56 mb-3 w-76 rounded-2xl border border-white/12 bg-[#141722] p-4 shadow-2xl">
          <div className="mb-2 flex items-center justify-between gap-2">
            <h3 className="text-xs font-bold text-slate-100">Mixeur 4 Pistes (Demucs IA)</h3>
            {activeTrack && !stemsReady && !stemsProcessing ? (
              <button
                type="button"
                onClick={() => onSeparateStems(activeTrack.id, "htdemucs")}
                className="rounded-md bg-sky-400/20 px-2 py-0.5 text-[11px] font-semibold text-sky-300 hover:bg-sky-400/30"
              >
                Séparer IA
              </button>
            ) : (
              <button
                type="button"
                onClick={() =>
                  onSetStemMix({
                    vocals: 0.0,
                    drums: 1.0,
                    bass: 1.0,
                    other: 1.0,
                    guitar: 1.0,
                    piano: 1.0,
                  })
                }
                className="text-[11px] font-medium text-sky-400 hover:underline"
              >
                Instrumental pur
              </button>
            )}
          </div>
          {activeTrack && stemsProcessing && (
            <p className="mb-2.5 rounded-lg border border-sky-400/25 bg-sky-500/10 px-2.5 py-1.5 text-[11px] text-sky-200">
              Séparation IA Demucs en cours ({Math.round(activeTrack.stemProgress)}%) — la voix guide deviendra indépendante automatiquement.
            </p>
          )}
          {activeTrack && !stemsReady && !stemsProcessing && (
            <p className="mb-2.5 rounded-lg border border-amber-400/25 bg-amber-500/10 px-2.5 py-1.5 text-[11px] text-amber-200">
              Pistes IA non encore générées : cliquez « Séparer IA » pour isoler Voix Guide.
            </p>
          )}
          <div className="space-y-2.5">
            {(
              [
                ["vocals", "Voix originale (Guide)"],
                ["drums", "Batterie"],
                ["bass", "Basse"],
                ["other", "Instruments"],
              ] as [keyof StemMix, string][]
            ).map(([k, label]) => (
              <div key={k} className="space-y-1">
                <div className="flex justify-between text-[11px]">
                  <span className="text-slate-300">{label}</span>
                  <span className="tabular-nums text-slate-400">
                    {Math.round(telemetry.stemMix[k] * 100)} %
                  </span>
                </div>
                <input
                  type="range"
                  min={0}
                  max={130}
                  value={Math.round(telemetry.stemMix[k] * 100)}
                  onChange={(e) => updateStem(k, Number(e.target.value) / 100)}
                  className="studio-slider w-full"
                />
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Scrubber Bar */}
      <div className="mb-2.5 flex items-center gap-3">
        <span className="w-11 text-right font-mono text-[11px] text-slate-400 tabular-nums">
          {formatTimeMs(telemetry.positionMs)}
        </span>
        <div className="relative flex flex-1 items-center">
          <input
            type="range"
            min={0}
            max={Math.max(1000, telemetry.durationMs)}
            value={Math.min(telemetry.positionMs, Math.max(1000, telemetry.durationMs))}
            onChange={(e) => onSeekMs(Number(e.target.value))}
            aria-label="Position de lecture"
            className="studio-slider w-full"
            style={{
              background: `linear-gradient(90deg, #38bdf8 ${progressPct}%, rgba(255,255,255,0.12) ${progressPct}%)`,
            }}
          />
        </div>
        <span className="w-11 font-mono text-[11px] text-slate-400 tabular-nums">
          {formatTimeMs(telemetry.durationMs)}
        </span>
      </div>

      {/* Main Dock Controls */}
      <div className="flex flex-wrap items-center justify-between gap-4">
        {/* Left: Transport + Rewind 5s + A-B Loop */}
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={onRewind5s}
            title="Reculer de 5 secondes"
            className="flex h-9 items-center gap-1 rounded-xl border border-white/10 bg-white/5 px-2.5 text-xs font-semibold text-slate-200 transition hover:bg-white/10"
          >
            <RotateCcw className="h-3.5 w-3.5" />
            -5s
          </button>

          <button
            type="button"
            onClick={onPlayPause}
            aria-label={telemetry.isPlaying ? "Pause" : "Lecture"}
            className="flex h-10 w-10 items-center justify-center rounded-full bg-slate-50 text-slate-950 shadow-lg transition hover:scale-105"
          >
            {telemetry.isPlaying ? (
              <Pause className="h-5 w-5 fill-current" />
            ) : (
              <Play className="ml-0.5 h-5 w-5 fill-current" />
            )}
          </button>

          <button
            type="button"
            onClick={handleAbLoopClick}
            className={`flex h-9 items-center gap-1.5 rounded-xl border px-2.5 text-xs font-semibold transition ${
              telemetry.abLoop.enabled
                ? "border-amber-400/50 bg-amber-400/15 text-amber-300"
                : telemetry.abLoop.startMs > 0
                  ? "border-sky-400/50 bg-sky-400/15 text-sky-300"
                  : "border-white/10 bg-white/5 text-slate-300 hover:bg-white/10"
            }`}
          >
            <Repeat className="h-3.5 w-3.5" />
            {abLabel}
          </button>
        </div>

        {/* Center: Tempo + Vocal Guide + 4-Stem Mix */}
        <div className="flex flex-wrap items-center gap-5">
          {/* Tempo Control (70% .. 130%) */}
          <div className="hidden items-center gap-2 xl:flex">
            <span className="text-[11px] text-slate-400">Tempo</span>
            <input
              type="range"
              min={70}
              max={130}
              value={Math.round(telemetry.tempoPct)}
              onChange={(e) => onSetTempoPct(Number(e.target.value))}
              className="studio-slider w-20"
            />
            <span className="w-10 font-mono text-[11px] text-slate-300 tabular-nums">
              {Math.round(telemetry.tempoPct)}%
            </span>
          </div>

          {/* Vocal Guide + 4-Stem Mix Button */}
          <div className="flex items-center gap-2 rounded-xl border border-white/8 bg-[#11131B] px-3 py-1.5">
            <Volume2 className="h-3.5 w-3.5 text-sky-400" />
            <span className="text-[11px] text-slate-400">Voix Guide</span>
            {activeTrack && stemsProcessing && (
              <span className="rounded bg-sky-500/20 px-1.5 py-0.5 font-mono text-[10px] font-semibold text-sky-300 tabular-nums">
                IA {Math.round(activeTrack.stemProgress)}%
              </span>
            )}
            {activeTrack && !stemsReady && !stemsProcessing && (
              <button
                type="button"
                onClick={() => onSeparateStems(activeTrack.id, "htdemucs")}
                title="Lancer la séparation IA HTDemucs pour isoler la voix"
                className="rounded bg-amber-500/20 px-1.5 py-0.5 text-[10px] font-semibold text-amber-300 hover:bg-amber-500/30"
              >
                Séparer IA
              </button>
            )}
            <input
              type="range"
              min={0}
              max={100}
              value={Math.round(telemetry.stemMix.vocals * 100)}
              onChange={(e) => updateStem("vocals", Number(e.target.value) / 100)}
              className="studio-slider w-20"
            />
            <span className="w-8 font-mono text-[11px] text-slate-300 tabular-nums">
              {Math.round(telemetry.stemMix.vocals * 100)}%
            </span>
            <button
              type="button"
              onClick={() => setStemMixerOpen((v) => !v)}
              className={`ml-1 flex items-center gap-1 rounded-lg px-2 py-1 text-[11px] font-semibold transition ${
                stemMixerOpen
                  ? "bg-sky-400 text-slate-950"
                  : "bg-white/8 text-slate-200 hover:bg-white/15"
              }`}
            >
              <Sliders className="h-3 w-3" />
              4 Pistes
            </button>
          </div>
        </div>

        {/* Right: Record (SingSnap), Voices & Mics, TV Second Screen */}
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={onToggleRecording}
            className={`flex h-9 items-center gap-1.5 rounded-xl border px-3 text-xs font-semibold transition ${
              telemetry.isRecording
                ? "border-rose-400 bg-rose-500/20 text-rose-200"
                : "border-white/10 bg-white/5 text-slate-200 hover:bg-white/10"
            }`}
          >
            {telemetry.isRecording ? (
              <>
                <Square className="h-3.5 w-3.5 fill-rose-400 text-rose-400" />
                Stop Rec
              </>
            ) : (
              <>
                <Circle className="h-3.5 w-3.5 fill-rose-500 text-rose-500" />
                Enregistrer
              </>
            )}
          </button>

          <button
            type="button"
            onClick={onToggleMicDrawer}
            className={`flex h-9 items-center gap-2 rounded-xl border px-3 text-xs font-semibold transition ${
              micDrawerOpen
                ? "border-sky-400/60 bg-sky-400/15 text-sky-200"
                : "border-white/10 bg-white/5 text-slate-200 hover:bg-white/10"
            }`}
          >
            <Mic className="h-3.5 w-3.5" />
            Voix & Micros
            <span className="flex items-center gap-1">
              <span
                className={`h-2 w-2 rounded-full ${
                  telemetry.mic1.connected ? "bg-sky-400 shadow-[0_0_6px_#38bdf8]" : "bg-slate-600"
                }`}
              />
              <span
                className={`h-2 w-2 rounded-full ${
                  telemetry.mic2.connected
                    ? "bg-rose-400 shadow-[0_0_6px_#fb7185]"
                    : "bg-slate-600"
                }`}
              />
            </span>
          </button>

          <button
            type="button"
            onClick={onToggleTvWindow}
            title="Ouvrir la fenêtre Second Écran / TV"
            className={`flex h-9 items-center gap-1.5 rounded-xl border px-3 text-xs font-semibold transition ${
              tvOpen
                ? "border-emerald-400/50 bg-emerald-500/15 text-emerald-300"
                : "border-white/10 bg-white/5 text-slate-200 hover:bg-white/10"
            }`}
          >
            <Tv className="h-3.5 w-3.5" />
            Écran TV
          </button>
        </div>
      </div>
    </footer>
  );
}
