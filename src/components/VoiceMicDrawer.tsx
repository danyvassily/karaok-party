import { useState } from "react";
import { Headphones, Mic, MicOff, RefreshCw, Sliders, Speaker, X } from "lucide-react";
import type {
  ActiveMicInputs,
  AudioInputDeviceInfo,
  AudioOutputDeviceInfo,
  MicChannelId,
  MicTelemetry,
  ScaleType,
  VocalDspParams,
  VocalPreset,
} from "../types/karaok";

interface VoiceMicDrawerProps {
  open: boolean;
  outputDevices: AudioOutputDeviceInfo[];
  activeOutputDevice: string | null;
  clamshellClosed: boolean;
  inputDevices: AudioInputDeviceInfo[];
  activeMicInputs: ActiveMicInputs;
  micError: string | null;
  mic1Telemetry: MicTelemetry;
  mic2Telemetry: MicTelemetry;
  mic1Params: VocalDspParams;
  mic2Params: VocalDspParams;
  latencyCalibrationMs: number;
  bufferSize: string;
  onClose: () => void;
  onRefreshDevices: () => void;
  onTestOutputTone: () => void;
  onSelectOutputDevice: (deviceName: string | null) => void;
  onSelectInputDevice: (ch: MicChannelId, deviceName: string | null) => void;
  onSelectPreset: (ch: MicChannelId, preset: VocalPreset) => void;
  onUpdateParams: (ch: MicChannelId, params: VocalDspParams) => void;
  onSetLatencyMs: (ms: number) => void;
}

const PRESETS: { id: VocalPreset; label: string; desc: string }[] = [
  { id: "natural", label: "Naturel", desc: "Comp. légère · 0 % tune" },
  { id: "studio", label: "Studio", desc: "Chaleur · 85 ms · Plate" },
  { id: "live", label: "Scène / Live", desc: "Grande salle · 45 ms" },
  { id: "popStar", label: "Pop Star", desc: "Autotune 8 ms · Brillant" },
  { id: "expert", label: "Mode Expert", desc: "Tous les réglages DSP" },
];

const isBuiltInMacBookMic = (name?: string | null): boolean => {
  if (!name) return false;
  const l = name.toLowerCase();
  return (
    l.includes("built-in") ||
    l.includes("interne") ||
    (l.includes("macbook") && (l.includes("micro") || l.includes("mikro") || l.includes("mic")))
  );
};

