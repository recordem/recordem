# @recordem/desktop-sdk

Record calls from your Electron or Node app with [Recordem](https://github.com/recordem/recordem). The SDK runs the
Recordem engine next to your app: it notices calls (Google Meet, Zoom, Teams, Webex, in the browser or their apps),
records the microphone and the call's audio apart, reads who is in the call and who speaks, transcribes on the computer,
and sends everything to Recordem. Your backend gets the transcript, the audio and notes by webhook.

macOS on Apple silicon (14.4+). The engine is downloaded on install (`bin/recordem-desktop`), checked against the
SHA-256 in `engine.json`.

## How it fits

1. Your backend creates the recording with its Recordem API key, and asks for an upload token:
   `POST /v1/recordings` with `{ id, ref: "interview:123", upload: true }` (or `@recordem/sdk`). The token is good for
   that one recording, for 6 hours.
2. Your desktop app gives the engine that token. It never sees your API key.
3. Your backend hears `transcript.final`, `recording.uploaded` and `notes.ready` by webhook, and reads them with
   `@recordem/sdk`.

## Electron

```ts
import { app } from "electron";
import sdk from "@recordem/desktop-sdk";

await sdk.init({ apiUrl: "https://api.recordem.app", dataDir: app.getPath("userData") });

sdk.addEventListener("meeting-detected", async ({ window }) => {
  // Ask your backend for a recording of this call (it creates it with upload: true).
  const { uploadToken } = await fetch("https://your.app/interviews/123/recording", { method: "POST" }).then((r) => r.json());
  await sdk.startRecording({ windowId: window.id, uploadToken });
});

sdk.addEventListener("realtime-event", ({ event, data }) => {
  if (event === "transcript.data") console.log(`${data.name ?? data.speaker}: ${data.text}`);
});

sdk.addEventListener("upload-status", ({ recordingId, status }) => console.log(recordingId, status));
```

The engine has to sit outside the asar archive: with electron-builder, `asarUnpack: ["**/@recordem/desktop-sdk/bin/**"]`.
Sign it with your app (it records in your app's name: macOS asks your users for the microphone, system audio and
accessibility permissions as your app). Your Info.plist needs `NSMicrophoneUsageDescription` and
`NSAudioCaptureUsageDescription`.

## API

| Method | |
| --- | --- |
| `init({ apiUrl, dataDir, languages?, live?, micDevice?, … })` | starts the engine; resolves `{ modelsReady, pending }` |
| `downloadModels()` | the speech models, once (~575 MB); progress as `model-download` |
| `startRecording({ windowId, uploadToken })` | resolves the recording id |
| `startRecording({ windowId, recordingId })`, then `setUploadToken({ recordingId, uploadToken })` | start before your backend answers |
| `stopRecording`, `pauseRecording`, `resumeRecording` (`{ windowId }`) | |
| `prepareDesktopAudioRecording()` | a window to record with no call (in person) |
| `permissions()`, `requestPermission(p)`, `openPermissionSettings(p)` | `microphone`, `system-audio`, `accessibility`, `automation` |
| `setMuted(muted)`, `listDevices()`, `pending()`, `configure(…)`, `shutdown()` | |

Events: `meeting-detected`, `meeting-updated`, `meeting-closed`, `recording-started`, `recording-ended`,
`upload-status` (`transcribing`, `uploading`, `done`, `failed`, `waiting` for a token), `realtime-event`
(`transcript.data`, `participants.update`, `speaker.on`), `permission-status`, `mute-status`, `model-download`,
`error`, `log`, `shutdown`. If the engine exits on its own it is started again with the same `init`.

A recording that couldn't be sent stays on the computer and is resumed at the next `init`; one whose token expired
comes back as `upload-status` `waiting`: get a fresh token (create the recording again with the same `id`) and call
`setUploadToken`.

`RECORDEM_DESKTOP_PATH` points the SDK at an engine of your own (development), and skips the download.

The same engine has a Rust client for Tauri apps: the `recordem-desktop` crate.

MIT
