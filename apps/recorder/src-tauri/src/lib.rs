// Recordem recorder: notices a call (a meeting app or browser starts using the mic), asks the Recordem API what the
// call is, records mic + system audio (and your own voice at 48 kHz), transcribes on this Mac and delivers the text,
// then the audio (AAC).
//
// Phases: setup → models → permissions → idle ⇄ detected → recording → processing → done | failed → idle.
// A recording that couldn't be transcribed or sent stays in recordings/<id>/ and is resumed when the app opens.
mod api;
mod apptexts;
mod asr;
mod bench;
mod callurl;
mod capture;
mod config;
mod diarize;
mod icons;
mod live;
mod models;
mod mute;
mod oauth;
mod participants;
mod permissions;
mod profiles;
mod transcribe;

use api::{Match, Transcript};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::menu::{Menu, MenuItem};
use tauri::image::Image;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State, WindowEvent};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;

/// Apps whose mic use means "probably a call": browsers (Meet, Teams and Zoom on the web) and the native clients.
const MEETING_APPS: &[(&str, Option<&str>)] = &[
    ("us.zoom.xos", Some("zoom")),
    ("com.microsoft.teams2", Some("microsoft_teams")),
    ("com.microsoft.teams", Some("microsoft_teams")),
    ("com.cisco.webexmeetingsapp", Some("webex")),
    ("com.google.Chrome", None),
    ("com.apple.Safari", None),
    ("company.thebrowser.Browser", None),
    ("com.microsoft.edgemac", None),
    ("com.brave.Browser", None),
    ("org.mozilla.firefox", None),
];

/// Menu-bar icons: a template (macOS tints it for light and dark bars) and a red one while recording.
const TRAY: &[u8] = include_bytes!("../icons/tray.png");
// A dev build records in violet (icons/dev, scripts/icons.py), and says "DEV" next to its menu-bar icon.
const TRAY_REC: &[u8] = if DEV { include_bytes!("../icons/dev/tray-rec.png") } else { include_bytes!("../icons/tray-rec.png") };

/// After the call app lets go of the mic, wait this long before stopping (a reconnect or device switch is not the end).
const STOP_GRACE_SECS: u64 = 45;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Call {
    app_id: Option<String>,
    app_name: Option<String>,
    app: Option<String>, // google_meet | zoom | microsoft_teams | … for the API
    title: Option<String>,
    url: Option<String>,
}

/// What a recording knows before it has a transcript; saved as meta.json next to the audio.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Meta {
    id: String,
    /// The meeting on the server: the one picked from the matches, or the one created with the recording.
    #[serde(default)]
    meeting_id: Option<String>,
    title: Option<String>,
    call: Call,
    started_at: String,
    ended_at: Option<String>,
    language: String, // the languages its turns may be in ("pt,en"); a single one in recordings before 0.3.0
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
enum Phase {
    Setup,
    Models { done: u64, total: u64, error: Option<String> },
    /// The required permissions, each with its status (filled in by `status`). The others are in Settings.
    Permissions { items: Vec<permissions::Item> },
    Idle,
    Detected { call: Call, matches: Option<Vec<Match>>, error: Option<String> },
    Recording { id: String, started_at: String, title: Option<String>, app_name: Option<String>, live: bool },
    Processing { id: String, progress: u32, step: String, title: Option<String> },
    Done { title: Option<String>, meeting_id: Option<String>, warning: Option<String> },
    Failed { message: String },
}

#[derive(Default)]
struct Inner {
    phase: Option<Phase>,
    stop: Option<CancellationToken>,
    recording: Option<Meta>,
    stop_timer: Option<CancellationToken>,
    pending: usize,
}

#[derive(Default)]
struct AppState {
    inner: Mutex<Inner>,
    model_bytes: Arc<AtomicU64>,
    progress: Arc<AtomicU32>,
    /// The recording's mic is silence while on (mute.rs); off when a recording starts.
    muted: Arc<AtomicBool>,
    mute_item: Mutex<Option<MenuItem<tauri::Wry>>>,
}

type Shared = Arc<AppState>;

/// The languages a transcript came out in, most spoken first ("pt,en"), for the API's `language`.
fn spoken(segments: &[api::Segment]) -> String {
    let mut n: Vec<(String, usize)> = vec![];
    for l in segments.iter().filter_map(|s| s.language.as_ref()) {
        match n.iter_mut().find(|(x, _)| x == l) {
            Some((_, c)) => *c += 1,
            None => n.push((l.clone(), 1)),
        }
    }
    n.sort_by(|a, b| b.1.cmp(&a.1));
    n.into_iter().map(|(l, _)| l).collect::<Vec<_>>().join(",")
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn set_phase(app: &AppHandle, p: Phase) {
    let s = app.state::<Shared>();
    let rec = matches!(p, Phase::Recording { .. });
    // "Record this call?" stays above the call until answered: clicking into the call would bury it, and a menu-bar
    // app has no Dock icon or Cmd-Tab entry to find it again.
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_always_on_top(matches!(p, Phase::Detected { .. }));
    }
    if !matches!(p, Phase::Detected { .. }) {
        hide_prompt(app);
    }
    let was_rec = matches!(s.inner.lock().unwrap().phase, Some(Phase::Recording { .. }));
    s.inner.lock().unwrap().phase = Some(p);
    if let Some(t) = app.tray_by_id("main") {
        let _ = t.set_icon(Image::from_bytes(if rec { TRAY_REC } else { TRAY }).ok());
        let _ = t.set_icon_as_template(!rec);
        let _ = t.set_tooltip(Some(tooltip(rec, s.muted.load(Ordering::Relaxed))));
        // While recording, a click on the icon is a menu: the live window, stop, the app. Otherwise it opens the app.
        if rec != was_rec {
            if let Ok(m) = tray_menu(app, rec) {
                let _ = t.set_menu(Some(m));
            }
            let _ = t.set_show_menu_on_left_click(rec);
        }
    }
}

