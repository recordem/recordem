// The recorder's side of the Recordem API (packages/contract): ask what a call is, create the recording, send the
// live transcript while the call goes on, then the final transcript and the audio tracks. Every call is idempotent
// by the recording id the recorder picked, so a retry after a failure resends safely.
use crate::config::Config;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Match {
    pub meeting_id: String,
    pub title: Option<String>,
    pub starts_at: String,
    pub matched_by: String, // link | time
}

#[derive(Debug, Serialize)]
pub struct ContextRequest<'a> {
    pub at: String,
    pub url: Option<&'a str>,
    pub app: Option<&'a str>,
    pub title: Option<&'a str>,
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

#[derive(Debug, Serialize)]
pub struct NewRecording<'a> {
    pub id: &'a str,
    pub meeting_id: Option<&'a str>,
    pub started_at: &'a str,
    pub url: Option<&'a str>,
    pub app: Option<&'a str>,
    pub title: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
pub struct Recording {
    pub meeting_id: String,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().timeout(std::time::Duration::from_secs(60)).build().expect("http client")
}

fn url(c: &Config, path: &str) -> String {
    format!("{}/v1{}", c.api_url.trim_end_matches('/'), path)
}

async fn check(r: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    if r.status().is_success() {
        return Ok(r);
    }
    let status = r.status();
    let body: serde_json::Value = r.json().await.unwrap_or_default();
    let msg = body.get("message").or_else(|| body.get("error")).and_then(|e| e.as_str()).unwrap_or("").to_string();
    anyhow::bail!("{} {}", status.as_u16(), msg)
}

/// Checks the token works against this API (right audience, a workspace).
pub async fn me(c: &Config) -> anyhow::Result<()> {
    let r = client().get(url(c, "/me")).bearer_auth(crate::oauth::bearer(c).await?).send().await?;
    check(r).await?;
    Ok(())
}

pub async fn context(c: &Config, req: &ContextRequest<'_>) -> anyhow::Result<Vec<Match>> {
    #[derive(Deserialize)]
    struct Res {
        matches: Vec<Match>,
    }
    let r = client().post(url(c, "/meetings/context")).bearer_auth(crate::oauth::bearer(c).await?).json(req).send().await?;
    Ok(check(r).await?.json::<Res>().await?.matches)
}

/// Creates the recording (and its meeting, when it isn't linked to one). The same id again returns the same recording.
pub async fn create(c: &Config, r: &NewRecording<'_>) -> anyhow::Result<Recording> {
    let res = client().post(url(c, "/recordings")).bearer_auth(crate::oauth::bearer(c).await?).json(r).send().await?;
    Ok(check(res).await?.json().await?)
}

pub async fn deliver(c: &Config, id: &str, t: &Transcript) -> anyhow::Result<()> {
    let r = client().put(url(c, &format!("/recordings/{id}/transcript"))).bearer_auth(crate::oauth::bearer(c).await?).json(t).send().await?;
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

pub async fn live(c: &Config, id: &str, b: &LiveBatch<'_>) -> anyhow::Result<LiveAck> {
    let r = client().post(url(c, &format!("/recordings/{id}/live"))).bearer_auth(crate::oauth::bearer(c).await?).json(b).send().await?;
    Ok(check(r).await?.json().await?)
}

/// One audio track: `mix` (the whole call, for playback) or `mic_hq` (your own voice at 48 kHz). Both AAC (.m4a).
pub async fn upload_track(c: &Config, id: &str, kind: &str, path: &std::path::Path) -> anyhow::Result<()> {
    let body = tokio::fs::read(path).await?;
    let r = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(900))
        .build()?
        .put(url(c, &format!("/recordings/{id}/tracks/{kind}")))
        .bearer_auth(crate::oauth::bearer(c).await?)
        .header("content-type", "audio/mp4")
        .body(body)
        .send()
        .await?;
    check(r).await?;
    Ok(())
}

/// One of the reader's broken snapshots (dumps.rs), gzipped JSON.
pub async fn put_reader_dump(c: &Config, id: &str, name: &str, body: Vec<u8>) -> anyhow::Result<()> {
    let r = client()
        .put(url(c, &format!("/recordings/{id}/reader-dumps/{name}")))
        .bearer_auth(crate::oauth::bearer(c).await?)
        .header("content-type", "application/gzip")
        .body(body)
        .send()
        .await?;
    check(r).await?;
    Ok(())
}

/// A meeting's page in the portal.
pub fn meeting_url(c: &Config, meeting_id: &str) -> String {
    format!("{}/m/{meeting_id}", c.web_url.trim_end_matches('/'))
}
