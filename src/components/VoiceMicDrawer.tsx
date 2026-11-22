import { useState } from "react";
import { Mic, MicOff, ShieldCheck, Sliders, X } from "lucide-react";
import type {
  MicChannelId,
  MicPairingInfo,
  MicTelemetry,
  ScaleType,
  VocalDspParams,
  VocalPreset,
} from "../types/karaok";

interface VoiceMicDrawerProps {
  open: boolean;
  pairingInfo: MicPairingInfo | null;
  mic1Telemetry: MicTelemetry;
  mic2Telemetry: MicTelemetry;
  mic1Params: VocalDspParams;
  mic2Params: VocalDspParams;
  latencyCalibrationMs: number;
  bufferSize: string;
  onClose: () => void;
  onSelectPreset: (ch: MicChannelId, preset: VocalPreset) => void;
  onUpdateParams: (ch: MicChannelId, params: VocalDspParams) => void;
  onSetLatencyMs: (ms: number) => void;
}

const PRESETS: { id: VocalPreset; label: string; desc: string }[] = [
  { id: "natural", label: "Naturel", desc: "Comp. légère · 0 % tune" },
  { id: "studio", label: "Studio", desc: "Chaleur · 85 ms · Plate" },
  { id: "live", label: "Scène / Live", desc: "Grande salle · 50 ms" },
  { id: "popStar", label: "Pop Star", desc: "Autotune 10 ms · Brillant" },
  { id: "expert", label: "Mode Expert", desc: "Tous les réglages DSP" },
];