fn tooltip(recording: bool, muted: bool) -> String {
    match (recording, muted) {
        (true, true) => format!("{}: recording, mic muted", config::APP),
        (true, false) => format!("{}: recording", config::APP),
        _ => config::APP.into(),
    }
}

fn mute_label(muted: bool) -> &'static str {
    if muted { "Unmute the microphone" } else { "Mute the microphone" }
}

/// Mute on or off for the recording (the toggle, or the call's own button): the tray follows.
fn set_muted(app: &AppHandle, on: bool) {
    let s = app.state::<Shared>();
    s.muted.store(on, Ordering::Relaxed);
    if let Some(item) = s.mute_item.lock().unwrap().as_ref() {
        let _ = item.set_text(mute_label(on));
    }
    let rec = matches!(s.inner.lock().unwrap().phase, Some(Phase::Recording { .. }));
    if let Some(t) = app.tray_by_id("main") {
        let _ = t.set_tooltip(Some(tooltip(rec, on)));
    }
}

fn tray_menu(app: &AppHandle, recording: bool) -> tauri::Result<Menu<tauri::Wry>> {
    if recording {
        let live = MenuItem::with_id(app, "live", "Open in Recordem", true, None::<&str>)?;
        let muted = app.state::<Shared>().muted.load(Ordering::Relaxed);
        let mute = MenuItem::with_id(app, "mute", mute_label(muted), true, None::<&str>)?;
        let stop = MenuItem::with_id(app, "stop", "Stop recording", true, None::<&str>)?;
        let open = MenuItem::with_id(app, "open", "Show the recorder", true, None::<&str>)?;
        *app.state::<Shared>().mute_item.lock().unwrap() = Some(mute.clone());
        Menu::with_items(app, &[&live, &mute, &stop, &tauri::menu::PredefinedMenuItem::separator(app)?, &open])
    } else {
        let open = MenuItem::with_id(app, "open", "Open", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
        Menu::with_items(app, &[&open, &quit])
    }
}

/// "Record this call?": a small card in the screen's top-right corner, like a notification, that doesn't take
/// the focus from the call and stays above it. Out of screen sharing too. Closed when the phase moves on.
fn show_prompt(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("prompt") {
        let _ = w.show();
        return;
    }
    const W: f64 = 360.0;
    const H: f64 = 70.0;
    // A notification's material, corners and shadow: the card is the whole window.
    let effects = tauri::utils::config::WindowEffectsConfig {
        effects: vec![tauri::window::Effect::Popover],
        state: Some(tauri::window::EffectState::Active),
        radius: Some(14.0),
        color: None,
    };
    let b = tauri::WebviewWindowBuilder::new(app, "prompt", tauri::WebviewUrl::App("prompt.html".into()))
        .title("Record this call?")
        .inner_size(W, H)
        .decorations(false)
        .transparent(true)
        .effects(effects)
        .shadow(true)
        .resizable(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .focused(false)
        .content_protected(true);
    // Under the menu bar, at the right edge of the screen the call is on (the main one). A dev build's card sits one
    // card lower, so with the release also running both stay readable.
    let b = match app.primary_monitor() {
        Ok(Some(m)) => {
            let sf = m.scale_factor();
            let (size, pos) = (m.size().to_logical::<f64>(sf), m.position().to_logical::<f64>(sf));
            let slot = if DEV { H + 10.0 } else { 0.0 };
            b.position(pos.x + size.width - W - 12.0, pos.y + 34.0 + slot)
        }
        _ => b,
    };
    if let Err(e) = b.build() {
        eprintln!("call prompt: {e}");
    }
}

/// A dev build (scripts/dev-build.sh): the windows show it, in their own colour.
const DEV: bool = option_env!("RECORDEM_APP").is_some();

#[derive(Serialize)]
struct Flavor {
    app: &'static str,
    dev: bool,
}

#[tauri::command]
fn flavor() -> Flavor {
    Flavor { app: config::APP, dev: DEV }
}

#[tauri::command]
async fn app_icon(bundle_id: String) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || icons::of(&bundle_id)).await.ok().flatten()
}

fn hide_prompt(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("prompt") {
        let _ = w.close();
    }
}

fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

/// The phase the app should be in when nothing is happening.
fn resting(cfg: &config::Config) -> Phase {
    if !cfg.signed_in() {
        Phase::Setup
    } else if !models::ready() {
        Phase::Models { done: 0, total: models::total(), error: None }
    } else if !cfg.onboarded || !permissions::ready() {
        Phase::Permissions { items: vec![] }
    } else {
        Phase::Idle
    }
}

// --- commands (called from ui/app.js) ----------------------------------------------------------------------------

