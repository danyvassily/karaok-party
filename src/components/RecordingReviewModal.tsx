import { useEffect, useState } from "react";
import { Download, Disc3, Play, X } from "lucide-react";
import { formatTimeMs } from "../lib/lyricsEngine";
import type { RecordingEntry } from "../types/karaok";

interface RecordingReviewModalProps {
  recording: RecordingEntry | null;
  onClose: () => void;
  onPreviewMix: (recordingId: string, vocalBalance: number) => Promise<void>;
  onExportMix: (recordingId: string, vocalBalance: number, format: "wav" | "mp3") => Promise<string>;
}

export function RecordingReviewModal({
  recording,
  onClose,
  onPreviewMix,
  onExportMix,
}: RecordingReviewModalProps) {
  const [vocalBalance, setVocalBalance] = useState(recording?.mixVocalBalance ?? 0.55);
  const [format, setFormat] = useState<"wav" | "mp3">("wav");
  const [previewing, setPreviewing] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [exportedPath, setExportedPath] = useState<string | null>(
    recording?.exportedPath ?? null
  );
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setVocalBalance(recording?.mixVocalBalance ?? 0.55);
    setExportedPath(recording?.exportedPath ?? null);
    setError(null);
  }, [recording?.id]);

  if (!recording) return null;

  const handlePreview = async () => {
    setPreviewing(true);
    setError(null);
    try {
      await onPreviewMix(recording.id, vocalBalance);
    } catch (err) {
      setError(String(err));
    } finally {
      setPreviewing(false);
    }
  };

  const handleExport = async () => {
    setExporting(true);
    setError(null);
    try {
      const path = await onExportMix(recording.id, vocalBalance, format);
      setExportedPath(path);
    } catch (err) {
      setError(String(err));
    } finally {
      setExporting(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/75 p-4 backdrop-blur-sm">
      <div className="w-full max-w-md rounded-2xl border border-white/12 bg-[#11131B] p-6 shadow-2xl">
        <div className="mb-4 flex items-center justify-between">
          <div className="flex items-center gap-2.5">
            <Disc3 className="h-5 w-5 text-rose-400" />
            <div>
              <h3 className="text-sm font-bold text-slate-50">
                Prise enregistrée — {recording.title}
              </h3>
              <p className="text-xs text-slate-400 tabular-nums">
                Durée : {formatTimeMs(recording.durationMs)} · Multipiste (Instru + Mic 1 + Mic 2)
              </p>
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded-lg p-1.5 text-slate-400 hover:bg-white/10 hover:text-slate-100"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        {/* Post-Mix Vocal vs Music Balance Slider (SingSnap workflow) */}
        <div className="space-y-3 rounded-xl border border-white/8 bg-[#090A0F] p-4">
          <div className="flex justify-between text-xs">
            <span className="font-semibold text-slate-200">
              Balance Voix / Musique (Post-Mix)
            </span>
            <span className="font-mono text-sky-300 tabular-nums">
              {Math.round(vocalBalance * 100)} % voix
            </span>
          </div>
          <input
            type="range"
            min={0}
            max={100}
            value={Math.round(vocalBalance * 100)}
            onChange={(e) => setVocalBalance(Number(e.target.value) / 100)}
            className="studio-slider w-full"
          />
          <div className="flex justify-between text-[10px] text-slate-500">
            <span>Plus d'instrumental</span>
            <span>Équilibré</span>
            <span>Voix en avant</span>
          </div>
        </div>

        {/* Format Selection */}
        <div className="mt-4 flex items-center justify-between">
          <span className="text-xs font-medium text-slate-300">Format d'exportation</span>
          <div className="flex rounded-lg bg-[#090A0F] p-1 text-xs">
            <button
              type="button"
              onClick={() => setFormat("wav")}
              className={`rounded-md px-3 py-1 font-semibold ${
                format === "wav" ? "bg-sky-400 text-slate-950" : "text-slate-400"
              }`}
            >
              WAV (Studio)
            </button>
            <button
              type="button"
              onClick={() => setFormat("mp3")}
              className={`rounded-md px-3 py-1 font-semibold ${
                format === "mp3" ? "bg-sky-400 text-slate-950" : "text-slate-400"
              }`}
            >
              MP3 (256k)
            </button>
          </div>
        </div>

        {exportedPath && (
          <p className="mt-3 rounded-lg bg-emerald-500/15 px-3 py-2 text-xs text-emerald-300 break-all">
            Exporté vers : {exportedPath}
          </p>
        )}
        {error && (
          <p className="mt-3 rounded-lg bg-rose-500/15 px-3 py-2 text-xs text-rose-300">
            {error}
          </p>
        )}

        <div className="mt-5 flex flex-wrap items-center justify-between gap-2">
          <button
            type="button"
            disabled={previewing}
            onClick={handlePreview}
            className="flex items-center gap-1.5 rounded-xl border border-sky-400/40 bg-sky-400/15 px-3.5 py-2 text-xs font-semibold text-sky-200 hover:bg-sky-400/25 disabled:opacity-50"
          >
            <Play className="h-3.5 w-3.5 fill-current" />
            {previewing ? "Chargement..." : "Réécouter"}
          </button>
          <div className="flex gap-2">
            <button
              type="button"
              onClick={onClose}
              className="rounded-xl border border-white/10 px-4 py-2 text-xs font-semibold text-slate-300 hover:bg-white/5"
            >
              Fermer
            </button>
            <button
              type="button"
              disabled={exporting}
              onClick={handleExport}
              className="flex items-center gap-1.5 rounded-xl bg-sky-400 px-4 py-2 text-xs font-bold text-slate-950 hover:bg-sky-300 disabled:opacity-50"
            >
              <Download className="h-3.5 w-3.5" />
              {exporting ? "Mixage..." : "Exporter"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
