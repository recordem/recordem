// How the engine works, as the host set it (`init`, then `configure`): the server, where to keep things, languages
// and engines. Under the data dir:
//   models/                Whisper, VAD and Parakeet models (downloaded once)
//   recordings/<id>/       one call: mic.wav, system.wav, mic_hq.wav, call.m4a, voice.m4a, meta.json, transcript.json;
//                          audio deleted once sent unless `keepAudio`
//   profiles.json          the server's reader profiles, with profiles.etag
use recordem_desktop::Settings;
use std::path::PathBuf;
use std::sync::RwLock;

#[derive(Debug, Clone)]
pub struct Config {
    /// The Recordem API the recordings go to; empty until `init`.
    pub api_url: String,
    pub data_dir: PathBuf,
    /// The languages calls may be in, comma-separated ("pt,en"): each turn is transcribed in the one it sounds like.
    pub languages: String,
    /// Extra words for Whisper (one per line).
    pub vocabulary: String,
    /// Speech recognition engine for the live draft and for the pass after the call (asr.rs): "whisper", "parakeet",
    /// "parakeet:ultra".
    pub asr_live: String,
    pub asr_final: String,
    /// Transcribe while the call goes on too, not only after it.
    pub live: bool,
    /// Input device by name; None = the system default.
    pub mic_device: Option<String>,
    /// Also record the mic at 48 kHz, without echo cancellation, as its own track.
    pub voice_hq: bool,
    /// Keep the audio after it is sent.
    pub keep_audio: bool,
    /// Send what the call app's window looked like when reading it broke (dumps.rs).
    pub share_reader_dumps: bool,
    /// The person on this computer, when the call app doesn't say (participants.rs).
    pub account: String,
    /// Watch for calls.
    pub detect: bool,
}

/// English and Portuguese unless the host picks otherwise.
const DEFAULT_LANGUAGES: &str = "en,pt";

impl Default for Config {
    fn default() -> Self {
        Config {
            api_url: String::new(),
            data_dir: dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("recordem-desktop"),
            languages: DEFAULT_LANGUAGES.into(),
            vocabulary: String::new(),
            asr_live: "whisper".into(),
            asr_final: "whisper".into(),
            live: true,
            mic_device: None,
            voice_hq: true,
            keep_audio: false,
            share_reader_dumps: false,
            account: String::new(),
            detect: true,
        }
    }
}

static CURRENT: RwLock<Option<Config>> = RwLock::new(None);

/// The settings now.
pub fn load() -> Config {
    CURRENT.read().unwrap().clone().unwrap_or_default()
}

/// Takes what `s` sets, keeps the rest.
pub fn apply(s: &Settings) {
    let mut c = load();
    if let Some(v) = &s.api_url {
        c.api_url = v.trim().trim_end_matches('/').to_string();
    }
    if let Some(v) = &s.data_dir {
        c.data_dir = PathBuf::from(v);
    }
    if let Some(v) = &s.languages {
        c.languages = languages(v).join(",");
    }
    if let Some(v) = &s.vocabulary {
        c.vocabulary = v.clone();
    }
    if let Some(v) = &s.asr_live {
        c.asr_live = v.clone();
    }
    if let Some(v) = &s.asr_final {
        c.asr_final = v.clone();
    }
    if let Some(v) = s.live {
        c.live = v;
    }
    if let Some(v) = &s.mic_device {
        c.mic_device = Some(v.clone()).filter(|m| !m.is_empty());
    }
    if let Some(v) = s.voice_hq {
        c.voice_hq = v;
    }
    if let Some(v) = s.keep_audio {
        c.keep_audio = v;
    }
    if let Some(v) = s.share_reader_dumps {
        c.share_reader_dumps = v;
    }
    if let Some(v) = &s.self_name {
        c.account = v.clone();
    }
    if let Some(v) = s.detect {
        c.detect = v;
    }
    *CURRENT.write().unwrap() = Some(c);
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

pub fn root() -> PathBuf {
    load().data_dir
}

pub fn recordings_dir() -> PathBuf {
    root().join("recordings")
}

pub fn models_dir() -> PathBuf {
    root().join("models")
}