#[derive(Serialize)]
struct Status {
    #[serde(flatten)]
    phase: Phase,
    pending: usize,
    muted: bool,
}

#[tauri::command]
fn status(state: State<'_, Shared>) -> Status {
    let inner = state.inner.lock().unwrap();
    let mut phase = inner.phase.clone().unwrap_or_else(|| resting(&config::load()));
    // Idle again with a required permission gone (revoked in System Settings): back to the permissions step.
    if matches!(phase, Phase::Idle) && !permissions::ready() {
        phase = Phase::Permissions { items: vec![] };
    }
    match &mut phase {
        Phase::Permissions { items } => *items = permissions::all().into_iter().filter(|i| i.required).collect(),
        Phase::Models { done, .. } => *done = state.model_bytes.load(Ordering::Relaxed),
        Phase::Processing { progress, step, .. } if step == "transcribing" => *progress = state.progress.load(Ordering::Relaxed),
        _ => {}
    }
    Status { phase, pending: inner.pending, muted: state.muted.load(Ordering::Relaxed) }
}

#[derive(Serialize)]
struct ConfigView {
    api_url: String,
    auth_url: String,
    web_url: String,
    signed_in: bool,
    account: String,
    languages: Vec<String>,
    keep_audio: bool,
    voice_hq: bool,
    launch_at_login: bool,
    on_call: String,
    mic_device: Option<String>,
    vocabulary: String,
    live: bool,
    mics: Vec<String>,
}

#[tauri::command]
fn get_config() -> ConfigView {
    let c = config::load();
    ConfigView {
        signed_in: c.signed_in(),
        api_url: c.api_url,
        auth_url: c.auth_url,
        web_url: c.web_url,
        account: c.account,
        languages: config::languages(&c.languages),
        keep_audio: c.keep_audio,
        voice_hq: c.voice_hq,
        launch_at_login: c.launch_at_login,
        on_call: c.on_call,
        mic_device: c.mic_device,
        vocabulary: c.vocabulary,
        live: c.live,
        mics: capture::mics(),
    }
}

/// Points the app at a Recordem server (addresses), forgetting the sign-in when the server changes: tokens belong to
/// one identity provider and one API.
fn set_addresses(c: &mut config::Config, api: &str, auth: &str, web: &str) {
    let clean = |s: &str| s.trim().trim_end_matches('/').to_string();
    let (api, auth) = (clean(api), clean(auth));
    if api != c.api_url || auth != c.auth_url {
        let _ = oauth::sign_out(c);
    }
    c.api_url = api;
    c.auth_url = auth;
    c.web_url = clean(web);
}

/// "Sign in": opens the identity provider's sign-in in the browser; it comes back through recordem://oauth.
async fn begin_login(app: &AppHandle) -> Result<(), String> {
    let url = oauth::begin().await.map_err(|e| format!("couldn't reach the server: {e}"))?;
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
async fn login(app: AppHandle, api_url: String, auth_url: String, web_url: String) -> Result<(), String> {
    let mut c = config::load();
    set_addresses(&mut c, &api_url, &auth_url, &web_url);
    config::save(&c).map_err(|e| e.to_string())?;
    begin_login(&app).await
}

#[tauri::command]
fn logout(app: AppHandle) -> Result<(), String> {
    let mut c = config::load();
    oauth::sign_out(&mut c).map_err(|e| e.to_string())?;
    set_phase(&app, Phase::Setup);
    Ok(())
}

/// After a sign-in: the token works against the API, and who it is (for the window); then the app is ready.
async fn signed_in(app: &AppHandle) -> Result<String, String> {
    let mut c = config::load();
    if let Err(e) = api::me(&c).await {
        let _ = oauth::sign_out(&mut c);
        return Err(format!("couldn't sign in: {e}"));
    }
    c.account = oauth::who(&c).await.unwrap_or_default();
    config::save(&c).map_err(|e| e.to_string())?;
    apply_autostart(app, c.launch_at_login);
    set_phase(app, resting(&c));
    Ok(c.account)
}

/// recordem:// links:
///   login?api=…&auth=…&web=…   a server's "Connect the recorder" button: set the addresses and sign in
///   oauth?code=…               the browser coming back from the sign-in
async fn on_link(app: AppHandle, url: tauri::Url) {
    let q: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
    show(&app);
    let result = match url.host_str() {
        Some("login") => {
            let mut c = config::load();
            if let (Some(api), Some(auth)) = (q.get("api"), q.get("auth")) {
                set_addresses(&mut c, api, auth, q.get("web").map(String::as_str).unwrap_or(api));
                let _ = config::save(&c);
            }
            begin_login(&app).await
        }
        Some("oauth") => match oauth::finish(&url).await {
            Ok(()) => signed_in(&app).await.map(|who| notify(&app, "Recordem connected", &if who.is_empty() { "Signed in.".into() } else { format!("Signed in as {who}.") })),
            Err(e) => Err(e.to_string()),
        },
        _ => Ok(()),
    };
    if let Err(e) = result {
        set_phase(&app, Phase::Failed { message: e });
    }
}

#[derive(Serialize)]
struct Version {
    current: String,
    latest: Option<String>,
    newer: bool,
}

/// This build's version. (Update checks come with published releases.)
#[tauri::command]
async fn version(app: AppHandle) -> Version {
    Version { current: app.package_info().version.to_string(), latest: None, newer: false }
}

#[derive(Deserialize)]
struct ConfigInput {
    api_url: String,
    auth_url: String,
    web_url: String,
    languages: Vec<String>,
    keep_audio: bool,
    #[serde(default = "live_default")]
    voice_hq: bool,
    launch_at_login: bool,
    on_call: String,
    mic_device: Option<String>,
    vocabulary: String,
    #[serde(default = "live_default")]
    live: bool,
}

fn live_default() -> bool {
    true
}

/// Login item on or off, to match the setting (a LaunchAgent in ~/Library/LaunchAgents).
fn apply_autostart(app: &AppHandle, on: bool) {
    let a = app.autolaunch();
    let _ = if on { a.enable() } else { a.disable() };
}

#[tauri::command]
fn save_config(app: AppHandle, input: ConfigInput) -> Result<(), String> {
    let mut c = config::load();
    set_addresses(&mut c, &input.api_url, &input.auth_url, &input.web_url);
    c.languages = config::languages(&input.languages.join(",")).join(",");
    c.keep_audio = input.keep_audio;
    c.voice_hq = input.voice_hq;
    c.launch_at_login = input.launch_at_login;
    c.on_call = match input.on_call.as_str() { "auto" | "off" => input.on_call, _ => "ask".into() };
    c.mic_device = input.mic_device.filter(|m| !m.is_empty());
    c.vocabulary = input.vocabulary;
    c.live = input.live;
    config::save(&c).map_err(|e| e.to_string())?;
    apply_autostart(&app, c.launch_at_login);
    if !c.signed_in() {
        set_phase(&app, Phase::Setup);
    }
    Ok(())
}

#[tauri::command]
async fn download_models(app: AppHandle) -> Result<(), String> {
    let st = app.state::<Shared>().inner().clone();
    st.model_bytes.store(0, Ordering::Relaxed);
    set_phase(&app, Phase::Models { done: 0, total: models::total(), error: None });
    match models::download(st.model_bytes.clone()).await {
        Ok(()) => {
            set_phase(&app, resting(&config::load()));
            Ok(())
        }
        Err(e) => {
            set_phase(&app, Phase::Models { done: 0, total: models::total(), error: Some(e.to_string()) });
            Err(e.to_string())
        }
    }
}

/// Asks macOS for one permission (its prompt, or its pane in System Settings once refused).
#[tauri::command]
async fn request_permission(id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || permissions::request(&id)).await.map_err(|e| e.to_string())
}

