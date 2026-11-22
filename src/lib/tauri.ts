import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ActiveMicInputs,
  AudioInputDeviceInfo,
  AudioOutputDeviceInfo,
  AudioRoutingState,
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
  StemProgressEvent,
  Track,
  VocalDspParams,
  VocalPreset,
} from "../types/karaok";

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export const DEFAULT_TELEMETRY: PlaybackTelemetry = {
  trackId: null,
  isPlaying: false,
  isRecording: false,
  positionMs: 0,
  durationMs: 215000,
  transposeSemitones: 0,
  tempoPct: 100,
  effectiveKey: { root: "C", scale: "major" },
  stemMix: { vocals: 1.0, drums: 1.0, bass: 1.0, other: 1.0, guitar: 1.0, piano: 1.0 },
  abLoop: { enabled: false, startMs: 0, endMs: 0 },
  latencyCalibrationMs: 0,
  bufferSize: "Frames128",
  mic1: {
    connected: false,
    muted: false,
    vuDb: -96,
    peak: 0,
    jitterMs: 0,
    packetLossPct: 0,
    pitchHz: 0,
    pitchMidi: 0,
    pitchCents: 0,
    gateOpen: false,
  },
  mic2: {
    connected: false,
    muted: false,
    vuDb: -96,
    peak: 0,
    jitterMs: 0,
    packetLossPct: 0,
    pitchHz: 0,
    pitchMidi: 0,
    pitchCents: 0,
    gateOpen: false,
  },
};

export function defaultVocalParams(preset: VocalPreset = "studio"): VocalDspParams {
  switch (preset) {
    case "natural":
      return {
        preset,
        muted: false,
        gain: 2.2,
        hpfHz: 80,
        gateThresholdDb: -68,
        compRatio: 2.2,
        compThresholdDb: -22,
        autotuneEnabled: false,
        scale: "chromatic",
        retuneSpeedMs: 120,
        autotuneMix: 0,
        reverbRoomSize: 0.45,
        reverbDamping: 0.55,
        reverbWet: 0.22,
        presenceDb: 1.5,
      };
    case "live":
      return {
        preset,
        muted: false,
        gain: 2.7,
        hpfHz: 85,
        gateThresholdDb: -64,
        compRatio: 3.5,
        compThresholdDb: -24,
        autotuneEnabled: true,
        scale: "chromatic",
        retuneSpeedMs: 45,
        autotuneMix: 1.0,
        reverbRoomSize: 0.84,
        reverbDamping: 0.28,
        reverbWet: 0.55,
        presenceDb: 3.0,
      };
    case "popStar":
      return {
        preset,
        muted: false,
        gain: 2.8,
        hpfHz: 85,
        gateThresholdDb: -65,
        compRatio: 4.0,
        compThresholdDb: -25,
        autotuneEnabled: true,
        scale: "chromatic",
        retuneSpeedMs: 8,
        autotuneMix: 1.0,
        reverbRoomSize: 0.68,
        reverbDamping: 0.28,
        reverbWet: 0.45,
        presenceDb: 3.8,
      };
    default:
      return {
        preset,
        muted: false,
        gain: 2.5,
        hpfHz: 80,
        gateThresholdDb: -66,
        compRatio: 3.2,
        compThresholdDb: -24,
        autotuneEnabled: true,
        scale: "chromatic",
        retuneSpeedMs: 80,
        autotuneMix: 1.0,
        reverbRoomSize: 0.64,
        reverbDamping: 0.38,
        reverbWet: 0.4,
        presenceDb: 2.8,
      };
  }
}

