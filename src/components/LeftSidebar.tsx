import { memo, useMemo, useState } from "react";
import {
  Download,
  FolderOpen,
  Heart,
  ListMusic,
  Music2,
  Play,
  Plus,
  Search,
  Sparkles,
  Trash2,
  Users,
  Youtube,
} from "lucide-react";
import type {
  DemucsModel,
  PlaylistTrackCandidate,
  QueueItem,
  QueueMode,
  Track,
} from "../types/karaok";
import { formatTimeMs } from "../lib/lyricsEngine";

interface LeftSidebarProps {
  tracks: Track[];
  queue: QueueItem[];
  activeTrackId: string | null;
  isImporting: boolean;
  importError: string | null;
  spotifyCandidates: PlaylistTrackCandidate[];
  onPlayTrack: (trackId: string) => void;
  onImportLocalDialog: () => void;
  onImportYoutube: (url: string) => void;
  onImportSpotify: (url: string) => void;
  onSeparateStems: (trackId: string, model: DemucsModel) => void;
  onToggleFavorite: (trackId: string) => void;
  onDeleteTrack: (trackId: string) => void;
  onEnqueueTrack: (trackId: string, mode: QueueMode, singer1: string, singer2?: string) => void;
  onAdvanceQueue: () => void;
  onRemoveQueueItem: (itemId: string) => void;
}

type LibraryFilter = "all" | "favorites" | "stems" | "duo";