/// Every permission with its status, for Settings → Permissions.
#[tauri::command]
fn permission_list() -> Vec<permissions::Item> {
    permissions::all()
}

#[tauri::command]
fn open_permission(id: String) {
    permissions::open_settings(&id);
}

/// "Continue" on the permissions step (the required ones granted); also how Settings reopens the step.
#[tauri::command]
fn finish_onboarding(app: AppHandle) -> Result<(), String> {
    if !permissions::ready() {
        return Err("the microphone and system audio permissions are needed to record".into());
    }
    let mut c = config::load();
    c.onboarded = true;
    config::save(&c).map_err(|e| e.to_string())?;
    set_phase(&app, resting(&c));
    Ok(())
}

/// "Record now" without a detected call, or the detector's call: ask the API what it is.
#[tauri::command]
async fn lookup(app: AppHandle, call: Option<Call>) -> Result<(), String> {
    detected(app, call, false).await
}

/// `auto`: called by the detector (may start recording on its own), not by a click.
async fn detected(app: AppHandle, call: Option<Call>, auto: bool) -> Result<(), String> {
    let call = call.unwrap_or(Call { app_id: None, app_name: None, app: None, title: None, url: None });
    set_phase(&app, Phase::Detected { call: call.clone(), matches: None, error: None });
    let c = config::load();
    let req = api::ContextRequest { at: now(), url: call.url.as_deref(), app: call.app.as_deref(), title: call.title.as_deref() };
    let (matches, error) = match api::context(&c, &req).await {
        Ok(m) => (Some(m), None),
        Err(e) => (Some(vec![]), Some(format!("couldn't look up this call: {e}"))),
    };
    // Only fill in if the user is still looking at this call.
    let st = app.state::<Shared>();
    if !matches!(st.inner.lock().unwrap().phase, Some(Phase::Detected { .. })) {
        return Ok(());
    }
    // "auto": a detected call that is clearly one meeting (same link, or the only one of yours now) starts by itself.
    let sure = matches.as_ref().and_then(|m| match m.as_slice() {
        [one] => Some(one.clone()),
        [first, ..] if first.matched_by == "link" => Some(first.clone()),
        _ => None,
    });
    set_phase(&app, Phase::Detected { call: call.clone(), matches, error });
    if let (true, true, Some(m)) = (auto, c.on_call == "auto", sure) {
        notify(&app, "Recording", &format!("{}. Let the others know the call is being recorded.", m.title.as_deref().unwrap_or("This call")));
        return start(app, Some(m.meeting_id)).await;
    }
    Ok(())
}

