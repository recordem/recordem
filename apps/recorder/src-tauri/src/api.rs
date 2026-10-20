// The app's side of the Recordem API (packages/contract), as the person signed in: ask what a call is, and create the
// recording with an upload token for the engine, which sends everything else with it (like any Desktop SDK host's
// backend would). Creating is idempotent by the recording id the app picked, and asked again it gives a fresh token.
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

#[derive(Debug, Serialize)]
pub struct NewRecording<'a> {
    pub id: &'a str,
    pub meeting_id: Option<&'a str>,
    pub started_at: &'a str,
    pub url: Option<&'a str>,
    pub app: Option<&'a str>,
    pub title: Option<&'a str>,
    /// Also an upload token, for the engine to send the recording with.
    pub upload: bool,
}

#[derive(Debug, Deserialize)]
pub struct Upload {
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct Recording {
    pub meeting_id: String,
    pub upload: Option<Upload>,
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

/// A meeting's page in the portal.
pub fn meeting_url(c: &Config, meeting_id: &str) -> String {
    format!("{}/m/{meeting_id}", c.web_url.trim_end_matches('/'))
}