export const karaokApi = {
  listTracks: (search?: string) =>
    isTauriRuntime() ? invoke<Track[]>("list_tracks", { search: search ?? null }) : Promise.resolve([]),

  importLocalFile: (path: string, title?: string, artist?: string) =>
    invoke<Track>("import_local_file", { path, title: title ?? null, artist: artist ?? null }),

  importYoutubeUrl: (url: string) => invoke<Track>("import_youtube_url", { url }),

  importSpotifyPlaylist: (playlistUrl: string) =>
    invoke<PlaylistTrackCandidate[]>("import_spotify_playlist", { playlistUrl }),

  toggleFavorite: (trackId: string) => invoke<boolean>("toggle_favorite", { trackId }),

  deleteTrack: (trackId: string) => invoke<void>("delete_track", { trackId }),

  separateTrackStems: (trackId: string, model?: DemucsModel) =>
    invoke<void>("separate_track_stems", { trackId, model: model ?? null }),

  loadAndPlayTrack: (trackId: string, autoplay = true) =>
    invoke<Track>("load_and_play_track", { trackId, autoplay }),

  setPlaying: (playing: boolean) =>
    isTauriRuntime() ? invoke<void>("set_playing", { playing }) : Promise.resolve(),

  seekMs: (positionMs: number) =>
    isTauriRuntime() ? invoke<void>("seek_ms", { positionMs: Math.floor(positionMs) }) : Promise.resolve(),

  rewind5s: () => (isTauriRuntime() ? invoke<void>("rewind_5s") : Promise.resolve()),

  setTranspose: (semitones: number, trackId?: string) =>
    isTauriRuntime()
      ? invoke<void>("set_transpose", { semitones, trackId: trackId ?? null })
      : Promise.resolve(),

  setTempoPct: (tempoPct: number) =>
    isTauriRuntime() ? invoke<void>("set_tempo_pct", { tempoPct }) : Promise.resolve(),

  setStemMix: (mix: StemMix) =>
    isTauriRuntime() ? invoke<void>("set_stem_mix", { mix }) : Promise.resolve(),

  setAbLoop: (region: LoopRegion) =>
    isTauriRuntime() ? invoke<void>("set_ab_loop", { region }) : Promise.resolve(),

  setLatencyCalibrationMs: (ms: number) =>
    isTauriRuntime() ? invoke<void>("set_latency_calibration_ms", { ms }) : Promise.resolve(),

  getTelemetry: () =>
    isTauriRuntime() ? invoke<PlaybackTelemetry>("get_telemetry") : Promise.resolve(DEFAULT_TELEMETRY),

  listAudioInputDevices: () =>
    isTauriRuntime()
      ? invoke<AudioInputDeviceInfo[]>("list_audio_input_devices")
      : Promise.resolve([]),

  listAudioOutputDevices: () =>
    isTauriRuntime()
      ? invoke<AudioOutputDeviceInfo[]>("list_audio_output_devices")
      : Promise.resolve([]),

  selectAudioOutputDevice: (deviceName: string | null) =>
    isTauriRuntime()
      ? invoke<AudioRoutingState>("select_audio_output_device", { deviceName })
      : Promise.resolve({ outputDevice: null, mic1Device: null, mic2Device: null }),

  getAudioRoutingState: () =>
    isTauriRuntime()
      ? invoke<AudioRoutingState>("get_audio_routing_state")
      : Promise.resolve({
          outputDevice: null,
          mic1Device: null,
          mic2Device: null,
          clamshellClosed: false,
        }),

  playOutputTestTone: () =>
    isTauriRuntime() ? invoke<void>("play_output_test_tone") : Promise.resolve(),

  selectMicInputDevice: (channel: MicChannelId, deviceName: string | null) =>
    isTauriRuntime()
      ? invoke<ActiveMicInputs>("select_mic_input_device", { channel, deviceName })
      : Promise.resolve({ mic1Device: null, mic2Device: null }),

  getActiveMicInputs: () =>
    isTauriRuntime()
      ? invoke<ActiveMicInputs>("get_active_mic_inputs")
      : Promise.resolve({ mic1Device: null, mic2Device: null }),

  setMicPreset: (channel: MicChannelId, preset: VocalPreset) =>
    isTauriRuntime()
      ? invoke<VocalDspParams>("set_mic_preset", { channel, preset })
      : Promise.resolve(defaultVocalParams(preset)),

  setMicParams: (channel: MicChannelId, params: VocalDspParams) =>
    isTauriRuntime() ? invoke<void>("set_mic_params", { channel, params }) : Promise.resolve(),

  getMicParams: (channel: MicChannelId) =>
    isTauriRuntime()
      ? invoke<VocalDspParams>("get_mic_params", { channel })
      : Promise.resolve(defaultVocalParams("studio")),

  getLyrics: (trackId: string) =>
    isTauriRuntime() ? invoke<LyricsDocument | null>("get_lyrics", { trackId }) : Promise.resolve(null),

  saveLyrics: (trackId: string, doc: LyricsDocument) =>
    invoke<LyricsDocument>("save_lyrics", { trackId, doc }),

  savePlainLyricsForTrack: (trackId: string, plainLyrics: string) =>
    invoke<LyricsDocument>("save_plain_lyrics_for_track", { trackId, plainLyrics }),

  fetchLrclibLyrics: (trackId: string, title: string, artist: string, durationSecs?: number) =>
    invoke<LyricsDocument>("fetch_lrclib_lyrics", {
      trackId,
      title,
      artist,
      durationSecs: durationSecs ?? null,
    }),

  openGoogleLyricsSearch: (title: string, artist: string) =>
    isTauriRuntime()
      ? invoke<string>("open_google_lyrics_search", { title, artist })
      : Promise.resolve(
          `https://www.google.com/search?q=${encodeURIComponent(`${artist} ${title} paroles lyrics`)}`
        ),

  autoAlignLyricsWhisper: (trackId: string, plainLyrics: string) =>
    invoke<LyricsDocument>("auto_align_lyrics_whisper", { trackId, plainLyrics }),

  realignLyricsVocalOnset: (trackId: string) =>
    invoke<LyricsDocument>("realign_lyrics_vocal_onset", { trackId }),

  setLyricsStartHere: (trackId: string, positionMs: number) =>
    invoke<LyricsDocument>("set_lyrics_start_here", {
      trackId,
      positionMs: Math.max(0, Math.floor(positionMs)),
    }),

  shiftLyricsOffset: (trackId: string, deltaMs: number) =>
    invoke<LyricsDocument>("shift_lyrics_offset", { trackId, deltaMs }),

  listQueue: () => (isTauriRuntime() ? invoke<QueueItem[]>("list_queue") : Promise.resolve([])),

  enqueueTrack: (trackId: string, mode: QueueMode, singer1: string, singer2?: string) =>
    invoke<QueueItem>("enqueue_track", {
      trackId,
      mode,
      singer1,
      singer2: singer2 ?? null,
    }),

  advanceQueue: () => invoke<QueueItem | null>("advance_queue"),

  removeQueueItem: (itemId: string) => invoke<void>("remove_queue_item", { itemId }),

  startRecording: (title: string) => invoke<void>("start_recording", { title }),

  stopRecording: () => invoke<RecordingEntry>("stop_recording"),

  listRecordings: () =>
    isTauriRuntime() ? invoke<RecordingEntry[]>("list_recordings") : Promise.resolve([]),

  previewRecordingMix: (recordingId: string, vocalBalance: number) =>
    invoke<void>("preview_recording_mix", { recordingId, vocalBalance }),

  exportRecordingMix: (recordingId: string, vocalBalance: number, outputPath: string) =>
    invoke<string>("export_recording_mix", { recordingId, vocalBalance, outputPath }),

  toggleTvWindow: () =>
    isTauriRuntime() ? invoke<boolean>("toggle_tv_window") : Promise.resolve(false),

  onTelemetry: (cb: (t: PlaybackTelemetry) => void): Promise<UnlistenFn> =>
    isTauriRuntime()
      ? listen<PlaybackTelemetry>("karaok-telemetry", (e) => cb(e.payload))
      : Promise.resolve(() => {}),

  onStemProgress: (cb: (ev: StemProgressEvent) => void): Promise<UnlistenFn> =>
    isTauriRuntime()
      ? listen<StemProgressEvent>("stem-progress", (e) => cb(e.payload))
      : Promise.resolve(() => {}),

  onActiveTrackChanged: (cb: (track: Track) => void): Promise<UnlistenFn> =>
    isTauriRuntime()
      ? listen<Track>("active-track-changed", (e) => cb(e.payload))
      : Promise.resolve(() => {}),

  onLyricsUpdated: (cb: (trackId: string) => void): Promise<UnlistenFn> =>
    isTauriRuntime()
      ? listen<string>("lyrics-updated", (e) => cb(e.payload))
      : Promise.resolve(() => {}),

  onQueueUpdated: (cb: () => void): Promise<UnlistenFn> =>
    isTauriRuntime() ? listen("queue-updated", () => cb()) : Promise.resolve(() => {}),

  onAudioRoutingUpdated: (cb: () => void): Promise<UnlistenFn> =>
    isTauriRuntime()
      ? listen("audio-routing-updated", () => cb())
      : Promise.resolve(() => {}),
};