#[tauri::command]
async fn start(app: AppHandle, meeting_id: Option<String>) -> Result<(), String> {
    let st = app.state::<Shared>().inner().clone();
    let (call, matches) = match st.inner.lock().unwrap().phase.clone() {
        Some(Phase::Detected { call, matches, .. }) => (call, matches.unwrap_or_default()),
        Some(Phase::Recording { .. }) => return Err("already recording".into()),
        _ => (Call { app_id: None, app_name: None, app: None, title: None, url: None }, vec![]),
    };
    let picked = meeting_id.as_ref().and_then(|id| matches.into_iter().find(|m| &m.meeting_id == id));
    let title = picked.as_ref().and_then(|m| m.title.clone()).or_else(|| call.title.as_deref().and_then(callurl::meaningful_title));
    let cfg = config::load();
    let meta = Meta {
        id: uuid::Uuid::new_v4().to_string(),
        meeting_id: picked.map(|m| m.meeting_id),
        title: title.clone(),
        call: call.clone(),
        started_at: now(),
        ended_at: None,
        language: cfg.languages.clone(),
    };
    let dir = config::recordings_dir().join(&meta.id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    write_json(&dir.join("meta.json"), &meta).map_err(|e| e.to_string())?;
    let stop = CancellationToken::new();
    st.muted.store(false, Ordering::Relaxed);
    {
        let mut inner = st.inner.lock().unwrap();
        inner.stop = Some(stop.clone());
        inner.recording = Some(meta.clone());
    }
    set_phase(&app, Phase::Recording { id: meta.id.clone(), started_at: meta.started_at.clone(), title, app_name: call.app_name, live: cfg.live });
    // The recording exists on the server from the start (the portal shows it live), and its meeting id comes back.
    // Offline, it is created later, before delivering (same id, idempotent).
    let created = {
        let (app, cfg, meta) = (app.clone(), cfg.clone(), meta.clone());
        tauri::async_runtime::spawn(async move { register(&app, &cfg, &meta).await.is_ok() })
    };
    // The live transcript: a thread transcribes as the capture goes, a task sends what it makes once the recording
    // exists on the server.
    let people = participants::Latest::default();
    let (live_tx, live) = if cfg.live {
        let (frames_tx, frames) = std::sync::mpsc::channel();
        let (segs_tx, segs) = tokio::sync::mpsc::unbounded_channel();
        let (langs, vocabulary, engine) = (cfg.languages.clone(), cfg.vocabulary.clone(), cfg.asr_live.clone());
        let worker = std::thread::spawn(move || {
            if let Err(e) = live::transcribe(frames, segs_tx, &langs, &vocabulary, &engine) {
                eprintln!("live transcript stopped: {e}");
            }
        });
        let (cfg2, id, stop2, people, dir2) = (cfg.clone(), meta.id.clone(), stop.clone(), people.clone(), dir.clone());
        let sender = tauri::async_runtime::spawn(async move {
            if created.await.unwrap_or(false) {
                live::send(cfg2, id, segs, stop2, people, dir2).await;
            }
        });
        (Some(frames_tx), Some((worker, sender)))
    } else {
        (None, None)
    };
    // Mute follows the call's own button where it can be read (Meet in a browser, the Zoom app): where it is and what
    // it says come from the app's reader profile.
    let source = call.app_id.as_deref().and_then(|id| {
        let all = profiles::current();
        let p = all.for_app(id, None).into_iter().find(|p| p.self_mic.is_some())?;
        Some(mute::Reader { mic: p.self_mic.clone()?, links: p.links.clone() })
    });
    if let (Some(source), Some(id)) = (source, call.app_id.clone()) {
        let app = app.clone();
        mute::watch(source, id, stop.clone(), move |m| set_muted(&app, m));
    }
    // Who is in the call and who speaks, from the call app's window: shown live, names the voices after the call.
    if let Some(id) = call.app_id.clone() {
        participants::watch(id, dir.clone(), std::time::Instant::now(), stop.clone(), people);
    }
    let app2 = app.clone();
    let muted = st.muted.clone();
    tauri::async_runtime::spawn(async move {
        // The other side's voices are told apart as the call goes; the pass after it only clusters them.
        let (voices_tx, voices_rx) = std::sync::mpsc::channel();
        let vdir = dir.clone();
        let voices = std::thread::spawn(move || diarize::during(voices_rx, &vdir));
        let result = capture::record(&dir, cfg.mic_device.clone(), cfg.voice_hq, stop, live_tx, voices_tx, muted).await;
        let _ = tauri::async_runtime::spawn_blocking(move || voices.join()).await;
        // The capture dropped its end: the live thread finishes the turn it is on and stops, before the pass after
        // the call needs Whisper. Its sender gets one last try.
        if let Some((worker, sender)) = live {
            let _ = tauri::async_runtime::spawn_blocking(move || worker.join()).await;
            let _ = sender.await;
        }
        // meta.json again from disk: the meeting id may have arrived while recording.
        let mut meta: Meta = read_json(&dir.join("meta.json")).unwrap_or(meta);
        meta.ended_at = Some(now());
        let _ = write_json(&dir.join("meta.json"), &meta);
        {
            let st = app2.state::<Shared>();
            let mut inner = st.inner.lock().unwrap();
            inner.stop = None;
            inner.recording = None;
            st.muted.store(false, Ordering::Relaxed);
            if let Some(t) = inner.stop_timer.take() {
                t.cancel();
            }
        }
        let warning = match &result {
            Ok((m, s)) if *m < 0.001 && *s < 0.001 => Some("No sound was captured. Check the Microphone and System Audio Recording permissions in System Settings → Privacy & Security.".to_string()),
            Ok((m, _)) if *m < 0.001 => Some("The microphone captured nothing. Check the Microphone permission.".to_string()),
            Ok((_, s)) if *s < 0.001 => Some("The call's audio wasn't captured. Check the System Audio Recording permission.".to_string()),
            _ => None,
        };
        if let Err(e) = result {
            set_phase(&app2, Phase::Failed { message: e.to_string() });
            show(&app2);
            return;
        }
        process(&app2, dir, warning).await;
    });
    Ok(())
}

/// Creates the recording on the server (idempotent by id) and keeps the meeting id it answers with: in memory for the
/// running recording, and in meta.json.
async fn register(app: &AppHandle, cfg: &config::Config, meta: &Meta) -> anyhow::Result<String> {
    let r = api::create(cfg, &api::NewRecording {
        id: &meta.id,
        meeting_id: meta.meeting_id.as_deref(),
        started_at: &meta.started_at,
        url: meta.call.url.as_deref(),
        app: meta.call.app.as_deref(),
        title: meta.title.as_deref(),
    })
    .await?;
    let dir = config::recordings_dir().join(&meta.id);
    if let Ok(mut m) = read_json::<Meta>(&dir.join("meta.json")) {
        m.meeting_id = Some(r.meeting_id.clone());
        let _ = write_json(&dir.join("meta.json"), &m);
    }
    if let Some(rec) = app.state::<Shared>().inner.lock().unwrap().recording.as_mut() {
        if rec.id == meta.id {
            rec.meeting_id = Some(r.meeting_id.clone());
        }
    }
    Ok(r.meeting_id)
}

/// The meeting being recorded, in the portal (its transcript grows live there).
#[tauri::command]
async fn open_live(app: AppHandle) -> Result<(), String> {
    let id = match app.state::<Shared>().inner.lock().unwrap().recording.as_ref() {
        Some(m) => m.meeting_id.clone().ok_or("the recording isn't on the server yet")?,
        None => return Err("nothing is being recorded".into()),
    };
    open(app, id)
}

#[tauri::command]
fn toggle_mute(app: AppHandle) {
    let on = !app.state::<Shared>().muted.load(Ordering::Relaxed);
    set_muted(&app, on);
}

#[tauri::command]
fn stop(state: State<'_, Shared>) {
    if let Some(t) = state.inner.lock().unwrap().stop.take() {
        t.cancel();
    }
}

/// From the call prompt: the main window, for the choices the prompt doesn't have.
#[tauri::command]
fn show_main(app: AppHandle) {
    show(&app);
}

#[tauri::command]
fn dismiss(app: AppHandle) {
    set_phase(&app, resting(&config::load()));
}

/// A meeting's page in the portal.
#[tauri::command]
fn open(app: AppHandle, meeting_id: String) -> Result<(), String> {
    let url = api::meeting_url(&config::load(), &meeting_id);
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

// --- after a recording ------------------------------------------------------------------------------------------

fn write_json<T: Serialize>(p: &std::path::Path, v: &T) -> anyhow::Result<()> {
    std::fs::write(p, serde_json::to_vec_pretty(v)?)?;
    Ok(())
}

fn read_json<T: for<'a> Deserialize<'a>>(p: &std::path::Path) -> anyhow::Result<T> {
    Ok(serde_json::from_slice(&std::fs::read(p)?)?)
}

/// Transcribe (unless already done) and deliver one recording directory.
async fn process(app: &AppHandle, dir: PathBuf, warning: Option<String>) {
    let meta: Meta = match read_json(&dir.join("meta.json")) {
        Ok(m) => m,
        Err(e) => return set_phase(app, Phase::Failed { message: format!("unreadable recording: {e}") }),
    };
    let st = app.state::<Shared>().inner().clone();
    let tpath = dir.join("transcript.json");
    let transcript: Transcript = match read_json(&tpath) {
        Ok(t) => t,
        Err(_) => {
            st.progress.store(0, Ordering::Relaxed);
            set_phase(app, Phase::Processing { id: meta.id.clone(), progress: 0, step: "transcribing".into(), title: meta.title.clone() });
            let (d, lang, progress) = (dir.clone(), meta.language.clone(), st.progress.clone());
            let c = config::load();
            let (vocabulary, engine) = (c.vocabulary, c.asr_final);
            let model = format!("{} + silero vad + pyannote", asr::describe(&engine));
            let segs = tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<_> {
                let f = capture::files(&d);
                transcribe::run(&capture::read(&f.mic)?, &capture::read(&f.system)?, diarize::found(&d), &lang, &vocabulary, &engine, &progress)
            })
            .await;
            let segments = match segs {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => return set_phase(app, Phase::Failed { message: format!("transcription failed: {e}") }),
                Err(e) => return set_phase(app, Phase::Failed { message: format!("transcription failed: {e}") }),
            };
            // Names for the voices, from who the call app showed speaking.
            let speakers = participants::names(&participants::name_voices(&participants::read(&dir), &segments));
            let t = Transcript {
                ended_at: meta.ended_at.clone().unwrap_or_else(now),
                language: Some(spoken(&segments)),
                model: Some(model),
                segments,
                speakers,
            };
            if let Err(e) = write_json(&tpath, &t) {
                return set_phase(app, Phase::Failed { message: e.to_string() });
            }
            t
        }
    };
    set_phase(app, Phase::Processing { id: meta.id.clone(), progress: 1000, step: "sending".into(), title: meta.title.clone() });
    let cfg = config::load();
    // The recording on the server first (if it never got there), then the text (the part that matters), then the
    // audio: the whole call, and your own voice in high quality. All idempotent, so a retry resends everything.
    let sent = async {
        let meeting_id = register(app, &cfg, &meta).await?;
        api::deliver(&cfg, &meta.id, &transcript).await?;
        let d = dir.clone();
        let (mix, voice) = tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<_> { Ok((capture::call_m4a(&d)?, capture::voice_m4a(&d)?)) }).await??;
        api::upload_track(&cfg, &meta.id, "mix", &mix).await?;
        if let Some(v) = voice {
            api::upload_track(&cfg, &meta.id, "mic_hq", &v).await?;
        }
        anyhow::Ok(meeting_id)
    }
    .await;
    match sent {
        Ok(meeting_id) => {
            let _ = std::fs::write(dir.join("sent"), now());
            if !cfg.keep_audio {
                let f = capture::files(&dir);
                for p in [f.mic, f.system, f.mic_hq, dir.join("call.m4a"), dir.join("voice.m4a")] {
                    let _ = std::fs::remove_file(p);
                }
            }
            refresh_pending(app);
            notify(app, "Transcript sent", meta.title.as_deref().unwrap_or("Untitled meeting"));
            let quiet = warning.is_none();
            set_phase(app, Phase::Done { title: meta.title, meeting_id: Some(meeting_id), warning });
            if quiet {
                return;
            }
        }
        Err(e) => {
            refresh_pending(app);
            set_phase(app, Phase::Failed { message: format!("couldn't send it ({e}). It stays on this Mac and I'll try again when the app opens.") });
        }
    }
    show(app);
}

