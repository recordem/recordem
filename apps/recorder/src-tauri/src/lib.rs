// Recordem recorder: the menu-bar app around the Recordem engine. The engine (recordem-desktop, a sidecar, driven
// through the Desktop SDK's client like any other app would) notices calls, records, transcribes and sends; the app asks
// the API what a call is, asks you whether to record it, creates the recording with an upload token for the engine, and
// shows how it goes.
//
// Phases: setup → models → permissions → idle ⇄ detected → recording → processing → done | failed → idle.
// A recording that couldn't be sent stays with the engine and is resumed when the app opens; one whose upload token is
// missing or expired gets a fresh one here.
mod api;
mod config;
mod icons;
mod oauth;

use api::Match;
use recordem_desktop::{Client, Event, Permission, PermissionItem, PermissionStatus, Settings, StartRecording, UploadStatus, Window};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State, WindowEvent};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

/// Menu-bar icons: a template (macOS tints it for light and dark bars) and a red one while recording.
const TRAY: &[u8] = include_bytes!("../icons/tray.png");
// A dev build records in violet (icons/dev, scripts/icons.py), and says "DEV" next to its menu-bar icon.
const TRAY_REC: &[u8] = if DEV { include_bytes!("../icons/dev/tray-rec.png") } else { include_bytes!("../icons/tray-rec.png") };

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Call {
    app_id: Option<String>,
    app_name: Option<String>,
    app: Option<String>, // google_meet | zoom | microsoft_teams | … for the API
    title: Option<String>,
    url: Option<String>,
    /// The engine's window for this call; none for "record now".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_id: Option<String>,
}

