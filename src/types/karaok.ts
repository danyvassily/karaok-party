export type PitchClass =
  | "C"
  | "CSharp"
  | "D"
  | "DSharp"
  | "E"
  | "F"
  | "FSharp"
  | "G"
  | "GSharp"
  | "A"
  | "ASharp"
  | "B";

export type ScaleType = "chromatic" | "major" | "minor";

export interface MusicalKey {
  root: PitchClass;
  scale: ScaleType;
}

export type StemStatus =
  | "pending"
  | "processing"
  | "streaming_ready"
  | "completed"
  | "failed";

export type DemucsModel = "htdemucs" | "htdemucs_6s";
export type TrackSource = "local" | "youtube";

export interface Track {
  id: string;
  title: string;
  artist: string;
  album: string | null;
  durationMs: number;
  sourceType: TrackSource;
  sourcePath: string;
  sourceUrl: string | null;
  contentHash: string;
  musicalKey: MusicalKey;
  bpm: number | null;
  favoriteTranspose: number;
  isFavorite: boolean;
  stemStatus: StemStatus;
  stemProgress: number;
  stemModel: DemucsModel;
  stemsDir: string | null;
  hasLyrics: boolean;
  hasWordSync: boolean;
  createdAt: string;
}

export interface LyricsWord {
  text: string;
  startMs: number;
  endMs: number;
}

export interface LyricsLine {
  startMs: number;
  endMs: number;
  text: string;
  words: LyricsWord[];
}

export interface LyricsDocument {
  format: "plain" | "lrc" | "enhanced_lrc";
  source: "manual" | "lrclib" | "whisper";
  hasWordSync: boolean;
  offsetMs: number;
  lines: LyricsLine[];
}

export interface StemMix {
  vocals: number;
  drums: number;
  bass: number;
  other: number;
  guitar: number;
  piano: number;
}

export interface LoopRegion {
  enabled: boolean;
  startMs: number;
  endMs: number;
}

export type MicChannelId = "mic1" | "mic2";
export type VocalPreset = "natural" | "studio" | "live" | "popStar" | "expert";

export interface VocalDspParams {
  preset: VocalPreset;
  muted: boolean;
  gain: number;
  hpfHz: number;
  gateThresholdDb: number;
  compRatio: number;
  compThresholdDb: number;
  autotuneEnabled: boolean;
  scale: ScaleType;
  retuneSpeedMs: number;
  autotuneMix: number;
  reverbRoomSize: number;
  reverbDamping: number;
  reverbWet: number;
  presenceDb: number;
}

export interface MicTelemetry {
  connected: boolean;
  muted: boolean;
  vuDb: number;
  peak: number;
  jitterMs: number;
  packetLossPct: number;
  pitchHz: number;
  pitchMidi: number;
  pitchCents: number;
  gateOpen: boolean;
}

export interface PlaybackTelemetry {
  trackId: string | null;
  isPlaying: boolean;
  isRecording: boolean;
  positionMs: number;
  durationMs: number;
  transposeSemitones: number;
  tempoPct: number;
  effectiveKey: MusicalKey;
  stemMix: StemMix;
  abLoop: LoopRegion;
  latencyCalibrationMs: number;
  bufferSize: "Frames64" | "Frames128" | "Frames256";
  mic1: MicTelemetry;
  mic2: MicTelemetry;
}

export interface AudioInputDeviceInfo {
  name: string;
  isDefault: boolean;
  isIphone: boolean;
  isBluetooth?: boolean;
  sampleRate: number;
  channels: number;
}

export interface AudioOutputDeviceInfo {
  name: string;
  isDefault: boolean;
  isBluetooth?: boolean;
  sampleRate: number;
  channels: number;
}

export interface AudioRoutingState {
  outputDevice: string | null;
  mic1Device: string | null;
  mic2Device: string | null;
  clamshellClosed?: boolean;
}

export interface ActiveMicInputs {
  mic1Device: string | null;
  mic2Device: string | null;
}

export interface MicPairingInfo {
  serverUrl: string;
  mdnsHostname: string;
  lanIp: string;
  port: number;
  caCertUrl: string;
  caCertQrSvg: string;
  mic1Url: string;
  mic2Url: string;
  mic1QrSvg: string;
  mic2QrSvg: string;
  sessionToken: string;
}

export type QueueMode = "solo" | "duo";

export interface QueueItem {
  id: string;
  trackId: string;
  trackTitle: string;
  trackArtist: string;
  position: number;
  mode: QueueMode;
  singer1: string;
  singer2: string | null;
  status: string;
  addedBy: string;
}

export interface RecordingEntry {
  id: string;
  trackId: string | null;
  title: string;
  createdAt: string;
  durationMs: number;
  instrumentalPath: string;
  mic1Path: string;
  mic2Path: string;
  mixVocalBalance: number;
  exportedPath: string | null;
}

export interface StemProgressEvent {
  trackId: string;
  status: StemStatus;
  progressPct: number;
  completedChunks: number;
  totalChunks: number;
  stemsDir: string;
}

export interface PlaylistTrackCandidate {
  title: string;
  artist: string;
  searchQuery: string;
}