/// Recordings not sent yet (no `sent` marker).
fn unsent() -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(config::recordings_dir()) else { return vec![] };
    let mut v: Vec<PathBuf> =
        rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.join("meta.json").exists() && !p.join("sent").exists()).collect();
    v.sort();
    v
}

fn refresh_pending(app: &AppHandle) {
    app.state::<Shared>().inner.lock().unwrap().pending = unsent().len();
}

/// At start-up: finish whatever an earlier run left behind (quit mid-transcription, offline, …).
async fn resume(app: AppHandle) {
    refresh_pending(&app);
    let cfg = config::load();
    if !matches!(resting(&cfg), Phase::Idle) {
        return;
    }
    for dir in unsent() {
        let meta: Option<Meta> = read_json(&dir.join("meta.json")).ok();
        // A recording without an end was cut short by a crash: its audio is still usable up to where it stopped,
        // but hound leaves the header unfinished; skip it rather than send half a file silently.
        if meta.as_ref().map(|m| m.ended_at.is_none()).unwrap_or(true) {
            continue;
        }
        process(&app, dir, None).await;
    }
    set_phase(&app, resting(&cfg));
}

// --- call detection --------------------------------------------------------------------------------------------

fn meeting_app(id: &str) -> Option<Option<&'static str>> {
    MEETING_APPS.iter().find(|(a, _)| *a == id).map(|(_, kind)| *kind)
}

