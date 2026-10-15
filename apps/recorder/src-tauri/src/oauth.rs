// Signing in: OAuth 2.1 authorization code + PKCE against the Recordem identity provider, as the first-party public
// client "recordem-recorder" with the recordem://oauth redirect. The person signs in in their browser; the app gets an
// access token for the API (its URL is the token's audience) and a rotating refresh token.
use crate::config::{self, Config};
use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Mutex;

pub const CLIENT_ID: &str = "recordem-recorder";
pub const REDIRECT: &str = "recordem://oauth";
const SCOPE: &str = "openid profile email offline_access meetings:read meetings:write transcripts:read";

/// The login in flight: (state, PKCE verifier). One at a time; a new sign-in replaces it.
static PENDING: Mutex<Option<(String, String)>> = Mutex::new(None);
/// Refresh tokens rotate: two refreshes at once would burn one. Serialize them.
static REFRESH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Deserialize)]
struct Metadata {
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build().expect("http client")
}

fn random(n: usize) -> String {
    let bytes: Vec<u8> = (0..n).map(|_| rand::random::<u8>()).collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

async fn metadata(c: &Config) -> anyhow::Result<Metadata> {
    let url = format!("{}/.well-known/oauth-authorization-server", c.auth_url.trim_end_matches('/'));
    Ok(http().get(url).send().await?.error_for_status()?.json().await?)
}

/// The API's address as the tokens' audience (no trailing slash: it must equal the server's configured resource).
fn resource(c: &Config) -> String {
    c.api_url.trim_end_matches('/').to_string()
}

/// Starts a login: returns the authorize URL to open in the browser.
pub async fn begin() -> anyhow::Result<String> {
    let c = config::load();
    anyhow::ensure!(!c.auth_url.is_empty() && !c.api_url.is_empty(), "set the server's addresses first");
    let m = metadata(&c).await?;
    let (state, verifier) = (random(16), random(48));
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    *PENDING.lock().unwrap() = Some((state.clone(), verifier));
    let mut u = tauri::Url::parse(&m.authorization_endpoint)?;
    u.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", REDIRECT)
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("resource", &resource(&c))
        .append_pair("state", &state);
    Ok(u.to_string())
}

fn store(c: &mut Config, t: TokenResponse) -> anyhow::Result<()> {
    c.access_token = t.access_token;
    if let Some(r) = t.refresh_token {
        c.refresh_token = r;
    }
    c.token_expires_at = chrono::Utc::now().timestamp() + t.expires_in.unwrap_or(3600);
    config::save(c)
}

/// The browser came back to recordem://oauth?code=…&state=…: trade the code for tokens.
pub async fn finish(url: &tauri::Url) -> anyhow::Result<()> {
    let q: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
    if let Some(e) = q.get("error") {
        anyhow::bail!("sign-in was refused: {}", q.get("error_description").unwrap_or(e));
    }
    let (state, verifier) = PENDING.lock().unwrap().take().ok_or_else(|| anyhow::anyhow!("no sign-in in progress"))?;
    anyhow::ensure!(q.get("state") == Some(&state), "a sign-in answer that isn't this app's");
    let code = q.get("code").ok_or_else(|| anyhow::anyhow!("sign-in returned no code"))?;
    let mut c = config::load();
    let m = metadata(&c).await?;
    let res = resource(&c);
    let t: TokenResponse = http()
        .post(&m.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT),
            ("client_id", CLIENT_ID),
            ("code_verifier", &verifier),
            ("resource", &res),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    store(&mut c, t)
}

/// Name and email of who signed in, from the identity provider's userinfo (for the window).
pub async fn who(c: &Config) -> anyhow::Result<String> {
    #[derive(Deserialize)]
    struct Info {
        name: Option<String>,
        email: Option<String>,
    }
    let m = metadata(c).await?;
    let Some(url) = m.userinfo_endpoint else { return Ok(String::new()) };
    let i: Info = http().get(url).bearer_auth(&c.access_token).send().await?.error_for_status()?.json().await?;
    Ok(match (i.name, i.email) {
        (Some(n), Some(e)) => format!("{n} ({e})"),
        (n, e) => n.or(e).unwrap_or_default(),
    })
}

/// The Bearer for API calls: the access token, refreshed when it's about to expire.
pub async fn bearer(_c: &Config) -> anyhow::Result<String> {
    let _guard = REFRESH.lock().await;
    let mut c = config::load(); // another call may have refreshed while we waited
    anyhow::ensure!(!c.refresh_token.is_empty(), "sign in first");
    if c.token_expires_at - 120 > chrono::Utc::now().timestamp() {
        return Ok(c.access_token);
    }
    let m = metadata(&c).await?;
    let res = resource(&c);
    let r = http()
        .post(&m.token_endpoint)
        .form(&[("grant_type", "refresh_token"), ("refresh_token", c.refresh_token.as_str()), ("client_id", CLIENT_ID), ("resource", &res)])
        .send()
        .await?;
    if r.status() == reqwest::StatusCode::BAD_REQUEST || r.status() == reqwest::StatusCode::UNAUTHORIZED {
        // Refresh token expired or revoked: sign in again.
        sign_out(&mut c)?;
        anyhow::bail!("the session expired: sign in again");
    }
    let t: TokenResponse = r.error_for_status()?.json().await?;
    store(&mut c, t)?;
    Ok(c.access_token)
}

pub fn sign_out(c: &mut Config) -> anyhow::Result<()> {
    c.access_token.clear();
    c.refresh_token.clear();
    c.token_expires_at = 0;
    c.account.clear();
    config::save(c)
}