impl Call {
    fn of(w: &Window) -> Call {
        Call { app_id: w.app_id.clone(), app_name: w.app_name.clone(), app: w.platform.clone(), title: w.title.clone(), url: w.url.clone(), window_id: Some(w.id.clone()) }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
enum Phase {
    Setup,
    Models { done: u64, total: u64, error: Option<String> },
    /// The required permissions, each with its status (filled in by `status`). The others are in Settings.
    Permissions { items: Vec<Item> },
    Idle,
    Detected { call: Call, matches: Option<Vec<Match>>, error: Option<String> },
    Recording { id: String, started_at: String, title: Option<String>, app_name: Option<String>, live: bool },
    Processing { id: String, progress: u32, step: String, title: Option<String> },
    Done { title: Option<String>, meeting_id: Option<String>, warning: Option<String> },
    Failed { message: String },
}

/// A permission as the window shows it.
#[derive(Debug, Clone, Serialize)]
struct Item {
    id: &'static str,
    status: &'static str, // granted | denied | not_asked | unknown
    required: bool,
}

fn item(p: &PermissionItem) -> Item {
    let id = match p.permission {
        Permission::Microphone => "microphone",
        Permission::SystemAudio => "system_audio",
        Permission::Accessibility => "accessibility",
        Permission::Automation => "automation",
    };
    let status = match p.status {
        PermissionStatus::Granted => "granted",
        PermissionStatus::Denied => "denied",
        PermissionStatus::NotRequested => "not_asked",
        PermissionStatus::Unknown => "unknown",
    };
    Item { id, status, required: p.required }
}

fn permission(id: &str) -> Option<Permission> {
    Some(match id {
        "microphone" => Permission::Microphone,
        "system_audio" => Permission::SystemAudio,
        "accessibility" => Permission::Accessibility,
        "automation" => Permission::Automation,
        _ => return None,
    })
}

/// What the app knows of a recording, for the server (creating it) and the window (its name, its meeting).
#[derive(Debug, Clone, Default)]
struct Known {
    started_at: String,
    call: Call,
    meeting_id: Option<String>,
    title: Option<String>,
    warning: Option<String>,
}

/// The recording going on.
#[derive(Debug, Clone)]
struct Recorded {
    id: String,
    window_id: String,
}

#[derive(Default)]
struct Inner {
    phase: Option<Phase>,
    recording: Option<Recorded>,
    known: HashMap<String, Known>,
    pending: usize,
}

#[derive(Default)]
struct AppState {
    inner: Mutex<Inner>,
    engine: OnceLock<Client>,
    model_bytes: AtomicU64,
    models_ready: AtomicBool,
    perms: Mutex<Vec<PermissionItem>>,
    /// The recording's mic is silence while on (the toggle, or the call's own button); off when a recording starts.
    muted: Arc<AtomicBool>,
    mute_item: Mutex<Option<MenuItem<tauri::Wry>>>,
}

type Shared = Arc<AppState>;

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn engine(app: &AppHandle) -> Result<Client, String> {
    app.state::<Shared>().engine.get().cloned().ok_or_else(|| "the recording engine isn't running".to_string())
}

/// The total size of the speech models (Whisper large-v3-turbo q5_0 + Silero VAD), for the download bar until the
/// engine reports its own.
const MODELS_TOTAL: u64 = 574_041_195 + 885_098;

/// The engine's settings, from the app's.
fn settings(c: &config::Config) -> Settings {
    Settings {
        api_url: Some(c.api_url.clone()),
        data_dir: Some(config::root().to_string_lossy().to_string()),
        languages: Some(c.languages.clone()),
        vocabulary: Some(c.vocabulary.clone()),
        asr_live: Some(c.asr_live.clone()),
        asr_final: Some(c.asr_final.clone()),
        live: Some(c.live),
        mic_device: Some(c.mic_device.clone().unwrap_or_default()),
        voice_hq: Some(c.voice_hq),
        keep_audio: Some(c.keep_audio),
        share_reader_dumps: Some(c.share_reader_dumps),
        self_name: Some(c.account.clone()),
        detect: Some(true),
    }
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

/// The recording's mute changed (the toggle, or the call's own button, as the engine tells): the tray follows.
fn show_muted(app: &AppHandle, on: bool) {
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

/// The permissions as the engine last said; asked of it when it hasn't yet.
fn permissions(app: &AppHandle) -> Vec<PermissionItem> {
    let st = app.state::<Shared>();
    let cached = st.perms.lock().unwrap().clone();
    if cached.len() == 4 {
        return cached;
    }
    let fresh = engine(app).ok().and_then(|e| e.permissions().ok()).unwrap_or_default();
    *st.perms.lock().unwrap() = fresh.clone();
    fresh
}

fn permissions_ready(app: &AppHandle) -> bool {
    let all = permissions(app);
    !all.is_empty() && all.iter().filter(|p| p.required).all(|p| p.status == PermissionStatus::Granted)
}

/// The phase the app should be in when nothing is happening.
fn resting(app: &AppHandle, cfg: &config::Config) -> Phase {
    if !cfg.signed_in() {
        Phase::Setup
    } else if !app.state::<Shared>().models_ready.load(Ordering::Relaxed) {
        Phase::Models { done: 0, total: MODELS_TOTAL, error: None }
    } else if !cfg.onboarded || !permissions_ready(app) {
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
fn status(app: AppHandle, state: State<'_, Shared>) -> Status {
    let current = state.inner.lock().unwrap().phase.clone();
    let mut phase = current.unwrap_or_else(|| resting(&app, &config::load()));
    // Idle again with a required permission gone (revoked in System Settings): back to the permissions step.
    if matches!(phase, Phase::Idle) && !permissions_ready(&app) {
        phase = Phase::Permissions { items: vec![] };
    }
    match &mut phase {
        Phase::Permissions { items } => *items = permissions(&app).iter().filter(|i| i.required).map(item).collect(),
        Phase::Models { done, .. } => *done = state.model_bytes.load(Ordering::Relaxed),
        _ => {}
    }
    let pending = state.inner.lock().unwrap().pending;
    Status { phase, pending, muted: state.muted.load(Ordering::Relaxed) }
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
    share_reader_dumps: bool,
    on_call: String,
    mic_device: Option<String>,
    vocabulary: String,
    live: bool,
    mics: Vec<String>,
}

#[tauri::command]
fn get_config(app: AppHandle) -> ConfigView {
    let c = config::load();
    ConfigView {
        signed_in: c.signed_in(),
        api_url: c.api_url,
        auth_url: c.auth_url,
        web_url: c.web_url,
        account: c.account,
        languages: config::languages(&c.languages),
        keep_audio: c.keep_audio,
        share_reader_dumps: c.share_reader_dumps,
        voice_hq: c.voice_hq,
        launch_at_login: c.launch_at_login,
        on_call: c.on_call,
        mic_device: c.mic_device,
        vocabulary: c.vocabulary,
        live: c.live,
        mics: engine(&app).ok().and_then(|e| e.list_devices().ok()).unwrap_or_default(),
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

/// The engine follows the app's settings (server, languages, mic…).
fn configure_engine(app: &AppHandle, c: &config::Config) {
    if let Ok(e) = engine(app)
        && let Err(err) = e.configure(&settings(c))
    {
        eprintln!("engine settings: {err}");
    }
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
    configure_engine(&app, &c);
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
    configure_engine(app, &c);
    apply_autostart(app, c.launch_at_login);
    set_phase(app, resting(app, &c));
    resume(app.clone()).await;
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
                configure_engine(&app, &c);
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
    #[serde(default)]
    share_reader_dumps: bool,
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
    c.share_reader_dumps = input.share_reader_dumps;
    c.voice_hq = input.voice_hq;
    c.launch_at_login = input.launch_at_login;
    c.on_call = match input.on_call.as_str() { "auto" | "off" => input.on_call, _ => "ask".into() };
    c.mic_device = input.mic_device.filter(|m| !m.is_empty());
    c.vocabulary = input.vocabulary;
    c.live = input.live;
    config::save(&c).map_err(|e| e.to_string())?;
    configure_engine(&app, &c);
    apply_autostart(&app, c.launch_at_login);
    if !c.signed_in() {
        set_phase(&app, Phase::Setup);
    }
    Ok(())
}

#[tauri::command]
async fn download_models(app: AppHandle) -> Result<(), String> {
    app.state::<Shared>().model_bytes.store(0, Ordering::Relaxed);
    set_phase(&app, Phase::Models { done: 0, total: MODELS_TOTAL, error: None });
    // Progress, and the end, come as model-download events.
    engine(&app)?.download_models().map_err(|e| e.to_string())
}

/// Asks macOS for one permission (its prompt, or its pane in System Settings once refused).
#[tauri::command]
async fn request_permission(app: AppHandle, id: String) -> Result<(), String> {
    let p = permission(&id).ok_or("no such permission")?;
    let e = engine(&app)?;
    tauri::async_runtime::spawn_blocking(move || e.request_permission(p)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    // The window asks right after: the answer, not the cache.
    *app.state::<Shared>().perms.lock().unwrap() = vec![];
    Ok(())
}

/// Every permission with its status, for Settings → Permissions.
#[tauri::command]
fn permission_list(app: AppHandle) -> Vec<Item> {
    permissions(&app).iter().map(item).collect()
}

#[tauri::command]
fn open_permission(app: AppHandle, id: String) {
    if let (Ok(e), Some(p)) = (engine(&app), permission(&id)) {
        let _ = e.open_permission_settings(p);
    }
}

/// "Continue" on the permissions step (the required ones granted); also how Settings reopens the step.
#[tauri::command]
fn finish_onboarding(app: AppHandle) -> Result<(), String> {
    *app.state::<Shared>().perms.lock().unwrap() = vec![];
    if !permissions_ready(&app) {
        return Err("the microphone and system audio permissions are needed to record".into());
    }
    let mut c = config::load();
    c.onboarded = true;
    config::save(&c).map_err(|e| e.to_string())?;
    set_phase(&app, resting(&app, &c));
    Ok(())
}

/// "Record now" without a detected call, or the detector's call: ask the API what it is.
#[tauri::command]
async fn lookup(app: AppHandle, call: Option<Call>) -> Result<(), String> {
    detected(app, call, false).await
}

/// `auto`: called for the engine's call (may start recording on its own), not by a click.
async fn detected(app: AppHandle, call: Option<Call>, auto: bool) -> Result<(), String> {
    let call = call.unwrap_or_default();
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
        _ => (Call::default(), vec![]),
    };
    let picked = meeting_id.as_ref().and_then(|id| matches.into_iter().find(|m| &m.meeting_id == id));
    let title = picked.as_ref().and_then(|m| m.title.clone()).or_else(|| call.title.as_deref().and_then(recordem_desktop::meeting_title));
    let cfg = config::load();
    let e = engine(&app)?;
    // The call's own window when the engine saw it (participants and mute come from it); one of its own otherwise.
    let window_id = match call.window_id.clone() {
        Some(w) => w,
        None => e.prepare_desktop_audio_recording().map_err(|e| e.to_string())?,
    };
    let id = uuid::Uuid::new_v4().to_string();
    let started_at = now();
    let meeting_id = picked.map(|m| m.meeting_id);
    // It starts right away, before the server answers: its upload token follows (register), and offline it waits.
    e.start_recording(&StartRecording {
        window_id: window_id.clone(),
        upload_token: None,
        recording_id: Some(id.clone()),
        metadata: serde_json::json!({ "meeting_id": meeting_id, "title": title }),
    })
    .map_err(|e| e.to_string())?;
    st.muted.store(false, Ordering::Relaxed);
    {
        let mut inner = st.inner.lock().unwrap();
        inner.recording = Some(Recorded { id: id.clone(), window_id });
        inner.known.insert(id.clone(), Known { started_at: started_at.clone(), call: call.clone(), meeting_id, title: title.clone(), warning: None });
    }
    set_phase(&app, Phase::Recording { id: id.clone(), started_at, title, app_name: call.app_name, live: cfg.live });
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = register(&app2, &id).await {
            eprintln!("recording {id} not on the server yet: {e}");
        }
    });
    Ok(())
}

/// Creates the recording on the server (idempotent by id) with an upload token, and hands the token to the engine,
/// which sends with it. Also how a recording whose token expired gets a fresh one.
async fn register(app: &AppHandle, id: &str) -> anyhow::Result<String> {
    let k = app.state::<Shared>().inner.lock().unwrap().known.get(id).cloned().unwrap_or_default();
    let cfg = config::load();
    let started_at = if k.started_at.is_empty() { now() } else { k.started_at.clone() };
    let r = api::create(&cfg, &api::NewRecording {
        id,
        meeting_id: k.meeting_id.as_deref(),
        started_at: &started_at,
        url: k.call.url.as_deref(),
        app: k.call.app.as_deref(),
        title: k.title.as_deref(),
        upload: true,
    })
    .await?;
    if let Some(k) = app.state::<Shared>().inner.lock().unwrap().known.get_mut(id) {
        k.meeting_id = Some(r.meeting_id.clone());
    }
    let token = r.upload.ok_or_else(|| anyhow::anyhow!("the server gave no upload token"))?.token;
    let e = engine(app).map_err(anyhow::Error::msg)?;
    let id = id.to_string();
    tauri::async_runtime::spawn_blocking(move || e.set_upload_token(&id, &token)).await??;
    Ok(r.meeting_id)
}

/// The meeting being recorded, in the portal (its transcript grows live there).
#[tauri::command]
async fn open_live(app: AppHandle) -> Result<(), String> {
    let inner = app.state::<Shared>().inner.lock().unwrap().recording.clone();
    let id = match inner {
        Some(r) => app.state::<Shared>().inner.lock().unwrap().known.get(&r.id).and_then(|k| k.meeting_id.clone()).ok_or("the recording isn't on the server yet")?,
        None => return Err("nothing is being recorded".into()),
    };
    open(app, id)
}

#[tauri::command]
fn toggle_mute(app: AppHandle) {
    let on = !app.state::<Shared>().muted.load(Ordering::Relaxed);
    show_muted(&app, on);
    if let Ok(e) = engine(&app) {
        let _ = e.set_muted(on);
    }
}

#[tauri::command]
fn stop(app: AppHandle) {
    let rec = app.state::<Shared>().inner.lock().unwrap().recording.clone();
    if let (Some(r), Ok(e)) = (rec, engine(&app)) {
        let _ = e.stop_recording(&r.window_id);
    }
}

/// From the call prompt: the main window, for the choices the prompt doesn't have.
#[tauri::command]
fn show_main(app: AppHandle) {
    show(&app);
}

#[tauri::command]
fn dismiss(app: AppHandle) {
    set_phase(&app, resting(&app, &config::load()));
}

/// A meeting's page in the portal.
#[tauri::command]
fn open(app: AppHandle, meeting_id: String) -> Result<(), String> {
    let url = api::meeting_url(&config::load(), &meeting_id);
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

// --- the engine's events ------------------------------------------------------------------------------------------

fn on_event(app: &AppHandle, e: Event) {
    match e {
        Event::MeetingDetected { window } => on_call(app, window),
        Event::MeetingClosed { window } => {
            let st = app.state::<Shared>();
            let mut inner = st.inner.lock().unwrap();
            if let Some(Phase::Detected { call, .. }) = &inner.phase
                && call.window_id.as_deref() == Some(window.id.as_str())
            {
                inner.phase = Some(Phase::Idle);
                drop(inner);
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.set_always_on_top(false);
                }
                hide_prompt(app);
            }
        }
        Event::RecordingEnded { recording_id, warning, error, .. } => {
            let st = app.state::<Shared>();
            let title = {
                let mut inner = st.inner.lock().unwrap();
                inner.recording = None;
                let k = inner.known.entry(recording_id.clone()).or_default();
                k.warning = warning;
                k.title.clone()
            };
            show_muted(app, false);
            match error {
                Some(message) => {
                    set_phase(app, Phase::Failed { message });
                    show(app);
                }
                None => set_phase(app, Phase::Processing { id: recording_id, progress: 0, step: "transcribing".into(), title }),
            }
        }
        Event::UploadStatus { recording_id, status, progress, error } => on_upload(app, recording_id, status, progress, error),
        Event::MuteStatus { muted, .. } => show_muted(app, muted),
        Event::ModelDownload { done, ready, error, total } => {
            let st = app.state::<Shared>();
            st.model_bytes.store(done, Ordering::Relaxed);
            if ready {
                st.models_ready.store(true, Ordering::Relaxed);
                set_phase(app, resting(app, &config::load()));
            } else if let Some(error) = error {
                set_phase(app, Phase::Models { done: 0, total, error: Some(error) });
            }
        }
        Event::PermissionStatus { permission, status } => {
            let st = app.state::<Shared>();
            let mut perms = st.perms.lock().unwrap();
            if let Some(p) = perms.iter_mut().find(|p| p.permission == permission) {
                p.status = status;
            }
        }
        Event::Error { kind, message } => eprintln!("engine {kind}: {message}"),
        Event::Shutdown { code } => eprintln!("engine stopped ({code:?})"),
        _ => {}
    }
}

/// A call the engine noticed: "Record this call?", unless something else is on screen.
fn on_call(app: &AppHandle, window: Window) {
    let st = app.state::<Shared>();
    if !matches!(st.inner.lock().unwrap().phase, None | Some(Phase::Idle) | Some(Phase::Done { .. })) {
        return;
    }
    let cfg = config::load();
    if cfg.on_call == "off" || !matches!(resting(app, &cfg), Phase::Idle) {
        return;
    }
    show_prompt(app);
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = detected(app2, Some(Call::of(&window)), true).await;
    });
}

/// How a recording after its call goes: transcribing, sending, sent; one waiting for a token gets one.
fn on_upload(app: &AppHandle, id: String, status: UploadStatus, progress: Option<u32>, error: Option<String>) {
    let st = app.state::<Shared>();
    let (k, busy) = {
        let inner = st.inner.lock().unwrap();
        let busy = matches!(inner.phase, Some(Phase::Recording { .. }) | Some(Phase::Detected { .. }));
        (inner.known.get(&id).cloned().unwrap_or_default(), busy)
    };
    match status {
        UploadStatus::Waiting => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = register(&app, &id).await {
                    refresh_pending(&app);
                    if !matches!(app.state::<Shared>().inner.lock().unwrap().phase, Some(Phase::Recording { .. }) | Some(Phase::Detected { .. })) {
                        set_phase(&app, Phase::Failed { message: format!("couldn't send it ({e}). It stays on this Mac and I'll try again when the app opens.") });
                        show(&app);
                    }
                }
            });
        }
        UploadStatus::Transcribing | UploadStatus::Uploading if !busy => {
            let step = if status == UploadStatus::Transcribing { "transcribing" } else { "sending" };
            set_phase(app, Phase::Processing { id, progress: progress.unwrap_or(0), step: step.into(), title: k.title });
        }
        UploadStatus::Done => {
            refresh_pending(app);
            notify(app, "Transcript sent", k.title.as_deref().unwrap_or("Untitled meeting"));
            if !busy {
                let quiet = k.warning.is_none();
                set_phase(app, Phase::Done { title: k.title, meeting_id: k.meeting_id, warning: k.warning });
                if !quiet {
                    show(app);
                }
            }
        }
        UploadStatus::Failed => {
            refresh_pending(app);
            if !busy {
                set_phase(app, Phase::Failed { message: error.unwrap_or_else(|| "couldn't send it".into()) });
                show(app);
            }
        }
        _ => {}
    }
}

fn refresh_pending(app: &AppHandle) {
    let n = engine(app).ok().and_then(|e| e.pending().ok()).map(|p| p.len()).unwrap_or(0);
    app.state::<Shared>().inner.lock().unwrap().pending = n;
}

/// Recordings an earlier run left behind: the engine resumes those it can send; those without a usable token (made
/// offline, or waiting longer than a token lives) get one here.
async fn resume(app: AppHandle) {
    let Ok(e) = engine(&app) else { return };
    let pending = tauri::async_runtime::spawn_blocking(move || e.pending()).await.ok().and_then(Result::ok).unwrap_or_default();
    app.state::<Shared>().inner.lock().unwrap().pending = pending.len();
    if !config::load().signed_in() {
        return;
    }
    for p in pending {
        let call = Call::of(&p.window);
        let s = |k: &str| p.metadata.get(k).and_then(|v| v.as_str()).map(String::from);
        let known = Known { started_at: p.started_at.clone(), call: Call { window_id: None, ..call }, meeting_id: s("meeting_id"), title: s("title"), warning: None };
        app.state::<Shared>().inner.lock().unwrap().known.insert(p.recording_id.clone(), known);
        if p.needs_token && p.ended_at.is_some()
            && let Err(e) = register(&app, &p.recording_id).await
        {
            eprintln!("recording {} not sent yet: {e}", p.recording_id);
        }
    }
}

/// Where recordem-desktop is: $RECORDEM_DESKTOP, or next to this app's executable (bundled as a sidecar).
fn engine_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("RECORDEM_DESKTOP") {
        return p.into();
    }
    std::env::current_exe().unwrap_or_default().with_file_name("recordem-desktop")
}