/// Window title and platform from the Accessibility tree (needs that permission; without it we go by time only).
fn inspect(app_id: &str) -> (Option<String>, Option<String>) {
    for i in anlg_detect::inspect_meeting_accessibility() {
        if i.app.id == app_id {
            let platform = match i.platform {
                anlg_detect::MeetingPlatform::GoogleMeet => Some("google_meet"),
                anlg_detect::MeetingPlatform::Zoom => Some("zoom"),
                anlg_detect::MeetingPlatform::MicrosoftTeams => Some("microsoft_teams"),
                anlg_detect::MeetingPlatform::Webex => Some("webex"),
                _ => None,
            };
            return (i.window_title, platform.map(String::from));
        }
    }
    (None, None)
}

fn on_detect(app: &AppHandle, ev: anlg_detect::DetectEvent) {
    match ev {
        anlg_detect::DetectEvent::MicStarted(apps) => {
            let st = app.state::<Shared>();
            // The call app picked the mic up again: don't stop.
            {
                let inner = st.inner.lock().unwrap();
                if let (Some(rec), Some(timer)) = (&inner.recording, &inner.stop_timer) {
                    if apps.iter().any(|a| Some(&a.id) == rec.call.app_id.as_ref()) {
                        timer.cancel();
                    }
                }
            }
            if !matches!(st.inner.lock().unwrap().phase, None | Some(Phase::Idle) | Some(Phase::Done { .. })) {
                return;
            }
            let Some((a, kind)) = apps.iter().find_map(|a| meeting_app(&a.id).map(|k| (a.clone(), k))) else { return };
            let cfg = config::load();
            if cfg.on_call == "off" || !matches!(resting(&cfg), Phase::Idle) {
                return;
            }
            let (title, platform) = inspect(&a.id);
            let mut call = Call { app_id: Some(a.id.clone()), app_name: Some(a.name.clone()), app: platform.or(kind.map(String::from)), title, url: None };
            show_prompt(app);
            let app2 = app.clone();
            let browser = kind.is_none();
            tauri::async_runtime::spawn(async move {
                // In a browser, the meeting tab has the call's whole link (and sometimes its name).
                if browser {
                    let id = a.id.clone();
                    if let Ok(Some((url, tab))) = tauri::async_runtime::spawn_blocking(move || callurl::find(&id)).await {
                        call.app = call.app.or_else(|| callurl::platform(&a.id, &url).filter(|p| !p.is_empty()));
                        call.title = call.title.or(Some(tab));
                        call.url = Some(url);
                    }
                }
                let _ = detected(app2, Some(call), true).await;
            });
        }
        anlg_detect::DetectEvent::MicStopped(apps) => {
            let st = app.state::<Shared>().inner().clone();
            let mut inner = st.inner.lock().unwrap();
            if let Some(Phase::Detected { call, .. }) = &inner.phase {
                let gone = apps.iter().any(|a| Some(&a.id) == call.app_id.as_ref());
                if gone {
                    inner.phase = Some(Phase::Idle);
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.set_always_on_top(false);
                    }
                    hide_prompt(app);
                }
                return;
            }
            let Some(rec) = &inner.recording else { return };
            if !apps.iter().any(|a| Some(&a.id) == rec.call.app_id.as_ref()) {
                return;
            }
            let timer = CancellationToken::new();
            inner.stop_timer = Some(timer.clone());
            let stop = inner.stop.clone();
            tauri::async_runtime::spawn(async move {
                tokio::select! {
                    _ = timer.cancelled() => {}
                    _ = tokio::time::sleep(std::time::Duration::from_secs(STOP_GRACE_SECS)) => {
                        if let Some(s) = stop { s.cancel(); }
                    }
                }
            });
        }
    }
}