export function VoiceMicDrawer({
  open,
  outputDevices,
  activeOutputDevice,
  clamshellClosed,
  inputDevices,
  activeMicInputs,
  micError,
  mic1Telemetry,
  mic2Telemetry,
  mic1Params,
  mic2Params,
  latencyCalibrationMs,
  onClose,
  onRefreshDevices,
  onTestOutputTone,
  onSelectOutputDevice,
  onSelectInputDevice,
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
    <div className="fixed inset-y-0 right-0 z-40 flex w-[470px] max-w-[95vw] flex-col border-l border-white/10 bg-[#0D0F16]/98 shadow-2xl backdrop-blur-xl">
      {/* Header */}
      <div className="flex items-center justify-between border-b border-white/8 px-5 py-4">
        <div>
          <h2 className="text-sm font-bold text-slate-50">
            Réglages Audio, Casque Micro & Enceintes
          </h2>
          <p className="text-xs text-slate-400">
            Routage CoreAudio indépendant : entrée casque micro → sortie enceintes
          </p>
        </div>
        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={onRefreshDevices}
            title="Actualiser les périphériques audio détectés sur le Mac"
            className="rounded-lg p-1.5 text-slate-400 hover:bg-white/10 hover:text-sky-300"
          >
            <RefreshCw className="h-4 w-4" />
          </button>
          <button
            type="button"
            onClick={onClose}
            aria-label="Fermer les réglages audio"
            className="rounded-lg p-1.5 text-slate-400 hover:bg-white/10 hover:text-slate-100"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
      </div>

      <div className="flex-1 space-y-5 overflow-y-auto p-5">
        {micError && (
          <p className="rounded-lg bg-rose-500/15 px-3 py-2 text-xs text-rose-300">
            {micError}
          </p>
        )}

        {/* 1. Audio Output Selector (Speakers, Bluetooth, HDMI TV, Headphones) */}
        <section className="space-y-2.5 rounded-2xl border border-emerald-400/25 bg-emerald-500/6 p-4">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2">
              <Speaker className="h-4 w-4 text-emerald-400" />
              <h3 className="text-xs font-bold text-slate-100">
                Sortie Audio (Enceintes / TV / Casque)
              </h3>
            </div>
            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={onTestOutputTone}
                className="rounded-lg border border-emerald-400/35 bg-emerald-400/15 px-2.5 py-1 text-[10px] font-bold text-emerald-200 transition hover:bg-emerald-400/25"
              >
                Tester enceintes
              </button>
              <span className="text-[10px] font-medium text-emerald-300">
                {outputDevices.length} détectées
              </span>
            </div>
          </div>

          <select
            value={activeOutputDevice ?? ""}
            onChange={(e) => onSelectOutputDevice(e.target.value || null)}
            aria-label="Sortie audio principale"
            className="w-full rounded-xl border border-white/12 bg-[#07080C] px-3 py-2.5 text-xs font-medium text-slate-100 focus:border-emerald-400 focus:outline-none"
          >
            {outputDevices.map((dev) => (
              <option key={dev.name} value={dev.name}>
                {dev.name} ({(dev.sampleRate / 1000).toFixed(1)} kHz · {dev.channels} ch)
                {dev.isDefault ? " — Par défaut" : ""}
              </option>
            ))}
          </select>

          <p className="text-[11px] leading-relaxed text-slate-400">
            Sélectionnez vos enceintes (ex. <strong>Haut-parleurs MacBook Pro</strong>, enceintes Bluetooth ou TV HDMI) pendant que votre <strong>casque micro</strong> sert uniquement d'entrée micro ci-dessous.
          </p>
        </section>

        {/* 2. Audio Input Selectors: Mic 1 (Headset / Mac) & Mic 2 */}
        <section className="space-y-2.5">
          <div className="flex items-center gap-2">
            <Headphones className="h-4 w-4 text-sky-400" />
            <h3 className="text-xs font-bold text-slate-200">
              Entrées Micro (Casque micro / Micro Mac)
            </h3>
          </div>

          {clamshellClosed &&
            (isBuiltInMacBookMic(activeMicInputs.mic1Device) ||
              isBuiltInMacBookMic(activeMicInputs.mic2Device)) && (
              <p className="rounded-xl border border-rose-400/35 bg-rose-500/10 px-3.5 py-2.5 text-[11px] leading-relaxed text-rose-200">
                <strong>Capot du MacBook fermé :</strong> macOS coupe matériellement le microphone interne lorsque l'ordinateur est fermé. Sélectionnez votre <strong>casque micro</strong>, le <strong>Micro de votre iPhone</strong> ou un micro USB.
              </p>
            )}

          {((mic1Telemetry.connected && !mic1Params.muted && mic1Telemetry.vuDb <= -90) ||
            (mic2Telemetry.connected && !mic2Params.muted && mic2Telemetry.vuDb <= -90)) && (
            <p className="rounded-xl border border-amber-400/30 bg-amber-400/10 px-3.5 py-2.5 text-[11px] leading-relaxed text-amber-200">
              <strong>Aucun signal micro détecté (-96 dB) :</strong> parlez dans votre casque micro (le Bluetooth met 1 à 2 s à s'activer) ou vérifiez l'autorisation dans{" "}
              <strong>Réglages Système → Confidentialité et sécurité → Micro</strong>.
            </p>
          )}

          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <MicInputCard
              label="Micro 1 (Casque / Principal)"
              accent="sky"
              selected={activeMic === "mic1"}
              selectedDevice={activeMicInputs.mic1Device}
              devices={inputDevices}
              telemetry={mic1Telemetry}
              params={mic1Params}
              onSelectCard={() => setActiveMic("mic1")}
              onChangeDevice={(name) => onSelectInputDevice("mic1", name)}
              onToggleMute={() =>
                onUpdateParams("mic1", { ...mic1Params, muted: !mic1Params.muted })
              }
            />
            <MicInputCard
              label="Micro 2 (Duo)"
              accent="coral"
              selected={activeMic === "mic2"}
              selectedDevice={activeMicInputs.mic2Device}
              devices={inputDevices}
              telemetry={mic2Telemetry}
              params={mic2Params}
              onSelectCard={() => setActiveMic("mic2")}
              onChangeDevice={(name) => onSelectInputDevice("mic2", name)}
              onToggleMute={() =>
                onUpdateParams("mic2", { ...mic2Params, muted: !mic2Params.muted })
              }
            />
          </div>
        </section>

        {/* 3. Vocal Presets for selected microphone */}
        <section className="space-y-2.5">
          <div className="flex items-center justify-between">
            <h3 className="text-xs font-bold text-slate-200">
              Préréglage vocal —{" "}
              <span className={activeMic === "mic1" ? "text-sky-400" : "text-rose-400"}>
                {activeMic === "mic1" ? "Micro 1" : "Micro 2"}
              </span>
            </h3>
            <span className="text-[11px] text-slate-400">
              Autotune + Réverbe en direct
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

        {/* 4. Autotune & Reverb + Expert DSP Controls */}
        <section className="space-y-3 rounded-xl border border-white/8 bg-[#11131B] p-4">
          <div className="flex items-center justify-between">
            <span className="flex items-center gap-1.5 text-xs font-bold text-slate-100">
              <Sliders className="h-3.5 w-3.5 text-sky-400" />
              {isExpert ? "Chaîne DSP complète (Mode Expert)" : "Autotune & Réverbération"}
            </span>
            <div className="flex items-center gap-3">
              <label className="flex items-center gap-1.5 text-[11px] font-semibold text-sky-300">
                <input
                  type="checkbox"
                  checked={activeParams.autotuneEnabled}
                  onChange={(e) =>
                    onUpdateParams(activeMic, {
                      ...activeParams,
                      autotuneEnabled: e.target.checked,
                      autotuneMix: e.target.checked ? Math.max(activeParams.autotuneMix, 1.0) : 0.0,
                    })
                  }
                  className="accent-sky-400"
                />
                Autotune
              </label>
              <label className="flex items-center gap-1.5 text-[11px] font-semibold text-emerald-300">
                <input
                  type="checkbox"
                  checked={activeParams.reverbWet > 0.01}
                  onChange={(e) =>
                    patchParam("reverbWet", e.target.checked ? 0.36 : 0.0)
                  }
                  className="accent-emerald-400"
                />
                Réverbe
              </label>
            </div>
          </div>

          <ParamSlider
            label="Volume Retour Voix (Micro)"
            value={Math.round(activeParams.gain * 100)}
            min={0}
            max={400}
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
                  <option value="chromatic">Chromatique (universel)</option>
                  <option value="major">Majeure</option>
                  <option value="minor">Mineure</option>
                </select>
              </div>
              <ParamSlider
                label="Passe-Haut (Anti-bruits graves)"
                value={Math.round(activeParams.hpfHz)}
                min={40}
                max={160}
                unit="Hz"
                onChange={(v) => patchParam("hpfHz", v)}
              />
              <ParamSlider
                label="Noise Gate (Anti-Larsen)"
                value={Math.round(activeParams.gateThresholdDb)}
                min={-70}
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

        {/* 5. Latency Calibration (0..250 ms) for Soundbars / Bluetooth */}
        <section className="space-y-2 rounded-xl border border-white/8 bg-[#11131B] p-4">
          <div className="flex items-center justify-between">
            <h3 className="text-xs font-bold text-slate-100">
              Calibration Latence (Enceinte Bluetooth / Barre de son)
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
            Compense le décalage audio entre vos enceintes et l'affichage des paroles.
          </p>
        </section>
      </div>
    </div>
  );
}

function MicInputCard({
  label,
  accent,
  selected,
  selectedDevice,
  devices,
  telemetry,
  params,
  onSelectCard,
  onChangeDevice,
  onToggleMute,
}: {
  label: string;
  accent: "sky" | "coral";
  selected: boolean;
  selectedDevice: string | null;
  devices: AudioInputDeviceInfo[];
  telemetry: MicTelemetry;
  params: VocalDspParams;
  onSelectCard: () => void;
  onChangeDevice: (deviceName: string | null) => void;
  onToggleMute: () => void;
}) {
  const vuPct = Math.max(0, Math.min(100, ((telemetry.vuDb + 65) / 65) * 100));

  return (
    <div
      onClick={onSelectCard}
      role="button"
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") onSelectCard();
      }}
      className={`cursor-pointer rounded-2xl border p-3.5 transition ${
        selected
          ? accent === "sky"
            ? "border-sky-400 bg-sky-500/10"
            : "border-rose-400 bg-rose-500/10"
          : "border-white/8 bg-[#11131B] hover:border-white/20"
      }`}
    >
      <div className="mb-2.5 flex items-center justify-between">
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

      {/* Native CoreAudio Input Device Selector */}
      <select
        value={selectedDevice ?? ""}
        onClick={(e) => e.stopPropagation()}
        onChange={(e) => onChangeDevice(e.target.value || null)}
        aria-label={`Source audio ${label}`}
        className="mb-3 w-full rounded-lg border border-white/12 bg-[#07080C] px-2.5 py-2 text-xs text-slate-100 focus:border-sky-400 focus:outline-none"
      >
        <option value="">Désactivé</option>
        {devices.map((dev) => (
          <option key={dev.name} value={dev.name}>
            {dev.name} ({(dev.sampleRate / 1000).toFixed(1)} kHz)
          </option>
        ))}
      </select>

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
          <span className={telemetry.connected ? "font-semibold text-emerald-400" : ""}>
            {telemetry.connected ? "Micro actif" : "Désactivé"}
          </span>
          <span className="font-mono tabular-nums">
            {telemetry.connected ? `${telemetry.vuDb.toFixed(0)} dB` : "--"}
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
