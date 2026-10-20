// The protocol between a host app and the recordem-desktop engine (apps/recorder/desktop-client/src/protocol.rs is the
// source; keep the two in step). One JSON object per line: requests on the engine's stdin, responses and events on its
// stdout. A response only says the command was taken; what comes of it arrives as events.

/** Bumped on a change a host must know about; additions keep it. */
export const PROTOCOL = 1;

/** A call the engine noticed (a meeting app or browser took the microphone), or a window made for a recording with no
 * call (`prepareDesktopAudioRecording`). */
export type RecordemWindow = {
  id: string;
  /** The window's or the browser tab's title. */
  title?: string;
  /** The call's link, from the browser tab (needs the automation permission). */
  url?: string;
  /** google_meet | zoom | microsoft_teams | webex */
  platform?: string;
  /** The app's bundle id (com.google.Chrome, us.zoom.xos…). */
  appId?: string;
  appName?: string;
};

export type InitConfig = {
  /** The Recordem API recordings are sent to (https://api.recordem.app). */
  apiUrl: string;
  /** Where models, recordings not sent yet and caches are kept: in Electron, `app.getPath("userData")`. */
  dataDir: string;
  /** The languages calls may be in, comma-separated ("en,pt"). */
  languages?: string;
  /** Extra words for speech recognition, one per line. */
  vocabulary?: string;
  /** Speech recognition while the call goes on and after it: "whisper" (default), "parakeet", "parakeet:ultra". */
  asrLive?: string;
  asrFinal?: string;
  /** Transcribe while the call goes on (`realtime-event`); default true. */
  live?: boolean;
  /** Input device by name (see `listDevices`); empty = the system default. */
  micDevice?: string;
  /** Also record your own mic at 48 kHz without echo cancellation (track `mic_hq`); default true. */
  voiceHq?: boolean;
  /** Keep the audio files after they are sent; default false. */
  keepAudio?: boolean;
  /** Send what the call window looked like when reading it broke; default false. */
  shareReaderDumps?: boolean;
  /** The name of the person on this computer, when the call app doesn't show it. */
  selfName?: string;
  /** Watch for calls (`meeting-detected`); default true. */
  detect?: boolean;
};

/** A recording not sent yet: waiting for a token (`setUploadToken`), or one that failed and is retried. */
export type PendingRecording = {
  recordingId: string;
  window: RecordemWindow;
  startedAt: string;
  endedAt?: string;
  /** Its token is missing or expired: send a fresh one with `setUploadToken`. */
  needsToken: boolean;
  /** What the host gave `startRecording`. */
  metadata?: unknown;
};

export type Ready = { protocol: number; version: string; modelsReady: boolean; pending: PendingRecording[] };

export type StartRecording = {
  windowId: string;
  /** The token your backend got from `POST /v1/recordings` with `upload: true`. It names the recording. */
  uploadToken?: string;
  /** Instead of a token, to start before your backend answers: the id the recording will be created with. Its token
   * comes later with `setUploadToken`; until then nothing is sent. */
  recordingId?: string;
  /** Kept with the recording and given back in `pending`. */
  metadata?: unknown;
};

export type Permission = "microphone" | "system-audio" | "accessibility" | "automation";
export type PermissionStatus = "granted" | "denied" | "not_requested" | "unknown";
export type PermissionItem = { permission: Permission; status: PermissionStatus; required: boolean };
export type UploadStatus = "waiting" | "transcribing" | "uploading" | "done" | "failed";

/** A line of the live transcript (`realtime-event` with `event: "transcript.data"`). */
export type TranscriptLine = {
  start_ms: number;
  end_ms: number;
  channel: "mic" | "system";
  text: string;
  language?: string;
  /** "mic", or "system:1", "system:2"… */
  speaker?: string;
  /** Who it is, when the call app showed them speaking. */
  name?: string;
};

/** Every event, by name: what `addEventListener(name, …)` gets. */
export type Events = {
  hello: { protocol: number; version: string };
  "meeting-detected": { window: RecordemWindow };
  "meeting-updated": { window: RecordemWindow };
  /** The call let go of the microphone (and isn't recorded), or its recording stopped after it did. */
  "meeting-closed": { window: RecordemWindow };
  "recording-started": { window: RecordemWindow; recordingId: string };
  /** `warning`: a channel stayed silent (usually a missing permission). `error`: the capture failed. */
  "recording-ended": { window: RecordemWindow; recordingId: string; warning?: string; error?: string };
  /** Transcribing after the call (progress 0–1000), sending, sent, failed (retried at the next start), or waiting
   * for a token. */
  "upload-status": { recordingId: string; status: UploadStatus; progress?: number; error?: string };
  /** While recording: `transcript.data` (a TranscriptLine), `participants.update` (who is in the call), `speaker.on`
   * (`{ t_ms, names }`: who the call app shows speaking). */
  "realtime-event": { window: RecordemWindow; recordingId: string; event: "transcript.data" | "participants.update" | "speaker.on"; data: any };
  "permission-status": { permission: Permission; status: PermissionStatus };
  "mute-status": { window: RecordemWindow; muted: boolean };
  "model-download": { done: number; total: number; ready: boolean; error?: string };
  error: { type: string; message: string };
  log: { level: string; message: string };
  /** The engine stopped: `shutdown()`, or it exited (then it is started again with the same `init`). */
  shutdown: { code?: number | null };
};

export type EventType = keyof Events;
