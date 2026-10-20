// The engine's side of the Recordem API (packages/contract): the live transcript while the call goes on, then the
// final transcript and the audio tracks, all with the recording's upload token (scope recordings:upload: good for
// that one recording, on the routes marked `x-upload`). The host created the recording; every call here is
// idempotent, so a retry after a failure resends safely.
use serde::{Deserialize, Serialize};

/// Where a recording goes: the API, and the recording's upload token.
#[derive(Debug, Clone)]
pub struct Target {
    pub api_url: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub channel: String, // mic | system
    pub text: String,
    /// What this turn was transcribed as (pt, en, …); turns of one call can differ.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Whose voice: "mic", or "system:1", "system:2", … by first appearance; none when the voices weren't told apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

/// Something the reader profile's Monitor found (a rule that stopped matching, nobody shown speaking…).
#[derive(Debug, Clone, Serialize)]
pub struct ReaderReport {
    pub t_ms: u64,
    pub profile: String,
    pub revision: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    pub event: serde_json::Value,
}

/// A name for a voice of the transcript: "system:1" → "Ana Souza".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeakerName {
    pub label: String,
    pub name: String,
    pub source: String, // platform | llm | user
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

/// One person the call app shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Participant {
    pub name: String,
    pub speaking: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not", rename = "self")]
    pub is_self: bool,
}

/// Who is in the call, at `t_ms` from the start of the recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Participants {
    pub t_ms: u64,
    pub visible: bool,
    pub people: Vec<Participant>,
}

/// The final transcript as kept on disk (transcript.json) and sent with PUT /v1/recordings/{id}/transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub ended_at: String,
    pub language: Option<String>,
    pub model: Option<String>,
    pub segments: Vec<Segment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speakers: Vec<SpeakerName>,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().timeout(std::time::Duration::from_secs(60)).build().expect("http client")
}

fn url(c: &Target, path: &str) -> String {
    format!("{}/v1{}", c.api_url.trim_end_matches('/'), path)
}

/// The API refused the token (expired, or not for this recording): a fresh one is the fix, not a retry.
#[derive(Debug)]
pub struct Unauthorized(pub String);

impl std::fmt::Display for Unauthorized {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the upload token was refused ({})", self.0)
    }
}

impl std::error::Error for Unauthorized {}

async fn check(r: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    if r.status().is_success() {
        return Ok(r);
    }
    let status = r.status();
    let body: serde_json::Value = r.json().await.unwrap_or_default();
    let msg = body.get("message").or_else(|| body.get("error")).and_then(|e| e.as_str()).unwrap_or("").to_string();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(Unauthorized(msg).into());
    }
    anyhow::bail!("{} {}", status.as_u16(), msg)
}

pub async fn deliver(c: &Target, id: &str, t: &Transcript) -> anyhow::Result<()> {
    let r = client().put(url(c, &format!("/recordings/{id}/transcript"))).bearer_auth(&c.token).json(t).send().await?;
    check(r).await?;
    Ok(())
}

/// Part of the live transcript of a call still going on: the segments from index `from` on (the API holds `from`).
#[derive(Debug, Serialize)]
pub struct LiveBatch<'a> {
    pub from: usize,
    pub segments: &'a [Segment],
    /// Who is in the call now, when it changed since the last batch the API took.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participants: Option<&'a Participants>,
    /// What went wrong reading the call app since the last batch the API took (participants.rs).
    #[serde(skip_serializing_if = "<[ReaderReport]>::is_empty")]
    pub reader: &'a [ReaderReport],
    /// Names for the live voices, when one appeared since the last batch the API took.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speakers: Option<&'a [SpeakerName]>,
    /// The last batch: the recording ended.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ended: bool,
}

/// What the API answers a live batch: how many segments it holds, and whether "Stop" was pressed in the portal.
#[derive(Debug, Deserialize)]
pub struct LiveAck {
    pub n: usize,
    #[serde(default)]
    pub stop: bool,
}

pub async fn live(c: &Target, id: &str, b: &LiveBatch<'_>) -> anyhow::Result<LiveAck> {
    let r = client().post(url(c, &format!("/recordings/{id}/live"))).bearer_auth(&c.token).json(b).send().await?;
    Ok(check(r).await?.json().await?)
}

/// One audio track: `mix` (the whole call, for playback) or `mic_hq` (your own voice at 48 kHz). Both AAC (.m4a).
pub async fn upload_track(c: &Target, id: &str, kind: &str, path: &std::path::Path) -> anyhow::Result<()> {
    let body = tokio::fs::read(path).await?;
    let r = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(900))
        .build()?
        .put(url(c, &format!("/recordings/{id}/tracks/{kind}")))
        .bearer_auth(&c.token)
        .header("content-type", "audio/mp4")
        .body(body)
        .send()
        .await?;
    check(r).await?;
    Ok(())
}

/// The engine gave up on the recording: the server keeps why and tells the host's backend (recording.failed).
/// `reason`: permission_denied | capture_failed | transcription_failed | upload_failed | other.
pub async fn report_failure(c: &Target, id: &str, reason: &str, message: &str) -> anyhow::Result<()> {
    let body = serde_json::json!({ "reason": reason, "message": message.chars().take(2000).collect::<String>() });
    let r = client().post(url(c, &format!("/recordings/{id}/failure"))).bearer_auth(&c.token).json(&body).send().await?;
    check(r).await?;
    Ok(())
}

/// One of the reader's broken snapshots (dumps.rs), gzipped JSON.
pub async fn put_reader_dump(c: &Target, id: &str, name: &str, body: Vec<u8>) -> anyhow::Result<()> {
    let r = client()
        .put(url(c, &format!("/recordings/{id}/reader-dumps/{name}")))
        .bearer_auth(&c.token)
        .header("content-type", "application/gzip")
        .body(body)
        .send()
        .await?;
    check(r).await?;
    Ok(())
}