export function VoiceMicDrawer({
  open,
  pairingInfo,
  mic1Telemetry,
  mic2Telemetry,
  mic1Params,
  mic2Params,
  latencyCalibrationMs,
  bufferSize,
  onClose,
  onSelectPreset,
  onUpdateParams,
  onSetLatencyMs,
}: VoiceMicDrawerProps) {
  const [activeMic, setActiveMic] = useState<MicChannelId>("mic1");

  if (!open) return null;

  const activeParams = activeMic === "mic1" ? mic1Params : mic2Params;
  const isExpert = activeParams.preset === "expert";

  const patchParam = <K extends keyof VocalDspParams>(key: K, value: VocalDspParams[K]) => {
    onUpdateParams(activeMic, { ...activeParams, [key]: value });
  };

  return (
    <div className="fixed inset-y-0 right-0 z-40 flex w-[460px] max-w-[95vw] flex-col border-l border-white/10 bg-[#0D0F16]/98 shadow-2xl backdrop-blur-xl">
      {/* Header */}
      <div className="flex items-center justify-between border-b border-white/8 px-5 py-4">
        <div>
          <h2 className="text-sm font-bold text-slate-50">
            Voix & Microphones iPhone (Mode Duo)
          </h2>
          <p className="text-xs text-slate-400">
            Sans application iOS · HTTPS/WSS local · CoreAudio 48 kHz ({bufferSize.replace("Frames", "")} éch.)
          </p>
        </div>
        <button
          type="button"
          onClick={onClose}
          aria-label="Fermer le tiroir micros"
          className="rounded-lg p-1.5 text-slate-400 hover:bg-white/10 hover:text-slate-100"
        >
          <X className="h-4 w-4" />
        </button>
      </div>

      <div className="flex-1 space-y-5 overflow-y-auto p-5">
        {/* Pairing Cards: Mic 1 (Blue) & Mic 2 (Coral) */}
        <section className="grid grid-cols-2 gap-3">
          <MicCard
            label="Micro 1 (Bleu)"
            accent="sky"
            selected={activeMic === "mic1"}
            telemetry={mic1Telemetry}
            params={mic1Params}
            qrSvg={pairingInfo?.mic1QrSvg ?? null}
            url={pairingInfo?.mic1Url ?? "https://karaok-party.local:8443/mic/1"}
            onSelect={() => setActiveMic("mic1")}
            onToggleMute={() =>
              onUpdateParams("mic1", { ...mic1Params, muted: !mic1Params.muted })
            }
          />
          <MicCard
            label="Micro 2 (Corail)"
            accent="coral"
            selected={activeMic === "mic2"}
            telemetry={mic2Telemetry}
            params={mic2Params}
            qrSvg={pairingInfo?.mic2QrSvg ?? null}
            url={pairingInfo?.mic2Url ?? "https://karaok-party.local:8443/mic/2"}
            onSelect={() => setActiveMic("mic2")}
            onToggleMute={() =>
              onUpdateParams("mic2", { ...mic2Params, muted: !mic2Params.muted })
            }
          />
        </section>

        {/* 1-tap iPhone Local CA Certificate Helper (plain HTTP :8444 — zero red TLS warning) */}
        <section className="rounded-xl border border-sky-400/25 bg-sky-500/8 p-3.5">
          <div className="flex items-center gap-3">
            {pairingInfo?.caCertQrSvg && (
              <div className="h-24 w-24 shrink-0 overflow-hidden rounded-lg bg-white p-1.5">
                <img
                  src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(
                    pairingInfo.caCertQrSvg
                  )}`}
                  alt="QR Code installation certificat iPhone"
                  className="h-full w-full object-contain"
                />
              </div>
            )}
            <div className="min-w-0 flex-1 space-y-1.5">
              <div className="flex items-center gap-1.5">
                <ShieldCheck className="h-4 w-4 shrink-0 text-sky-400" />
                <p className="text-xs font-bold text-slate-100">
                  1re connexion iPhone (Sans avertissement)
                </p>
              </div>
              <p className="text-[11px] leading-relaxed text-slate-300">
                Scannez ce QR (ouvrir dans <strong>Safari</strong>) ou visitez{" "}
                <code className="rounded bg-black/40 px-1.5 py-0.5 text-[10px] text-sky-300 break-all">
                  {pairingInfo?.caCertUrl ?? "http://karaok-party.local:8444/cert/install"}
                </code>{" "}
                pour installer <strong>Karaok Party Local CA (.cer)</strong> en 1 tap, puis activez-le dans{" "}
                <em>Réglages → Informations → Réglages des certificats</em>.
              </p>
            </div>
          </div>
        </section>

        {/* Vocal Presets for selected microphone */}
        <section className="space-y-2.5">
          <div className="flex items-center justify-between">
            <h3 className="text-xs font-bold text-slate-200">
              Préréglage vocal —{" "}
              <span className={activeMic === "mic1" ? "text-sky-400" : "text-rose-400"}>
                {activeMic === "mic1" ? "Micro 1" : "Micro 2"}
              </span>
            </h3>
            <span className="text-[11px] text-slate-400">
              Autotune lié à la tonalité
            </span>
          </div>

          <div className="grid grid-cols-2 gap-2">
            {PRESETS.map((p) => {
              const selected = activeParams.preset === p.id;
              return (
                <button
                  key={p.id}
                  type="button"
                  onClick={() => onSelectPreset(activeMic, p.id)}
                  className={`rounded-xl border p-2.5 text-left transition ${
                    selected
                      ? activeMic === "mic1"
                        ? "border-sky-400 bg-sky-400/15"
                        : "border-rose-400 bg-rose-400/15"
                      : "border-white/8 bg-[#11131B] hover:border-white/20"
                  }`}
                >
                  <p className="text-xs font-bold text-slate-100">{p.label}</p>
                  <p className="mt-0.5 text-[10px] text-slate-400">{p.desc}</p>
                </button>
              );
            })}
          </div>
        </section>

        {/* Expert DSP Controls (or quick Autotune / Reverb sliders) */}
        <section className="space-y-3 rounded-xl border border-white/8 bg-[#11131B] p-4">
          <div className="flex items-center justify-between">
            <span className="flex items-center gap-1.5 text-xs font-bold text-slate-100">
              <Sliders className="h-3.5 w-3.5 text-sky-400" />
              {isExpert ? "Chaîne DSP complète (Mode Expert)" : "Ajustements rapides"}
            </span>
            <label className="flex items-center gap-1.5 text-[11px] text-slate-300">
              <input
                type="checkbox"
                checked={activeParams.autotuneEnabled}
                onChange={(e) => patchParam("autotuneEnabled", e.target.checked)}
                className="accent-sky-400"
              />
              Autotune
            </label>
          </div>

          <ParamSlider
            label="Volume Micro"
            value={Math.round(activeParams.gain * 100)}
            min={0}
            max={200}
            unit="%"
            onChange={(v) => patchParam("gain", v / 100)}
          />
          <ParamSlider
            label="Vitesse Autotune (Retune Speed)"
            value={Math.round(activeParams.retuneSpeedMs)}
            min={0}
            max={120}
            unit="ms"
            onChange={(v) => patchParam("retuneSpeedMs", v)}
          />
          <ParamSlider
            label="Dosage Autotune (Dry/Wet)"
            value={Math.round(activeParams.autotuneMix * 100)}
            min={0}
            max={100}
            unit="%"
            onChange={(v) => patchParam("autotuneMix", v / 100)}
          />
          <ParamSlider
            label="Réverbération (Wet)"
            value={Math.round(activeParams.reverbWet * 100)}
            min={0}
            max={80}
            unit="%"
            onChange={(v) => patchParam("reverbWet", v / 100)}
          />

          {isExpert && (
            <div className="space-y-3 border-t border-white/8 pt-3">
              <div className="flex items-center justify-between text-xs">
                <span className="text-slate-300">Gamme Autotune</span>
                <select
                  value={activeParams.scale}
                  onChange={(e) => patchParam("scale", e.target.value as ScaleType)}
                  className="rounded-lg border border-white/10 bg-[#07080C] px-2 py-1 text-xs text-slate-100"
                >
                  <option value="chromatic">Chromatique</option>
                  <option value="major">Majeure (auto tonalité)</option>
                  <option value="minor">Mineure (auto tonalité)</option>
                </select>
              </div>
              <ParamSlider
                label="Passe-Haut (Anti-manipulation)"
                value={Math.round(activeParams.hpfHz)}
                min={40}
                max={160}
                unit="Hz"
                onChange={(v) => patchParam("hpfHz", v)}
              />
              <ParamSlider
                label="Noise Gate (Anti-Larsen)"
                value={Math.round(activeParams.gateThresholdDb)}
                min={-65}
                max={-18}
                unit="dB"
                onChange={(v) => patchParam("gateThresholdDb", v)}
              />
              <ParamSlider
                label="Compresseur (Ratio)"
                value={Math.round(activeParams.compRatio * 10)}
                min={10}
                max={60}
                unit="x0.1"
                onChange={(v) => patchParam("compRatio", v / 10)}
              />
              <ParamSlider
                label="Taille Salle Réverb (Room)"
                value={Math.round(activeParams.reverbRoomSize * 100)}
                min={10}
                max={95}
                unit="%"
                onChange={(v) => patchParam("reverbRoomSize", v / 100)}
              />
              <ParamSlider
                label="Amortissement Réverb (Damping)"
                value={Math.round(activeParams.reverbDamping * 100)}
                min={0}
                max={100}
                unit="%"
                onChange={(v) => patchParam("reverbDamping", v / 100)}
              />
              <ParamSlider
                label="Brillance Vocale (6 kHz)"
                value={Math.round(activeParams.presenceDb * 10)}
                min={-40}
                max={60}
                unit="x0.1 dB"
                onChange={(v) => patchParam("presenceDb", v / 10)}
              />
            </div>
          )}
        </section>

        {/* Latency Calibration (0..250 ms) for Soundbars / Bluetooth */}
        <section className="space-y-2 rounded-xl border border-white/8 bg-[#11131B] p-4">
          <div className="flex items-center justify-between">
            <h3 className="text-xs font-bold text-slate-100">
              Calibration Latence (Barre de son / Bluetooth)
            </h3>
            <span className="font-mono text-xs font-bold text-amber-300 tabular-nums">
              {latencyCalibrationMs} ms
            </span>
          </div>
          <input
            type="range"
            min={0}
            max={250}
            step={5}
            value={latencyCalibrationMs}
            onChange={(e) => onSetLatencyMs(Number(e.target.value))}
            className="studio-slider w-full"
          />
          <p className="text-[11px] text-slate-400">
            Compense le décalage audio entre la sortie Mac et les paroles affichées.
          </p>
        </section>
      </div>
    </div>
  );
}

function MicCard({
  label,
  accent,
  selected,
  telemetry,
  params,
  qrSvg,
  url,
  onSelect,
  onToggleMute,
}: {
  label: string;
  accent: "sky" | "coral";
  selected: boolean;
  telemetry: MicTelemetry;
  params: VocalDspParams;
  qrSvg: string | null;
  url: string;
  onSelect: () => void;
  onToggleMute: () => void;
}) {
  const vuPct = Math.max(0, Math.min(100, ((telemetry.vuDb + 60) / 60) * 100));

  return (
    <div
      onClick={onSelect}
      role="button"
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") onSelect();
      }}
      className={`cursor-pointer rounded-2xl border p-3 transition ${
        selected
          ? accent === "sky"
            ? "border-sky-400 bg-sky-500/10"
            : "border-rose-400 bg-rose-500/10"
          : "border-white/8 bg-[#11131B] hover:border-white/20"
      }`}
    >
      <div className="mb-2 flex items-center justify-between">
        <span
          className={`text-xs font-bold ${
            accent === "sky" ? "text-sky-300" : "text-rose-300"
          }`}
        >
          {label}
        </span>
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onToggleMute();
          }}
          title={params.muted ? "Réactiver" : "Couper le micro"}
          className={`rounded-lg p-1 ${
            params.muted ? "bg-rose-500/20 text-rose-300" : "bg-white/8 text-slate-200"
          }`}
        >
          {params.muted ? <MicOff className="h-3.5 w-3.5" /> : <Mic className="h-3.5 w-3.5" />}
        </button>
      </div>

      {/* QR Code (safe data-URI img, zero dangerouslySetInnerHTML) */}
      <div className="mx-auto mb-2.5 flex aspect-square w-36 items-center justify-center overflow-hidden rounded-xl bg-white p-2">
        {qrSvg ? (
          <img
            src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(qrSvg)}`}
            alt={`QR Code d'appairage ${label}`}
            className="h-full w-full object-contain"
          />
        ) : (
          <span className="px-2 text-center text-[10px] font-medium text-slate-600">
            QR prêt au lancement Tauri ({url.slice(0, 26)}…)
          </span>
        )}
      </div>

      {/* Live VU Bar & Connection state */}
      <div className="space-y-1.5">
        <div className="h-2 overflow-hidden rounded-full bg-black/50">
          <div
            className={`h-full transition-all duration-75 ${
              accent === "sky" ? "bg-sky-400" : "bg-rose-400"
            }`}
            style={{ width: `${vuPct}%` }}
          />
        </div>
        <div className="flex items-center justify-between text-[10px] text-slate-400">
          <span>
            {telemetry.connected ? "iPhone connecté" : "En attente QR"}
          </span>
          <span className="font-mono tabular-nums">
            {telemetry.connected ? `${telemetry.jitterMs.toFixed(0)} ms` : "--"}
          </span>
        </div>
      </div>
    </div>
  );
}

function ParamSlider({
  label,
  value,
  min,
  max,
  unit,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  unit: string;
  onChange: (v: number) => void;
}) {
  return (
    <div className="space-y-1">
      <div className="flex justify-between text-[11px]">
        <span className="text-slate-300">{label}</span>
        <span className="font-mono text-slate-400 tabular-nums">
          {value} {unit}
        </span>
      </div>
      <input
        type="range"
        min={min}
        max={max}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        className="studio-slider w-full"
      />
    </div>
  );
}
