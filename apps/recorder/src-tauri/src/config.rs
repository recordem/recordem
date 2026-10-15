// Where the recorder keeps its state: ~/Library/Application Support/Recordem/
//   config.json            the server's addresses, the sign-in tokens, languages, call behaviour, mic, vocabulary
//   models/                Whisper and VAD models (downloaded once)
//   recordings/<id>/       one call: mic.wav, system.wav, mic_hq.wav, call.m4a, voice.m4a, meta.json, transcript.json;
//                          audio deleted once sent unless "keep a copy" is on
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// The Recordem API (resource server); also the audience of the tokens.
    #[serde(default = "default_api")]
    pub api_url: String,
    /// The identity provider that signs people in (OAuth 2.1).
    #[serde(default = "default_auth")]
    pub auth_url: String,
    /// The portal, for links people open (a meeting's page).
    #[serde(default = "default_web")]
    pub web_url: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub token_expires_at: i64,
    /// Who is signed in, for the window.
    #[serde(default)]
    pub account: String,
    /// The languages calls may be in, comma-separated ("pt,en"): each turn is transcribed in the one it sounds like.
    #[serde(default = "default_languages")]
    pub languages: String,
    /// Keep a local copy of the audio after it is sent (off: the server's copy is the only one).
    #[serde(default)]
    pub keep_audio: bool,
    /// Also record your own microphone at 48 kHz, without echo cancellation, as its own track: to hear yourself
    /// (pronunciation, pace) months later. The call's own mix stays 16 kHz.
    #[serde(default = "yes")]
    pub voice_hq: bool,
    #[serde(default = "yes")]
    pub launch_at_login: bool,
    /// When a call starts: ask (default), auto (record right away when it matches a meeting), off.
    #[serde(default = "default_on_call")]
    pub on_call: String,
    /// Input device by name; None = the system default.
    #[serde(default)]
    pub mic_device: Option<String>,
    /// Transcribe while the call goes on too (the live transcript in the portal), not only after it.
    #[serde(default = "yes")]
    pub live: bool,
    /// Extra words for Whisper (one per line).
    #[serde(default)]
    pub vocabulary: String,
}

/// Where the server lives; a build for a deployment sets RECORDEM_API_URL / RECORDEM_AUTH_URL / RECORDEM_WEB_URL.
fn default_api() -> String {
    option_env!("RECORDEM_API_URL").unwrap_or("https://api.recordem.dev").into()
}

fn default_auth() -> String {
    option_env!("RECORDEM_AUTH_URL").unwrap_or("https://auth.recordem.dev").into()
}

fn default_web() -> String {
    option_env!("RECORDEM_WEB_URL").unwrap_or("https://app.recordem.dev").into()
}

/// English and Portuguese unless the person picks otherwise.
const DEFAULT_LANGUAGES: &str = "en,pt";

fn default_languages() -> String {
    DEFAULT_LANGUAGES.into()
}

/// "pt,en" → ["pt", "en"]; nothing usable → the default pair. Codes are Whisper's (pt, en, es, …).
pub fn languages(s: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for l in s.split(',').map(|l| l.trim().to_lowercase()) {
        if whisper_rs::get_lang_id(&l).is_some() && !out.contains(&l) {
            out.push(l);
        }
    }
    if out.is_empty() { languages(DEFAULT_LANGUAGES) } else { out }
}

fn default_on_call() -> String {
    "ask".into()
}

fn yes() -> bool {
    true
}

pub fn root() -> PathBuf {
    dirs::data_dir().expect("no data dir").join("Recordem")
}

pub fn recordings_dir() -> PathBuf {
    root().join("recordings")
}

pub fn models_dir() -> PathBuf {
    root().join("models")
}

pub fn load() -> Config {
    std::fs::read(root().join("config.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| serde_json::from_str("{}").expect("defaults"))
}

impl Config {
    pub fn signed_in(&self) -> bool {
        !self.api_url.is_empty() && !self.refresh_token.is_empty()
    }
}

pub fn save(c: &Config) -> anyhow::Result<()> {
    std::fs::create_dir_all(root())?;
    let path = root().join("config.json");
    std::fs::write(&path, serde_json::to_vec_pretty(c)?)?;
    // The tokens are personal credentials: readable by this user only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