export const LeftSidebar = memo(function LeftSidebar({
  tracks,
  queue,
  activeTrackId,
  isImporting,
  importError,
  spotifyCandidates,
  onPlayTrack,
  onImportLocalDialog,
  onImportYoutube,
  onImportSpotify,
  onSeparateStems,
  onToggleFavorite,
  onDeleteTrack,
  onEnqueueTrack,
  onAdvanceQueue,
  onRemoveQueueItem,
}: LeftSidebarProps) {
  const [tab, setTab] = useState<"library" | "queue">("library");
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<LibraryFilter>("all");
  const [urlInput, setUrlInput] = useState("");
  const [queueMode, setQueueMode] = useState<QueueMode>("solo");
  const [singer1, setSinger1] = useState("Chanteur 1");
  const [singer2, setSinger2] = useState("Chanteur 2");

  const duoTrackIds = useMemo(
    () => new Set(queue.filter((q) => q.mode === "duo").map((q) => q.trackId)),
    [queue]
  );

  const filteredTracks = useMemo(() => {
    const q = search.trim().toLowerCase();
    return tracks.filter((t) => {
      if (filter === "favorites" && !t.isFavorite) return false;
      if (
        filter === "stems" &&
        t.stemStatus !== "completed" &&
        t.stemStatus !== "streaming_ready"
      ) {
        return false;
      }
      if (filter === "duo" && !duoTrackIds.has(t.id)) return false;
      if (!q) return true;
      return (
        t.title.toLowerCase().includes(q) ||
        t.artist.toLowerCase().includes(q)
      );
    });
  }, [tracks, search, filter, duoTrackIds]);

  const handleUrlSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = urlInput.trim();
    if (!trimmed) return;
    if (trimmed.includes("open.spotify.com")) {
      onImportSpotify(trimmed);
    } else {
      onImportYoutube(trimmed);
    }
    setUrlInput("");
  };

  return (
    <aside className="flex h-full w-80 shrink-0 flex-col border-r border-white/8 bg-[#0D0F16]">
      {/* Tab switcher: Bibliothèque / File d'attente */}
      <div className="grid grid-cols-2 gap-1 border-b border-white/8 p-2">
        <button
          type="button"
          onClick={() => setTab("library")}
          className={`flex items-center justify-center gap-2 rounded-lg py-2 text-xs font-semibold transition ${
            tab === "library"
              ? "bg-white/10 text-slate-50"
              : "text-slate-400 hover:text-slate-200"
          }`}
        >
          <Music2 className="h-3.5 w-3.5" />
          Bibliothèque ({tracks.length})
        </button>
        <button
          type="button"
          onClick={() => setTab("queue")}
          className={`flex items-center justify-center gap-2 rounded-lg py-2 text-xs font-semibold transition ${
            tab === "queue"
              ? "bg-white/10 text-slate-50"
              : "text-slate-400 hover:text-slate-200"
          }`}
        >
          <ListMusic className="h-3.5 w-3.5" />
          File ({queue.length})
        </button>
      </div>

      {tab === "library" ? (
        <div className="flex flex-1 flex-col overflow-hidden">
          {/* Import controls: Local File + YouTube / Spotify URL */}
          <div className="space-y-2 border-b border-white/8 p-3">
            <div className="flex gap-2">
              <button
                type="button"
                onClick={onImportLocalDialog}
                disabled={isImporting}
                className="flex flex-1 items-center justify-center gap-1.5 rounded-lg border border-white/10 bg-white/5 px-3 py-2 text-xs font-semibold text-slate-100 transition hover:bg-white/10 disabled:opacity-50"
              >
                <FolderOpen className="h-3.5 w-3.5 text-sky-400" />
                Fichier audio local
              </button>
            </div>

            <form onSubmit={handleUrlSubmit} className="flex gap-1.5">
              <div className="relative flex-1">
                <Youtube className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-rose-400" />
                <input
                  type="url"
                  value={urlInput}
                  onChange={(e) => setUrlInput(e.target.value)}
                  placeholder="URL YouTube ou Playlist Spotify..."
                  className="w-full rounded-lg border border-white/10 bg-[#07080C] py-1.5 pl-8 pr-2 text-xs text-slate-100 placeholder:text-slate-500 focus:border-sky-400/60 focus:outline-none"
                />
              </div>
              <button
                type="submit"
                disabled={isImporting || !urlInput.trim()}
                title="Importer depuis YouTube ou playlist Spotify"
                className="rounded-lg bg-sky-400 px-2.5 py-1.5 text-xs font-bold text-slate-950 transition hover:bg-sky-300 disabled:opacity-40"
              >
                <Download className="h-3.5 w-3.5" />
              </button>
            </form>

            {isImporting && (
              <p className="text-[11px] text-sky-300">
                Importation et analyse audio en cours...
              </p>
            )}
            {importError && (
              <p className="rounded-md bg-rose-500/15 px-2 py-1 text-[11px] text-rose-300">
                {importError}
              </p>
            )}

            {/* Instant Search & Filter pills */}
            <div className="relative pt-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-slate-500" />
              <input
                type="text"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="Rechercher titre ou artiste..."
                className="w-full rounded-lg border border-white/8 bg-[#07080C] py-1.5 pl-8 pr-3 text-xs text-slate-100 placeholder:text-slate-500 focus:border-white/20 focus:outline-none"
              />
            </div>

            <div className="flex flex-wrap gap-1">
              {(
                [
                  ["all", "Tous"],
                  ["favorites", "Favoris"],
                  ["stems", "Stems prêts"],
                  ["duo", "Duo"],
                ] as [LibraryFilter, string][]
              ).map(([key, label]) => (
                <button
                  key={key}
                  type="button"
                  onClick={() => setFilter(key)}
                  className={`rounded-md px-2 py-1 text-[11px] font-medium transition ${
                    filter === key
                      ? "bg-sky-400/20 text-sky-300"
                      : "bg-white/5 text-slate-400 hover:text-slate-200"
                  }`}
                >
                  {label}
                </button>
              ))}
            </div>
          </div>

          {/* Spotify Public Playlist Metadata Matches */}
          {spotifyCandidates.length > 0 && (
            <div className="border-b border-white/8 bg-emerald-500/5 p-2.5">
              <p className="mb-1.5 text-[11px] font-semibold text-emerald-300">
                Titres Spotify détectés ({spotifyCandidates.length})
              </p>
              <div className="max-h-24 space-y-1 overflow-y-auto">
                {spotifyCandidates.slice(0, 8).map((c, idx) => (
                  <div
                    key={`${c.searchQuery}-${idx}`}
                    className="flex items-center justify-between text-[11px] text-slate-300"
                  >
                    <span className="truncate">{c.artist} — {c.title}</span>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* Track List */}
          <div className="flex-1 space-y-1.5 overflow-y-auto p-2.5">
            {filteredTracks.length === 0 ? (
              <div className="rounded-xl border border-dashed border-white/10 p-5 text-center">
                <p className="text-xs font-medium text-slate-300">
                  Bibliothèque vide
                </p>
                <p className="mt-1 text-[11px] text-slate-500">
                  Ajoutez un fichier MP3/FLAC/WAV/M4A ou collez un lien YouTube ci-dessus.
                </p>
              </div>
            ) : (
              filteredTracks.map((track) => {
                const isActive = track.id === activeTrackId;
                const isSeparating =
                  track.stemStatus === "processing" ||
                  track.stemStatus === "streaming_ready";
                return (
                  <div
                    key={track.id}
                    className={`group rounded-xl border p-2.5 transition ${
                      isActive
                        ? "border-sky-400/50 bg-sky-500/10"
                        : "border-white/6 bg-[#11131B] hover:border-white/15"
                    }`}
                  >
                    <div className="flex items-start justify-between gap-2">
                      <button
                        type="button"
                        onClick={() => onPlayTrack(track.id)}
                        className="min-w-0 flex-1 text-left"
                      >
                        <p className="truncate text-xs font-semibold text-slate-100">
                          {track.title}
                        </p>
                        <p className="truncate text-[11px] text-slate-400">
                          {track.artist} ·{" "}
                          <span className="tabular-nums">
                            {formatTimeMs(track.durationMs)}
                          </span>
                        </p>
                      </button>
                      <div className="flex items-center gap-1">
                        <button
                          type="button"
                          onClick={() => onToggleFavorite(track.id)}
                          title="Favori"
                          className="rounded p-1 text-slate-400 hover:text-rose-400"
                        >
                          <Heart
                            className={`h-3.5 w-3.5 ${
                              track.isFavorite ? "fill-rose-400 text-rose-400" : ""
                            }`}
                          />
                        </button>
                        <button
                          type="button"
                          onClick={() =>
                            onEnqueueTrack(
                              track.id,
                              queueMode,
                              singer1 || "Chanteur 1",
                              queueMode === "duo" ? singer2 || "Chanteur 2" : undefined
                            )
                          }
                          title="Ajouter à la file d'attente"
                          className="rounded p-1 text-slate-400 hover:bg-white/10 hover:text-sky-300"
                        >
                          <Plus className="h-3.5 w-3.5" />
                        </button>
                        <button
                          type="button"
                          onClick={() => onDeleteTrack(track.id)}
                          title="Supprimer"
                          className="rounded p-1 text-slate-500 opacity-0 transition group-hover:opacity-100 hover:text-rose-400"
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </button>
                      </div>
                    </div>

                    {/* Demucs Stem Separation status & action */}
                    <div className="mt-2 flex items-center justify-between gap-2">
                      {track.stemStatus === "completed" ? (
                        <span className="inline-flex items-center gap-1 rounded-md bg-emerald-500/15 px-2 py-0.5 text-[10px] font-semibold text-emerald-300">
                          <Sparkles className="h-3 w-3" />
                          4 Stems prêts
                        </span>
                      ) : isSeparating ? (
                        <div className="flex flex-1 items-center gap-2">
                          <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-white/10">
                            <div
                              className="h-full bg-sky-400 transition-all"
                              style={{ width: `${track.stemProgress}%` }}
                            />
                          </div>
                          <span className="tabular-nums text-[10px] font-semibold text-sky-300">
                            {track.stemStatus === "streaming_ready"
                              ? `Prêt (${track.stemProgress}%)`
                              : `${track.stemProgress}%`}
                          </span>
                        </div>
                      ) : (
                        <button
                          type="button"
                          onClick={() => onSeparateStems(track.id, "htdemucs")}
                          className="inline-flex items-center gap-1 rounded-md bg-white/8 px-2 py-0.5 text-[10px] font-semibold text-slate-200 transition hover:bg-sky-400/20 hover:text-sky-200"
                        >
                          <Sparkles className="h-3 w-3 text-sky-400" />
                          Séparer les pistes (IA)
                        </button>
                      )}

                      {track.hasWordSync ? (
                        <span className="rounded bg-sky-400/15 px-1.5 py-0.5 text-[10px] font-medium text-sky-300">
                          Paroles sync
                        </span>
                      ) : track.hasLyrics ? (
                        <span className="rounded bg-white/8 px-1.5 py-0.5 text-[10px] font-medium text-slate-300">
                          Paroles
                        </span>
                      ) : null}
                    </div>
                  </div>
                );
              })
            )}
          </div>
        </div>
      ) : (
        /* Party Queue (Solo & Duo rotation) */
        <div className="flex flex-1 flex-col overflow-hidden p-3">
          <div className="space-y-2 rounded-xl border border-white/8 bg-[#11131B] p-2.5">
            <div className="flex items-center justify-between">
              <span className="text-[11px] font-semibold text-slate-300">
                Mode d'ajout file d'attente
              </span>
              <div className="flex rounded-lg bg-[#07080C] p-0.5 text-[11px]">
                <button
                  type="button"
                  onClick={() => setQueueMode("solo")}
                  className={`rounded-md px-2 py-0.5 font-semibold ${
                    queueMode === "solo" ? "bg-sky-400 text-slate-950" : "text-slate-400"
                  }`}
                >
                  Solo
                </button>
                <button
                  type="button"
                  onClick={() => setQueueMode("duo")}
                  className={`rounded-md px-2 py-0.5 font-semibold ${
                    queueMode === "duo" ? "bg-rose-400 text-slate-950" : "text-slate-400"
                  }`}
                >
                  Duo
                </button>
              </div>
            </div>
            <div className="grid grid-cols-1 gap-1.5">
              <input
                type="text"
                value={singer1}
                onChange={(e) => setSinger1(e.target.value)}
                placeholder="Nom chanteur Micro 1"
                className="rounded-lg border border-sky-400/30 bg-[#07080C] px-2.5 py-1 text-xs text-slate-100 focus:outline-none"
              />
              {queueMode === "duo" && (
                <input
                  type="text"
                  value={singer2}
                  onChange={(e) => setSinger2(e.target.value)}
                  placeholder="Nom chanteur Micro 2"
                  className="rounded-lg border border-rose-400/30 bg-[#07080C] px-2.5 py-1 text-xs text-slate-100 focus:outline-none"
                />
              )}
            </div>
          </div>

          <button
            type="button"
            onClick={onAdvanceQueue}
            disabled={queue.length === 0}
            className="mt-3 flex items-center justify-center gap-2 rounded-xl bg-sky-400 py-2 text-xs font-bold text-slate-950 transition hover:bg-sky-300 disabled:opacity-40"
          >
            <Play className="h-3.5 w-3.5 fill-current" />
            Lancer le prochain chanteur
          </button>

          <div className="mt-3 flex-1 space-y-2 overflow-y-auto">
            {queue.length === 0 ? (
              <p className="py-8 text-center text-xs text-slate-500">
                File d'attente vide. Ajoutez des morceaux depuis la bibliothèque ou depuis un iPhone !
              </p>
            ) : (
              queue.map((item, idx) => (
                <div
                  key={item.id}
                  className={`rounded-xl border p-2.5 ${
                    item.status === "playing"
                      ? "border-sky-400/50 bg-sky-500/10"
                      : "border-white/8 bg-[#11131B]"
                  }`}
                >
                  <div className="flex items-start justify-between gap-2">
                    <div className="min-w-0">
                      <p className="truncate text-xs font-semibold text-slate-100">
                        {idx + 1}. {item.trackTitle}
                      </p>
                      <p className="truncate text-[11px] text-slate-400">
                        {item.trackArtist}
                      </p>
                    </div>
                    <button
                      type="button"
                      onClick={() => onRemoveQueueItem(item.id)}
                      className="rounded p-1 text-slate-500 hover:text-rose-400"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </button>
                  </div>
                  <div className="mt-1.5 flex items-center gap-1.5 text-[11px]">
                    <span className="rounded bg-sky-400/20 px-1.5 py-0.5 font-medium text-sky-300">
                      {item.singer1}
                    </span>
                    {item.mode === "duo" && item.singer2 && (
                      <>
                        <Users className="h-3 w-3 text-slate-500" />
                        <span className="rounded bg-rose-400/20 px-1.5 py-0.5 font-medium text-rose-300">
                          {item.singer2}
                        </span>
                      </>
                    )}
                  </div>
                </div>
              ))
            )}
          </div>
        </div>
      )}
    </aside>
  );
});
