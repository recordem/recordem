// The protocol between a host app and the recordem-desktop engine: one JSON object per line, both ways. The host
// writes requests on the engine's stdin; the engine writes responses and events on its stdout (logs go to stderr).
//
//   → {"id":7,"cmd":"startRecording","params":{"windowId":"w1","uploadToken":"eyJ…"}}
//   ← {"id":7,"ok":true,"result":{"recordingId":"5b0e…"}}
//   ← {"event":"recording-started","payload":{"window":{"id":"w1","platform":"google_meet"},"recordingId":"5b0e…"}}
//
// A response only says the command was taken; what comes of it arrives as events. The first line the engine writes
// is the `hello` event, with PROTOCOL: a host refuses an engine whose protocol it doesn't know.
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Bumped on a change a host must know about; additions (a new event, a new optional field) keep it.
pub const PROTOCOL: u32 = 1;

/// A request from the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub cmd: String,
    #[serde(default)]
    pub params: Value,
}

/// The engine's answer to request `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A line from the engine: an answer, or an event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Response(Response),
    Event(Event),
}

/// A call the engine noticed (a meeting app or browser started using the microphone), or one made up for a
/// recording with no call (`prepareDesktopAudioRecording`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub id: String,
    /// The window's or the browser tab's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The call's link, read from the browser tab (needs the automation permission).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// google_meet | zoom | microsoft_teams | webex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// The app's bundle id (com.google.Chrome, us.zoom.xos…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
}

/// How the engine works: `init` takes all of it (`apiUrl` and `dataDir` required), `configure` any part.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// The Recordem API the recordings are sent to (https://api.recordem.dev).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    /// Where models, recordings not sent yet and caches are kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
    /// The languages calls may be in, comma-separated ("en,pt").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub languages: Option<String>,
    /// Extra words for speech recognition, one per line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vocabulary: Option<String>,
    /// Speech recognition while the call goes on and after it: "whisper", "parakeet", "parakeet:ultra".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_live: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_final: Option<String>,
    /// Transcribe while the call goes on (live transcript, `realtime-event`); default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<bool>,
    /// Input device by name; none = the system default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mic_device: Option<String>,
    /// Also record your own mic at 48 kHz without echo cancellation (track `mic_hq`); default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_hq: Option<bool>,
    /// Keep the audio files after they are sent; default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_audio: Option<bool>,
    /// Send what the call window looked like when reading it broke; default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_reader_dumps: Option<bool>,
    /// The name of the person on this computer, when the call app doesn't show it: their voice is the mic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_name: Option<String>,
    /// Watch for calls (`meeting-detected`); default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detect: Option<bool>,
}

/// What `init` answers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ready {
    pub protocol: u32,
    pub version: String,
    /// The speech models are on disk; otherwise `downloadModels` first.
    pub models_ready: bool,
    /// Recordings not sent yet (see `pending`).
    pub pending: Vec<Pending>,
}

/// A recording not sent yet: waiting for a token (`setUploadToken`), or one that failed and is retried.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub recording_id: String,
    pub window: Window,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    /// Its token is missing or expired: send a fresh one with `setUploadToken`.
    pub needs_token: bool,
    /// What the host gave `startRecording`, as it was.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRecording {
    pub window_id: String,
    /// The token `POST /v1/recordings` with `upload: true` gave the host's backend. It names the recording.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_token: Option<String>,
    /// Instead of a token, to start before the server answers (offline): the id the recording will be created with.
    /// Its token comes later with `setUploadToken`; until then nothing is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_id: Option<String>,
    /// Kept with the recording and given back in `pending`.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    Microphone,
    SystemAudio,
    Accessibility,
    Automation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionStatus {
    Granted,
    Denied,
    NotRequested,
    /// Can't tell now (automation: the browser isn't running).
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionItem {
    pub permission: Permission,
    pub status: PermissionStatus,
    /// Recording can't work without it (microphone, system audio).
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadStatus {
    /// No upload token yet, or it expired: `setUploadToken`.
    Waiting,
    Transcribing,
    Uploading,
    Done,
    Failed,
}

/// What the engine tells the host, as `{"event": <name>, "payload": {…}}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "payload", rename_all = "kebab-case")]
pub enum Event {
    /// The first line: which engine this is.
    #[serde(rename_all = "camelCase")]
    Hello { protocol: u32, version: String },
    MeetingDetected { window: Window },
    MeetingUpdated { window: Window },
    /// The call let go of the microphone (and isn't recorded), or its recording stopped after it did.
    MeetingClosed { window: Window },
    #[serde(rename_all = "camelCase")]
    RecordingStarted { window: Window, recording_id: String },
    /// `warning`: a channel stayed silent (usually a missing permission). `error`: the capture failed.
    #[serde(rename_all = "camelCase")]
    RecordingEnded {
        window: Window,
        recording_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        warning: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Transcribing after the call (progress 0–1000), sending, sent, or failed (retried at the next start).
    #[serde(rename_all = "camelCase")]
    UploadStatus {
        recording_id: String,
        status: UploadStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// While recording: `transcript.data` (a line of the live transcript), `participants.update` (who is in the call),
    /// `speaker.on` (who the call app shows speaking).
    #[serde(rename_all = "camelCase")]
    RealtimeEvent { window: Window, recording_id: String, event: String, data: Value },
    PermissionStatus { permission: Permission, status: PermissionStatus },
    /// The recording's mic is silence (`setMuted`, or the call's own mute button).
    MuteStatus { window: Window, muted: bool },
    /// `downloadModels`: bytes so far; `ready` once done.
    ModelDownload {
        done: u64,
        total: u64,
        ready: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Error {
        #[serde(rename = "type")]
        kind: String,
        message: String,
    },
    Log { level: String, message: String },
    /// The engine stopped (`shutdown`, or it exited: then the client restarts it).
    Shutdown {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
    },
}

/// The recording an upload token is for (its `recording` claim). The token isn't checked here: the API does that.
pub fn token_recording(token: &str) -> Option<String> {
    let claims = token_claims(token)?;
    claims.get("recording")?.as_str().map(String::from)
}

/// When an upload token expires (unix seconds), from its `exp` claim.
pub fn token_expiry(token: &str) -> Option<i64> {
    token_claims(token)?.get("exp")?.as_i64()
}

fn token_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    serde_json::from_slice(&base64url(payload)?).ok()
}

fn base64url(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|c| *c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_tagged_by_name() {
        let e = Event::RecordingStarted { window: Window { id: "w1".into(), ..Default::default() }, recording_id: "r".into() };
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(s, r#"{"event":"recording-started","payload":{"window":{"id":"w1"},"recordingId":"r"}}"#);
        let back: Message = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, Message::Event(Event::RecordingStarted { .. })));
        let r: Message = serde_json::from_str(r#"{"id":3,"ok":true,"result":{"windowId":"w2"}}"#).unwrap();
        assert!(matches!(r, Message::Response(Response { id: 3, ok: true, .. })));
    }

    #[test]
    fn reads_the_recording_of_a_token() {
        // {"alg":"none"}.{"recording":"rec-1","exp":1800000000}
        let t = "eyJhbGciOiJub25lIn0.eyJyZWNvcmRpbmciOiJyZWMtMSIsImV4cCI6MTgwMDAwMDAwMH0.";
        assert_eq!(token_recording(t).as_deref(), Some("rec-1"));
        assert_eq!(token_expiry(t), Some(1_800_000_000));
        assert_eq!(token_recording("nope"), None);
    }
}
