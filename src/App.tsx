import { useCallback, useEffect, useMemo, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { Activity, AlignLeft, PanelLeft, PanelLeftClose, Settings2, Upload } from "lucide-react";
import { LeftSidebar } from "./components/LeftSidebar";
import { LyricsStage } from "./components/LyricsStage";
import { RecordingReviewModal } from "./components/RecordingReviewModal";
import { StudioDock } from "./components/StudioDock";
import { SyncLyricsEditor } from "./components/SyncLyricsEditor";
import { VoiceMicDrawer } from "./components/VoiceMicDrawer";
import {
  DEFAULT_TELEMETRY,
  defaultVocalParams,
  isTauriRuntime,
  karaokApi,
} from "./lib/tauri";
import type {
  ActiveMicInputs,
  AudioInputDeviceInfo,
  AudioOutputDeviceInfo,
  DemucsModel,
  LoopRegion,
  LyricsDocument,
  MicChannelId,
  PlaybackTelemetry,
  PlaylistTrackCandidate,
  QueueItem,
  QueueMode,
  RecordingEntry,
  StemMix,
  Track,
  VocalDspParams,
  VocalPreset,
} from "./types/karaok";

export default function App() {
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [syncMode, setSyncMode] = useState(false);
  const [micDrawerOpen, setMicDrawerOpen] = useState(false);
  const [showPitchGauge, setShowPitchGauge] = useState(true);
  const [tvOpen, setTvOpen] = useState(false);

  const [tracks, setTracks] = useState<Track[]>([]);
  const [queue, setQueue] = useState<QueueItem[]>([]);
  const [activeTrack, setActiveTrack] = useState<Track | null>(null);
  const [lyrics, setLyrics] = useState<LyricsDocument | null>(null);
  const [telemetry, setTelemetry] = useState<PlaybackTelemetry>(DEFAULT_TELEMETRY);
  const [outputDevices, setOutputDevices] = useState<AudioOutputDeviceInfo[]>([]);
  const [activeOutputDevice, setActiveOutputDevice] = useState<string | null>(null);
  const [clamshellClosed, setClamshellClosed] = useState(false);
  const [inputDevices, setInputDevices] = useState<AudioInputDeviceInfo[]>([]);
  const [activeMicInputs, setActiveMicInputs] = useState<ActiveMicInputs>({
    mic1Device: null,
    mic2Device: null,
  });
  const [micError, setMicError] = useState<string | null>(null);
  const [mic1Params, setMic1Params] = useState<VocalDspParams>(() => defaultVocalParams("studio"));
  const [mic2Params, setMic2Params] = useState<VocalDspParams>(() => defaultVocalParams("studio"));

  const [isImporting, setIsImporting] = useState(false);
  const [isDragHover, setIsDragHover] = useState(false);
  const [importError, setImportError] = useState<string | null>(null);
  const [spotifyCandidates, setSpotifyCandidates] = useState<PlaylistTrackCandidate[]>([]);
  const [reviewRecording, setReviewRecording] = useState<RecordingEntry | null>(null);

  const refreshLibrary = useCallback(async () => {
    const list = await karaokApi.listTracks();
    setTracks(list);
  }, []);

  const refreshQueue = useCallback(async () => {
    const items = await karaokApi.listQueue();
    setQueue(items);
  }, []);

  const loadLyricsForTrack = useCallback(async (trackId: string) => {
    const doc = await karaokApi.getLyrics(trackId);
    setLyrics(doc);
  }, []);

  const refreshAudioDevices = useCallback(async () => {
    const [inputs, outputs, routing] = await Promise.all([
      karaokApi.listAudioInputDevices(),
      karaokApi.listAudioOutputDevices(),
      karaokApi.getAudioRoutingState(),
    ]);
    setInputDevices(inputs);
    setOutputDevices(outputs);
    setActiveOutputDevice(routing.outputDevice);
    setClamshellClosed(Boolean(routing.clamshellClosed));
    setActiveMicInputs({
      mic1Device: routing.mic1Device,
      mic2Device: routing.mic2Device,
    });
  }, []);

  // Bootstrap state & subscribe to 60Hz Tauri telemetry + backend events
  useEffect(() => {
    void refreshLibrary();
    void refreshQueue();
    void refreshAudioDevices();
    void karaokApi.getMicParams("mic1").then(setMic1Params);
    void karaokApi.getMicParams("mic2").then(setMic2Params);

    const unlisteners: (() => void)[] = [];
    void karaokApi.onTelemetry((t) => setTelemetry(t)).then((u) => unlisteners.push(u));
    void karaokApi
      .onStemProgress((ev) => {
        setTracks((prev) =>
          prev.map((tr) =>
            tr.id === ev.trackId
              ? {
                  ...tr,
                  stemStatus: ev.status,
                  stemProgress: ev.progressPct,
                  stemsDir: ev.stemsDir,
                }
              : tr
          )
        );
        setActiveTrack((prev) =>
          prev && prev.id === ev.trackId
            ? {
                ...prev,
                stemStatus: ev.status,
                stemProgress: ev.progressPct,
                stemsDir: ev.stemsDir,
              }
            : prev
        );
      })
      .then((u) => unlisteners.push(u));
    void karaokApi
      .onActiveTrackChanged((tr) => {
        setActiveTrack(tr);
        void loadLyricsForTrack(tr.id);
      })
      .then((u) => unlisteners.push(u));
    void karaokApi
      .onLyricsUpdated((tid) => {
        setActiveTrack((cur) => {
          if (cur && cur.id === tid) {
            void loadLyricsForTrack(tid);
          }
          return cur;
        });
        void refreshLibrary();
      })
      .then((u) => unlisteners.push(u));
    void karaokApi.onQueueUpdated(() => void refreshQueue()).then((u) => unlisteners.push(u));
    void karaokApi
      .onAudioRoutingUpdated(() => void refreshAudioDevices())
      .then((u) => unlisteners.push(u));

    return () => {
      unlisteners.forEach((fn) => fn());
    };
  }, [refreshLibrary, refreshQueue, refreshAudioDevices, loadLyricsForTrack]);

  // Native macOS Drag & Drop audio file import via Tauri v2 webview
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let unlisten: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent(async (event) => {
        if (event.payload.type === "over") {
          setIsDragHover(true);
        } else if (event.payload.type === "leave") {
          setIsDragHover(false);
        } else if (event.payload.type === "drop") {
          setIsDragHover(false);
          const audioExts = [".mp3", ".flac", ".wav", ".m4a", ".aac", ".ogg"];
          const files = event.payload.paths.filter((p) =>
            audioExts.some((ext) => p.toLowerCase().endsWith(ext))
          );
          if (files.length === 0) return;
          setIsImporting(true);
          setImportError(null);
          try {
            for (const p of files) {
              const track = await karaokApi.importLocalFile(p);
              await refreshLibrary();
              void karaokApi.separateTrackStems(track.id, "htdemucs");
            }
          } catch (err) {
            setImportError(String(err));
          } finally {
            setIsImporting(false);
          }
        }
      })
      .then((u) => {
        unlisten = u;
      });
    return () => {
      unlisten?.();
    };
  }, [refreshLibrary]);

  // Global keyboard shortcuts (when not inside Sync Tap-to-Sync or inputs)
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (
        syncMode ||
        e.target instanceof HTMLInputElement ||
        e.target instanceof HTMLTextAreaElement ||
        e.target instanceof HTMLSelectElement
      ) {
        return;
      }
      if (e.code === "Space") {
        e.preventDefault();
        void karaokApi.setPlaying(!telemetry.isPlaying);
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        void karaokApi.rewind5s();
      } else if (e.key === "[" && activeTrack) {
        e.preventDefault();
        void karaokApi.shiftLyricsOffset(activeTrack.id, -100).then(setLyrics);
      } else if (e.key === "]" && activeTrack) {
        e.preventDefault();
        void karaokApi.shiftLyricsOffset(activeTrack.id, 100).then(setLyrics);
      } else if (e.key === "Escape") {
        setMicDrawerOpen(false);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [syncMode, telemetry.isPlaying, activeTrack]);

  const currentSingerBanner = useMemo(() => {
    const playing = queue.find((q) => q.status === "playing");
    if (!playing) return null;
    return playing.mode === "duo" && playing.singer2
      ? `Duo : ${playing.singer1} & ${playing.singer2}`
      : `Chanteur : ${playing.singer1}`;
  }, [queue]);

  // Stabilized callbacks so React.memo(LeftSidebar) skips 60 Hz telemetry re-renders
  const handlePlayTrack = useCallback(async (trackId: string) => {
    try {
      setSyncMode(false);
      const tr = await karaokApi.loadAndPlayTrack(trackId, true);
      setActiveTrack(tr);
      await loadLyricsForTrack(tr.id);
    } catch (err) {
      setImportError(String(err));
    }
  }, [loadLyricsForTrack]);

  const handleImportLocalDialog = useCallback(async () => {
    if (!isTauriRuntime()) {
      setImportError("Importation de fichiers locaux disponible dans l'application macOS Tauri.");
      return;
    }
    setImportError(null);
    const selected = await openDialog({
      multiple: true,
      filters: [
        { name: "Audio Studio", extensions: ["mp3", "flac", "wav", "m4a", "aac", "ogg"] },
      ],
    });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    setIsImporting(true);
    try {
      for (const p of paths) {
        const track = await karaokApi.importLocalFile(p);
        await refreshLibrary();
        void karaokApi.separateTrackStems(track.id, "htdemucs");
      }
    } catch (err) {
      setImportError(String(err));
    } finally {
      setIsImporting(false);
    }
  }, [refreshLibrary]);

  const handleImportYoutube = useCallback(async (url: string) => {
    setIsImporting(true);
    setImportError(null);
    try {
      const track = await karaokApi.importYoutubeUrl(url);
      await refreshLibrary();
      void karaokApi.separateTrackStems(track.id, "htdemucs");
    } catch (err) {
      setImportError(String(err));
    } finally {
      setIsImporting(false);
    }
  }, [refreshLibrary]);

  const handleImportSpotify = useCallback(async (url: string) => {
    setIsImporting(true);
    setImportError(null);
    try {
      const list = await karaokApi.importSpotifyPlaylist(url);
      setSpotifyCandidates(list);
      if (list.length === 0) {
        setImportError("Aucun titre public trouvé sur cette playlist Spotify.");
      }
    } catch (err) {
      setImportError(String(err));
    } finally {
      setIsImporting(false);
    }
  }, []);

  const handleSeparateStems = useCallback(async (trackId: string, model: DemucsModel) => {
    try {
      await karaokApi.separateTrackStems(trackId, model);
    } catch (err) {
      setImportError(String(err));
    }
  }, []);

  const handleToggleFavorite = useCallback(async (trackId: string) => {
    await karaokApi.toggleFavorite(trackId);
    await refreshLibrary();
  }, [refreshLibrary]);

  const handleDeleteTrack = useCallback(async (trackId: string) => {
    await karaokApi.deleteTrack(trackId);
    await refreshLibrary();
  }, [refreshLibrary]);

  const handleEnqueueTrack = useCallback(async (
    trackId: string,
    mode: QueueMode,
    singer1: string,
    singer2?: string
  ) => {
    await karaokApi.enqueueTrack(trackId, mode, singer1, singer2);
    await refreshQueue();
  }, [refreshQueue]);

  const handleAdvanceQueue = useCallback(async () => {
    const next = await karaokApi.advanceQueue();
    await refreshQueue();
    if (next) {
      await handlePlayTrack(next.trackId);
    }
  }, [refreshQueue, handlePlayTrack]);

  const handleRemoveQueueItem = useCallback(async (itemId: string) => {
    await karaokApi.removeQueueItem(itemId);
    await refreshQueue();
  }, [refreshQueue]);

  const handleToggleRecording = async () => {
    if (!telemetry.isRecording) {
      const title = activeTrack ? `${activeTrack.title} (Prise)` : "Session Karaok Party";
      await karaokApi.startRecording(title);
    } else {
      const entry = await karaokApi.stopRecording();
      setReviewRecording(entry);
    }
  };

  const handleSelectOutputDevice = async (deviceName: string | null) => {
    setMicError(null);
    try {
      const routing = await karaokApi.selectAudioOutputDevice(deviceName);
      setActiveOutputDevice(routing.outputDevice);
      await refreshAudioDevices();
    } catch (err) {
      setMicError(String(err));
    }
  };

  const handleSelectMicInput = async (ch: MicChannelId, deviceName: string | null) => {
    setMicError(null);
    try {
      const updated = await karaokApi.selectMicInputDevice(ch, deviceName);
      setActiveMicInputs(updated);
      await refreshAudioDevices();
    } catch (err) {
      setMicError(String(err));
    }
  };

  const handleSavePlainLyrics = async (plainText: string) => {
    if (!activeTrack) return;
    const saved = await karaokApi.savePlainLyricsForTrack(activeTrack.id, plainText);
    setLyrics(saved);
    await refreshLibrary();
    setSyncMode(false);
  };

  const handleSelectMicPreset = async (ch: MicChannelId, preset: VocalPreset) => {
    const updated = await karaokApi.setMicPreset(ch, preset);
    if (ch === "mic1") setMic1Params(updated);
    else setMic2Params(updated);
  };

  const handleUpdateMicParams = async (ch: MicChannelId, params: VocalDspParams) => {
    if (ch === "mic1") setMic1Params(params);
    else setMic2Params(params);
    await karaokApi.setMicParams(ch, params);
  };

  const handleExportRecordingMix = async (
    recordingId: string,
    vocalBalance: number,
    format: "wav" | "mp3"
  ): Promise<string> => {
    const dest = isTauriRuntime()
      ? await saveDialog({
          defaultPath: `karaok-party-mix.${format}`,
          filters: [{ name: format.toUpperCase(), extensions: [format] }],
        })
      : `/tmp/karaok-party-mix.${format}`;
    if (!dest) throw new Error("Export annulé");
    return karaokApi.exportRecordingMix(recordingId, vocalBalance, dest);
  };

  const handleHeaderMouseDown = (e: React.MouseEvent<HTMLElement>) => {
    if (!isTauriRuntime() || e.buttons !== 1) return;
    const target = e.target as HTMLElement;
    if (target.closest("button, input, a, select, textarea")) return;
    void getCurrentWindow().startDragging();
  };

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-[#07080C] text-slate-50">
      {/* Top Studio Bar — native macOS draggable via data-tauri-drag-region + startDragging() */}
      <header
        data-tauri-drag-region
        onMouseDown={handleHeaderMouseDown}
        className="titlebar-drag flex h-12 shrink-0 cursor-grab items-center justify-between border-b border-white/8 bg-[#0B0D14] pl-20 pr-4 active:cursor-grabbing"
      >
        <div data-tauri-drag-region className="flex items-center gap-3">
          <button
            type="button"
            onClick={() => setSidebarOpen((v) => !v)}
            title={sidebarOpen ? "Masquer la bibliothèque" : "Afficher la bibliothèque"}
            className="titlebar-no-drag rounded-lg p-1.5 text-slate-400 transition hover:bg-white/10 hover:text-slate-100"
          >
            {sidebarOpen ? (
              <PanelLeftClose className="h-4 w-4" />
            ) : (
              <PanelLeft className="h-4 w-4" />
            )}
          </button>
          <span
            data-tauri-drag-region
            className="select-none text-sm font-bold tracking-tight text-slate-100"
          >
            Karaok Party
          </span>
          {activeTrack && (
            <div
              data-tauri-drag-region
              className="hidden select-none items-center gap-2 border-l border-white/10 pl-3 md:flex"
            >
              <span
                data-tauri-drag-region
                className="max-w-xs truncate text-xs font-semibold text-slate-200"
              >
                {activeTrack.title}
              </span>
              <span data-tauri-drag-region className="text-xs text-slate-400">
                — {activeTrack.artist}
              </span>
              {currentSingerBanner && (
                <span className="rounded-full bg-sky-400/15 px-2.5 py-0.5 text-[11px] font-semibold text-sky-300">
                  {currentSingerBanner}
                </span>
              )}
            </div>
          )}
        </div>

        <div className="titlebar-no-drag flex items-center gap-2">
          <button
            type="button"
            onClick={() => setShowPitchGauge((v) => !v)}
            className={`flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs font-medium transition ${
              showPitchGauge
                ? "bg-white/10 text-emerald-300"
                : "text-slate-400 hover:bg-white/5 hover:text-slate-200"
            }`}
          >
            <Activity className="h-3.5 w-3.5" />
            Jauge justesse
          </button>

          <button
            type="button"
            onClick={() => setSyncMode((v) => !v)}
            className={`flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs font-semibold transition ${
              syncMode
                ? "bg-sky-400 text-slate-950"
                : "border border-white/10 bg-white/5 text-slate-200 hover:bg-white/10"
            }`}
          >
            <AlignLeft className="h-3.5 w-3.5" />
            Sync Paroles
          </button>

          <button
            type="button"
            onClick={() => {
              setMicDrawerOpen((v) => {
                if (!v) void refreshAudioDevices();
                return !v;
              });
            }}
            className={`flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs font-semibold transition ${
              micDrawerOpen
                ? "bg-emerald-400 text-slate-950"
                : "border border-white/10 bg-white/5 text-slate-200 hover:bg-white/10"
            }`}
          >
            <Settings2 className="h-3.5 w-3.5" />
            Réglages Audio
          </button>
        </div>
      </header>

      {/* Main Content Area: Collapsible Sidebar + Center Karaoke Stage / Sync Editor */}
      <div className="flex flex-1 overflow-hidden">
        {sidebarOpen && (
          <LeftSidebar
            tracks={tracks}
            queue={queue}
            activeTrackId={activeTrack?.id ?? null}
            isImporting={isImporting}
            importError={importError}
            spotifyCandidates={spotifyCandidates}
            onPlayTrack={handlePlayTrack}
            onImportLocalDialog={handleImportLocalDialog}
            onImportYoutube={handleImportYoutube}
            onImportSpotify={handleImportSpotify}
            onSeparateStems={handleSeparateStems}
            onToggleFavorite={handleToggleFavorite}
            onDeleteTrack={handleDeleteTrack}
            onEnqueueTrack={handleEnqueueTrack}
            onAdvanceQueue={handleAdvanceQueue}
            onRemoveQueueItem={handleRemoveQueueItem}
          />
        )}

        <main className="relative flex flex-1 flex-col overflow-hidden bg-[radial-gradient(ellipse_at_top,_rgba(56,189,248,0.08),_transparent_62%)]">
          {syncMode ? (
            <SyncLyricsEditor
              track={activeTrack}
              lyrics={lyrics}
              positionMs={telemetry.positionMs}
              onClose={() => setSyncMode(false)}
              onFetchLrclib={async () => {
                if (!activeTrack) return;
                const doc = await karaokApi.fetchLrclibLyrics(
                  activeTrack.id,
                  activeTrack.title,
                  activeTrack.artist,
                  Math.round(activeTrack.durationMs / 1000)
                );
                setLyrics(doc);
              }}
              onSearchGoogle={async () => {
                if (!activeTrack) return;
                await karaokApi.openGoogleLyricsSearch(activeTrack.title, activeTrack.artist);
              }}
              onAutoAlignWhisper={async (plain) => {
                if (!activeTrack) return;
                const doc = await karaokApi.autoAlignLyricsWhisper(activeTrack.id, plain);
                setLyrics(doc);
                await refreshLibrary();
              }}
              onSavePlainLyrics={handleSavePlainLyrics}
              onShiftOffset={async (delta) => {
                if (!activeTrack) return;
                const doc = await karaokApi.shiftLyricsOffset(activeTrack.id, delta);
                setLyrics(doc);
              }}
              onSaveLyrics={async (doc) => {
                if (!activeTrack) return;
                const saved = await karaokApi.saveLyrics(activeTrack.id, doc);
                setLyrics(saved);
                await refreshLibrary();
              }}
            />
          ) : (
            <LyricsStage
              lyrics={lyrics}
              positionMs={telemetry.positionMs}
              mic1={telemetry.mic1}
              mic2={telemetry.mic2}
              showPitchGauge={showPitchGauge}
              trackId={activeTrack?.id}
              trackTitle={activeTrack?.title}
              trackArtist={activeTrack?.artist}
              onOpenSync={() => setSyncMode(true)}
              onFetchWebLyrics={async () => {
                if (!activeTrack) return;
                const doc = await karaokApi.fetchLrclibLyrics(
                  activeTrack.id,
                  activeTrack.title,
                  activeTrack.artist,
                  Math.round(activeTrack.durationMs / 1000)
                );
                setLyrics(doc);
                await refreshLibrary();
              }}
              onSearchGoogle={async () => {
                if (!activeTrack) return;
                await karaokApi.openGoogleLyricsSearch(activeTrack.title, activeTrack.artist);
              }}
              onSavePastedLyrics={handleSavePlainLyrics}
              onRealignVocalOnset={async () => {
                if (!activeTrack) return;
                const doc = await karaokApi.realignLyricsVocalOnset(activeTrack.id);
                setLyrics(doc);
                await refreshLibrary();
              }}
              onSetStartHere={async (posMs) => {
                if (!activeTrack) return;
                const doc = await karaokApi.setLyricsStartHere(activeTrack.id, posMs);
                setLyrics(doc);
              }}
              onSeekMs={(ms) => void karaokApi.seekMs(ms)}
            />
          )}
        </main>
      </div>

      {/* Bottom Studio Control Dock */}
      <StudioDock
        telemetry={telemetry}
        activeTrack={activeTrack}
        tvOpen={tvOpen}
        micDrawerOpen={micDrawerOpen}
        onPlayPause={() => void karaokApi.setPlaying(!telemetry.isPlaying)}
        onSeekMs={(ms) => void karaokApi.seekMs(ms)}
        onRewind5s={() => void karaokApi.rewind5s()}
        onSetTempoPct={(pct) => void karaokApi.setTempoPct(pct)}
        onSetStemMix={(mix: StemMix) => void karaokApi.setStemMix(mix)}
        onSeparateStems={(trackId, model) => void handleSeparateStems(trackId, model)}
        onSetAbLoop={(region: LoopRegion) => void karaokApi.setAbLoop(region)}
        onToggleRecording={() => void handleToggleRecording()}
        onToggleMicDrawer={() => {
          setMicDrawerOpen((v) => {
            if (!v) void refreshAudioDevices();
            return !v;
          });
        }}
        onToggleTvWindow={async () => {
          const opened = await karaokApi.toggleTvWindow();
          setTvOpen(opened);
        }}
      />

      {/* Audio Settings, Headset Mic & Speaker Output Routing Drawer */}
      <VoiceMicDrawer
        open={micDrawerOpen}
        outputDevices={outputDevices}
        activeOutputDevice={activeOutputDevice}
        clamshellClosed={clamshellClosed}
        inputDevices={inputDevices}
        activeMicInputs={activeMicInputs}
        micError={micError}
        mic1Telemetry={telemetry.mic1}
        mic2Telemetry={telemetry.mic2}
        mic1Params={mic1Params}
        mic2Params={mic2Params}
        latencyCalibrationMs={telemetry.latencyCalibrationMs}
        bufferSize={telemetry.bufferSize}
        onClose={() => setMicDrawerOpen(false)}
        onRefreshDevices={() => void refreshAudioDevices()}
        onTestOutputTone={() => void karaokApi.playOutputTestTone()}
        onSelectOutputDevice={(name) => void handleSelectOutputDevice(name)}
        onSelectInputDevice={(ch, name) => void handleSelectMicInput(ch, name)}
        onSelectPreset={handleSelectMicPreset}
        onUpdateParams={handleUpdateMicParams}
        onSetLatencyMs={(ms) => void karaokApi.setLatencyCalibrationMs(ms)}
      />

      {/* SingSnap-style Post-Recording Review & Export Modal */}
      <RecordingReviewModal
        recording={reviewRecording}
        onClose={() => setReviewRecording(null)}
        onPreviewMix={(id, bal) => karaokApi.previewRecordingMix(id, bal)}
        onExportMix={handleExportRecordingMix}
      />

      {/* Drag & Drop Audio File Overlay */}
      {isDragHover && (
        <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center bg-sky-950/70 backdrop-blur-sm">
          <div className="flex flex-col items-center gap-3 rounded-3xl border-2 border-dashed border-sky-400 bg-[#0D0F16]/90 px-12 py-10 text-center">
            <Upload className="h-10 w-10 text-sky-400" />
            <p className="text-lg font-bold text-slate-50">
              Déposez vos fichiers audio (MP3, FLAC, WAV, M4A, OGG)
            </p>
            <p className="text-xs text-slate-300">
              Importation locale immédiate + séparation IA Demucs automatique
            </p>
          </div>
        </div>
      )}
    </div>
  );
}
