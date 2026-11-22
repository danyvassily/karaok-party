import { useEffect, useMemo, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LyricsStage } from "./components/LyricsStage";
import { DEFAULT_TELEMETRY, isTauriRuntime, karaokApi } from "./lib/tauri";
import type { LyricsDocument, PlaybackTelemetry, QueueItem, Track } from "./types/karaok";

/**
 * Dedicated Second Screen / TV Fullscreen Window (HDMI / AirPlay).
 * Displays zero technical toolbars: only the 5-line high-contrast karaoke lyrics,
 * active song title, 3-dot countdown, and upcoming singer banner.
 */
export default function TvApp() {
  const [telemetry, setTelemetry] = useState<PlaybackTelemetry>(DEFAULT_TELEMETRY);
  const [activeTrack, setActiveTrack] = useState<Track | null>(null);
  const [lyrics, setLyrics] = useState<LyricsDocument | null>(null);
  const [queue, setQueue] = useState<QueueItem[]>([]);

  useEffect(() => {
    const loadState = async () => {
      const [t, q] = await Promise.all([karaokApi.getTelemetry(), karaokApi.listQueue()]);
      setTelemetry(t);
      setQueue(q);
      if (t.trackId) {
        const tracks = await karaokApi.listTracks();
        const found = tracks.find((tr) => tr.id === t.trackId) ?? null;
        setActiveTrack(found);
        const doc = await karaokApi.getLyrics(t.trackId);
        setLyrics(doc);
      }
    };
    void loadState();

    const unlisteners: (() => void)[] = [];
    void karaokApi.onTelemetry(setTelemetry).then((u) => unlisteners.push(u));
    void karaokApi
      .onActiveTrackChanged((tr) => {
        setActiveTrack(tr);
        void karaokApi.getLyrics(tr.id).then(setLyrics);
      })
      .then((u) => unlisteners.push(u));
    void karaokApi
      .onLyricsUpdated((tid) => {
        setActiveTrack((cur) => {
          if (cur && cur.id === tid) {
            void karaokApi.getLyrics(tid).then(setLyrics);
          }
          return cur;
        });
      })
      .then((u) => unlisteners.push(u));
    void karaokApi
      .onQueueUpdated(() => void karaokApi.listQueue().then(setQueue))
      .then((u) => unlisteners.push(u));

    return () => {
      unlisteners.forEach((u) => u());
    };
  }, []);

  const nextSinger = useMemo(() => {
    const upcoming = queue.find((q) => q.status === "queued");
    if (!upcoming) return null;
    const names =
      upcoming.mode === "duo" && upcoming.singer2
        ? `${upcoming.singer1} & ${upcoming.singer2}`
        : upcoming.singer1;
    return `${names} — ${upcoming.trackTitle}`;
  }, [queue]);

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-[#05060A] bg-[radial-gradient(ellipse_at_center,_rgba(56,189,248,0.10),_transparent_70%)] text-slate-50">
      {/* Minimal TV Header: Current Song & Upcoming Singer (draggable to external TV/AirPlay) */}
      <header
        data-tauri-drag-region
        onMouseDown={(e) => {
          if (isTauriRuntime() && e.buttons === 1) {
            void getCurrentWindow().startDragging();
          }
        }}
        className="flex cursor-grab items-center justify-between px-10 py-6 active:cursor-grabbing"
      >
        <div>
          <p className="text-lg font-bold text-slate-100">
            {activeTrack ? activeTrack.title : "Karaok Party"}
          </p>
          {activeTrack && (
            <p className="text-sm text-slate-400">{activeTrack.artist}</p>
          )}
        </div>
        {nextSinger && (
          <div className="rounded-full border border-white/10 bg-white/5 px-5 py-2 text-sm text-slate-200">
            <span className="mr-2 font-semibold text-sky-400">À suivre :</span>
            {nextSinger}
          </div>
        )}
      </header>

      <LyricsStage
        lyrics={lyrics}
        positionMs={telemetry.positionMs}
        mic1={telemetry.mic1}
        mic2={telemetry.mic2}
        showPitchGauge={false}
        tvMode
        trackTitle={activeTrack?.title}
        trackArtist={activeTrack?.artist}
      />
    </div>
  );
}