// --- app ----------------------------------------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .manage(Shared::default())
        .setup(|app| {
            let handle = app.handle().clone();
            let menu = tray_menu(app.handle(), false)?;
            // Lives in the menu bar only (no Dock icon): click opens the window, right-click shows the menu.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            TrayIconBuilder::with_id("main")
                .icon(Image::from_bytes(TRAY)?)
                .icon_as_template(true)
                .tooltip(config::APP)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                        let app = tray.app_handle();
                        match app.get_webview_window("main") {
                            // Hide only a window you're looking at: one behind the call comes to the front.
                            Some(w) if w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) => { let _ = w.hide(); }
                            _ => show(app),
                        }
                    }
                })
                .on_menu_event(|app, ev| match ev.id.as_ref() {
                    "open" => show(app),
                    "live" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) = open_live(app.clone()).await {
                                notify(&app, "Recordem", &e);
                            }
                        });
                    }
                    "mute" => toggle_mute(app.clone()),
                    "stop" => stop(app.state::<Shared>()),
                    "quit" => {
                        // Never drop a recording on the floor: stop it and let it finish before quitting.
                        let recording = app.state::<Shared>().inner.lock().unwrap().recording.is_some();
                        if recording {
                            stop(app.state::<Shared>());
                            show(app);
                        } else {
                            app.exit(0);
                        }
                    }
                    _ => {}
                })
                .build(app)?;
            if DEV && let Some(t) = app.tray_by_id("main") {
                let _ = t.set_title(Some("DEV"));
            }

            // The detector lives as long as the app; it runs its own Core Audio listener thread.
            let detector: &'static mut anlg_detect::Detector = Box::leak(Box::default());
            let h = handle.clone();
            detector.start(anlg_detect::new_callback(move |ev| on_detect(&h, ev)));

            // Login item follows the setting (on by default), also on the very first run.
            let cfg = config::load();
            if app.autolaunch().is_enabled().unwrap_or(false) != cfg.launch_at_login {
                apply_autostart(&handle, cfg.launch_at_login);
            }
            if !matches!(resting(&cfg), Phase::Idle) || !unsent().is_empty() {
                show(&handle);
            }
            // A recordem:// link, whether it launched the app or arrived while it runs.
            let h = handle.clone();
            app.deep_link().on_open_url(move |ev| {
                for url in ev.urls() {
                    tauri::async_runtime::spawn(on_link(h.clone(), url));
                }
            });
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                for url in urls {
                    tauri::async_runtime::spawn(on_link(handle.clone(), url));
                }
            }
            tauri::async_runtime::spawn(async {
                loop {
                    let c = config::load();
                    if !c.refresh_token.is_empty() {
                        let _ = oauth::bearer(&c).await;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
                }
            });
            tauri::async_runtime::spawn(resume(handle));
            tauri::async_runtime::spawn(profiles::keep_fresh());
            Ok(())
        })
        .on_window_event(|w, e| {
            // Closing the main window hides it; the app keeps listening from the menu bar. The live window closes.
            if let (WindowEvent::CloseRequested { api, .. }, "main") = (e, w.label()) {
                api.prevent_close();
                let _ = w.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![status, flavor, app_icon, get_config, save_config, login, logout, download_models, lookup, start, stop, toggle_mute, dismiss, show_main, open, open_live, version, request_permission, open_permission, permission_list, finish_onboarding])
        .run(tauri::generate_context!())
        .expect("error while running the recorder");
}

/// The asr-bench tool (src/bin/asr-bench.rs): engines compared on the same audio.
pub fn asr_bench(args: Vec<String>) -> anyhow::Result<()> {
    bench::main(args)
}