/// Starts the engine and gives it the app's settings.
fn start_engine(app: &AppHandle) -> Result<(), String> {
    let h = app.clone();
    let mut opts = recordem_desktop::Options::new(engine_path());
    // A permission prompt waits for the person.
    opts.timeout = std::time::Duration::from_secs(150);
    let client = Client::start(opts, move |e| on_event(&h, e)).map_err(|e| e.to_string())?;
    let ready = client.init(&settings(&config::load())).map_err(|e| e.to_string())?;
    let st = app.state::<Shared>();
    st.models_ready.store(ready.models_ready, Ordering::Relaxed);
    st.inner.lock().unwrap().pending = ready.pending.len();
    let _ = st.engine.set(client);
    Ok(())
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
                    "stop" => stop(app.clone()),
                    "quit" => {
                        // Never drop a recording on the floor: stop it and let it finish before quitting.
                        let recording = app.state::<Shared>().inner.lock().unwrap().recording.is_some();
                        if recording {
                            stop(app.clone());
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

            // The engine: calls, recording, transcription, sending. Without it there is nothing to do but say so.
            if let Err(e) = start_engine(&handle) {
                set_phase(&handle, Phase::Failed { message: format!("the recording engine didn't start: {e}") });
                show(&handle);
            }

            // Login item follows the setting (on by default), also on the very first run.
            let cfg = config::load();
            if app.autolaunch().is_enabled().unwrap_or(false) != cfg.launch_at_login {
                apply_autostart(&handle, cfg.launch_at_login);
            }
            let pending = handle.state::<Shared>().inner.lock().unwrap().pending;
            if !matches!(resting(&handle, &cfg), Phase::Idle) || pending > 0 {
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
