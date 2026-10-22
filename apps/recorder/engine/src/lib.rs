// The Recordem engine: notices calls (a meeting app or browser starts using the mic), records mic + system audio
// (and your own voice at 48 kHz), reads who is in the call and who speaks, transcribes on this Mac, and sends the
// transcript, then the audio, to a Recordem server with the recording's upload token. It has no window: a host (the
// Recordem app, or any app through recordem-desktop) decides what to record and gets everything back as events.
//
// A recording that couldn't be transcribed or sent stays in recordings/<id>/ and is picked up again at the next `init`;
// one whose token is missing or expired waits for `set_upload_token`.
pub mod api;
mod apptexts;
pub mod asr;
pub mod bench;
mod callurl;
pub mod capture;
pub mod config;
pub mod eval;
mod diarize;
mod dumps;
mod live;
pub mod models;
mod mute;
mod participants;
pub mod permissions;
mod profiles;
mod transcribe;
mod vbx;

use recordem_desktop::{Event, Pending, PermissionItem, Ready, Settings, StartRecording, UploadStatus, Window};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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

/// After the call app lets go of the mic, wait this long before stopping (a reconnect or device switch is not the end).
const STOP_GRACE_SECS: u64 = 45;

/// The call as meta.json keeps it (the recorder app's format since 0.1, so its unsent recordings still read).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Call {
    app_id: Option<String>,
    app_name: Option<String>,
    app: Option<String>, // google_meet | zoom | microsoft_teams | … for the API
    title: Option<String>,
    url: Option<String>,
}

impl Call {
    fn of(w: &Window) -> Call {
        Call { app_id: w.app_id.clone(), app_name: w.app_name.clone(), app: w.platform.clone(), title: w.title.clone(), url: w.url.clone() }
    }

    fn window(&self, id: &str) -> Window {
        Window { id: id.into(), title: self.title.clone(), url: self.url.clone(), platform: self.app.clone(), app_id: self.app_id.clone(), app_name: self.app_name.clone() }
    }
}

/// What a recording knows before it has a transcript; saved as meta.json next to the audio.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Meta {
    id: String,
    /// Kept from recordings the app made before the engine: the meeting it picked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    meeting_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    call: Call,
    started_at: String,
    ended_at: Option<String>,
    language: String, // the languages its turns may be in ("pt,en")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_id: Option<String>,
    /// The token it is sent with; a fresh one replaces it (`set_upload_token`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    upload_token: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    metadata: serde_json::Value,
}

/// The recording going on.
struct Rec {
    id: String,
    window: Window,
    stop: CancellationToken,
    token: live::Token,
    /// Running while the call app has let go of the mic: stops the recording when it runs out.
    stop_timer: Option<CancellationToken>,
}

type Emit = Arc<dyn Fn(Event) + Send + Sync>;

struct State {
    emit: Emit,
    rt: tokio::runtime::Handle,
    /// Calls seen (and windows prepared for a recording with no call), by window id.
    windows: Mutex<HashMap<String, Window>>,
    next_window: AtomicU64,
    rec: Mutex<Option<Rec>>,
    muted: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    /// Recordings being transcribed or sent now (one at a time: Whisper takes the machine).
    busy: Mutex<Vec<String>>,
    queue: tokio::sync::Mutex<()>,
    started: AtomicBool,
    /// Recordings started and not yet through `process` (the gap between the two is covered too).
    active: AtomicU64,
}

/// The engine. Cheap to clone; all clones are the same engine. Its methods need a tokio runtime.
#[derive(Clone)]
pub struct Engine(Arc<State>);

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn write_json<T: Serialize>(p: &Path, v: &T) -> anyhow::Result<()> {
    std::fs::write(p, serde_json::to_vec_pretty(v)?)?;
    Ok(())
}

fn read_json<T: for<'a> Deserialize<'a>>(p: &Path) -> anyhow::Result<T> {
    Ok(serde_json::from_slice(&std::fs::read(p)?)?)
}

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

/// A token good for at least another minute.
fn usable(token: Option<&str>) -> Option<String> {
    let t = token?;
    match recordem_desktop::token_expiry(t) {
        Some(exp) if exp < chrono::Utc::now().timestamp() + 60 => None,
        _ => Some(t.to_string()),
    }
}

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

impl Engine {
    /// An engine that reports to `emit`. Nothing runs until `init`.
    pub fn new(emit: impl Fn(Event) + Send + Sync + 'static) -> Engine {
        Engine(Arc::new(State {
            emit: Arc::new(emit),
            rt: tokio::runtime::Handle::current(),
            windows: Mutex::new(HashMap::new()),
            next_window: AtomicU64::new(1),
            rec: Mutex::new(None),
            muted: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            busy: Mutex::new(vec![]),
            queue: tokio::sync::Mutex::new(()),
            started: AtomicBool::new(false),
            active: AtomicU64::new(0),
        }))
    }

    fn emit(&self, e: Event) {
        (self.0.emit)(e)
    }

    pub fn init(&self, s: &Settings) -> anyhow::Result<Ready> {
        anyhow::ensure!(s.api_url.as_deref().is_some_and(|u| !u.is_empty()), "init needs apiUrl");
        anyhow::ensure!(s.data_dir.as_deref().is_some_and(|d| !d.is_empty()), "init needs dataDir");
        config::apply(s);
        std::fs::create_dir_all(config::recordings_dir())?;
        if !self.0.started.swap(true, Ordering::Relaxed) {
            self.0.rt.spawn(profiles::keep_fresh());
            if config::load().detect {
                self.detect();
            }
            self.watch_permissions();
        }
        let e = self.clone();
        self.0.rt.spawn(async move { e.resume().await });
        Ok(Ready { protocol: recordem_desktop::PROTOCOL, version: VERSION.into(), models_ready: models::ready(), pending: self.pending() })
    }

    pub fn configure(&self, s: &Settings) {
        config::apply(s);
    }

    // --- calls ------------------------------------------------------------------------------------------------------

    fn new_window(&self, mut w: Window) -> Window {
        w.id = format!("w{}", self.0.next_window.fetch_add(1, Ordering::Relaxed));
        self.0.windows.lock().unwrap().insert(w.id.clone(), w.clone());
        w
    }

    /// A window for a recording with no call (in person, or "record now").
    pub fn prepare_desktop_audio_recording(&self) -> String {
        self.new_window(Window::default()).id
    }

    fn open_window_of(&self, app_id: &str) -> Option<Window> {
        self.0.windows.lock().unwrap().values().find(|w| w.app_id.as_deref() == Some(app_id)).cloned()
    }

    fn close_window(&self, id: &str) {
        if let Some(w) = self.0.windows.lock().unwrap().remove(id) {
            self.emit(Event::MeetingClosed { window: w });
        }
    }

    /// The detector lives as long as the engine; it runs its own Core Audio listener thread.
    fn detect(&self) {
        let detector: &'static mut anlg_detect::Detector = Box::leak(Box::default());
        let e = self.clone();
        detector.start(anlg_detect::new_callback(move |ev| e.on_detect(ev)));
    }

    fn on_detect(&self, ev: anlg_detect::DetectEvent) {
        match ev {
            anlg_detect::DetectEvent::MicStarted(apps) => {
                // The call app picked the mic up again: don't stop.
                if let Some(rec) = self.0.rec.lock().unwrap().as_mut()
                    && apps.iter().any(|a| Some(&a.id) == rec.window.app_id.as_ref())
                    && let Some(t) = rec.stop_timer.take()
                {
                    t.cancel();
                }
                for a in apps {
                    let Some(kind) = meeting_app(&a.id) else { continue };
                    if self.open_window_of(&a.id).is_some() {
                        continue;
                    }
                    let w = self.new_window(Window { app_id: Some(a.id.clone()), app_name: Some(a.name.clone()), platform: kind.map(String::from), ..Default::default() });
                    let e = self.clone();
                    self.0.rt.spawn_blocking(move || {
                        let mut w = w;
                        let (title, platform) = inspect(&a.id);
                        w.title = title;
                        w.platform = w.platform.or(platform);
                        // In a browser, the meeting tab has the call's whole link (and sometimes its name).
                        if kind.is_none()
                            && let Some((url, tab)) = callurl::find(&a.id)
                        {
                            w.platform = w.platform.or_else(|| callurl::platform(&a.id, &url).filter(|p| !p.is_empty()));
                            w.title = w.title.or(Some(tab));
                            w.url = Some(url);
                        }
                        // Gone while it was looked at: nothing to say.
                        let mut windows = e.0.windows.lock().unwrap();
                        let Some(slot) = windows.get_mut(&w.id) else { return };
                        *slot = w.clone();
                        drop(windows);
                        e.emit(Event::MeetingDetected { window: w });
                    });
                }
            }
            anlg_detect::DetectEvent::MicStopped(apps) => {
                for a in apps {
                    let Some(w) = self.open_window_of(&a.id) else { continue };
                    let mut rec = self.0.rec.lock().unwrap();
                    match rec.as_mut() {
                        Some(r) if r.window.id == w.id => {
                            let timer = CancellationToken::new();
                            r.stop_timer = Some(timer.clone());
                            let (stop, e, id) = (r.stop.clone(), self.clone(), w.id.clone());
                            self.0.rt.spawn(async move {
                                tokio::select! {
                                    _ = timer.cancelled() => {}
                                    _ = tokio::time::sleep(Duration::from_secs(STOP_GRACE_SECS)) => {
                                        stop.cancel();
                                        e.close_window(&id);
                                    }
                                }
                            });
                        }
                        _ => {
                            drop(rec);
                            self.close_window(&w.id);
                        }
                    }
                }
            }
        }
    }

    // --- recording --------------------------------------------------------------------------------------------------

    pub fn start_recording(&self, p: &StartRecording) -> anyhow::Result<String> {
        let window = self.0.windows.lock().unwrap().get(&p.window_id).cloned().ok_or_else(|| anyhow::anyhow!("no window {}", p.window_id))?;
        let from_token = p.upload_token.as_deref().map(|t| recordem_desktop::token_recording(t).ok_or_else(|| anyhow::anyhow!("that isn't an upload token"))).transpose()?;
        let id = match (from_token, &p.recording_id) {
            (Some(a), Some(b)) if &a != b => anyhow::bail!("the upload token is for recording {a}, not {b}"),
            (Some(a), _) => a,
            (None, Some(b)) => b.clone(),
            (None, None) => anyhow::bail!("startRecording needs an uploadToken (or a recordingId, and the token later)"),
        };
        let mut rec = self.0.rec.lock().unwrap();
        anyhow::ensure!(rec.is_none(), "already recording");
        let dir = config::recordings_dir().join(&id);
        anyhow::ensure!(!dir.join("meta.json").exists(), "recording {id} was already made on this computer");
        std::fs::create_dir_all(&dir)?;
        let cfg = config::load();
        let meta = Meta {
            id: id.clone(),
            meeting_id: None,
            title: None,
            call: Call::of(&window),
            started_at: now(),
            ended_at: None,
            language: cfg.languages.clone(),
            window_id: Some(window.id.clone()),
            upload_token: p.upload_token.clone(),
            metadata: p.metadata.clone(),
        };
        write_json(&dir.join("meta.json"), &meta)?;
        let stop = CancellationToken::new();
        let token: live::Token = Arc::new(Mutex::new(p.upload_token.clone()));
        self.0.muted.store(false, Ordering::Relaxed);
        self.0.paused.store(false, Ordering::Relaxed);
        *rec = Some(Rec { id: id.clone(), window: window.clone(), stop: stop.clone(), token: token.clone(), stop_timer: None });
        drop(rec);
        self.0.active.fetch_add(1, Ordering::Relaxed);
        self.emit(Event::RecordingStarted { window: window.clone(), recording_id: id.clone() });
        self.run(meta, dir, window, stop, token, cfg);
        Ok(id)
    }

    /// Everything a recording does while the call goes on, then the pass after it.
    fn run(&self, meta: Meta, dir: PathBuf, window: Window, stop: CancellationToken, token: live::Token, cfg: config::Config) {
        let rt = self.0.rt.clone();
        // The live transcript: a thread transcribes as the capture goes, a task sends what it makes (once there is a
        // token) and hands every line to the host.
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
            let (e, id, stop2, people, dir2, w) = (self.clone(), meta.id.clone(), stop.clone(), people.clone(), dir.clone(), window.clone());
            let sender = rt.spawn(live::send(cfg.api_url.clone(), token, meta.id.clone(), segs, stop2, people, dir2, move |s, name| {
                let mut data = serde_json::to_value(s).unwrap_or_default();
                if let Some(n) = name {
                    data["name"] = n.into();
                }
                e.emit(Event::RealtimeEvent { window: w.clone(), recording_id: id.clone(), event: "transcript.data".into(), data });
            }));
            (Some(frames_tx), Some((worker, sender)))
        } else {
            (None, None)
        };
        // Mute follows the call's own button where it can be read (Meet in a browser, the Zoom app): where it is and what
        // it says come from the app's reader profile.
        let source = window.app_id.as_deref().and_then(|id| {
            let all = profiles::current();
            let p = all.for_app(id, None).into_iter().find(|p| p.self_mic.is_some())?;
            Some(mute::Reader { mic: p.self_mic.clone()?, links: p.links.clone() })
        });
        if let (Some(source), Some(id)) = (source, window.app_id.clone()) {
            let (e, w) = (self.clone(), window.clone());
            mute::watch(source, id, stop.clone(), move |m| {
                e.0.muted.store(m, Ordering::Relaxed);
                e.emit(Event::MuteStatus { window: w.clone(), muted: m });
            });
        }
        // Who is in the call and who speaks, from the call app's window: told to the host as it changes, names the
        // voices after the call.
        if let Some(id) = window.app_id.clone() {
            participants::watch(id, dir.clone(), std::time::Instant::now(), stop.clone(), people.clone());
            rt.spawn(self.clone().tell_people(window.clone(), meta.id.clone(), people, stop.clone()));
        }
        let e = self.clone();
        let (muted, paused) = (self.0.muted.clone(), self.0.paused.clone());
        rt.spawn(async move {
            // The other side's voices are told apart as the call goes; the pass after it only clusters them.
            let (voices_tx, voices_rx) = std::sync::mpsc::channel();
            let vdir = dir.clone();
            let voices = std::thread::spawn(move || diarize::during(voices_rx, &vdir));
            let result = capture::record(&dir, cfg.mic_device.clone(), cfg.voice_hq, stop, live_tx, voices_tx, muted.clone(), paused.clone()).await;
            let _ = tokio::task::spawn_blocking(move || voices.join()).await;
            // The capture dropped its end: the live thread finishes the turn it is on and stops, before the pass after
            // the call needs Whisper. Its sender gets one last try.
            if let Some((worker, sender)) = live {
                let _ = tokio::task::spawn_blocking(move || worker.join()).await;
                let _ = sender.await;
            }
            // meta.json again from disk: a token may have arrived while recording.
            let mut meta: Meta = read_json(&dir.join("meta.json")).unwrap_or(meta);
            meta.ended_at = Some(now());
            let _ = write_json(&dir.join("meta.json"), &meta);
            {
                let mut rec = e.0.rec.lock().unwrap();
                if let Some(t) = rec.as_mut().and_then(|r| r.stop_timer.take()) {
                    t.cancel();
                }
                *rec = None;
            }
            muted.store(false, Ordering::Relaxed);
            paused.store(false, Ordering::Relaxed);
            let warning = match &result {
                Ok((m, s)) if *m < 0.001 && *s < 0.001 => Some("No sound was captured. Check the Microphone and System Audio Recording permissions in System Settings → Privacy & Security.".to_string()),
                Ok((m, _)) if *m < 0.001 => Some("The microphone captured nothing. Check the Microphone permission.".to_string()),
                Ok((_, s)) if *s < 0.001 => Some("The call's audio wasn't captured. Check the System Audio Recording permission.".to_string()),
                _ => None,
            };
            let error = result.as_ref().err().map(|e| e.to_string());
            e.emit(Event::RecordingEnded { window, recording_id: meta.id.clone(), warning, error: error.clone() });
            match error {
                None => e.process(dir).await,
                Some(m) => e.report(&dir, "capture_failed", &m).await,
            }
            e.0.active.fetch_sub(1, Ordering::Relaxed);
        });
    }

    /// Who is in the call (`participants.update`) and who the app shows speaking (`speaker.on`), when they change.
    async fn tell_people(self, window: Window, id: String, people: participants::Latest, stop: CancellationToken) {
        let mut last: Option<api::Participants> = None;
        let mut speaking: Vec<String> = vec![];
        while !stop.is_cancelled() {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let Some(now) = people.lock().unwrap().as_ref().map(|l| l.participants()) else { continue };
            if last.as_ref().is_none_or(|l| api::Participants { t_ms: now.t_ms, ..l.clone() } != now) {
                let data = serde_json::to_value(&now).unwrap_or_default();
                self.emit(Event::RealtimeEvent { window: window.clone(), recording_id: id.clone(), event: "participants.update".into(), data });
                let on: Vec<String> = now.people.iter().filter(|p| p.speaking).map(|p| p.name.clone()).collect();
                if on != speaking {
                    let data = serde_json::json!({ "t_ms": now.t_ms, "names": on });
                    self.emit(Event::RealtimeEvent { window: window.clone(), recording_id: id.clone(), event: "speaker.on".into(), data });
                    speaking = on;
                }
                last = Some(now);
            }
        }
    }

    fn with_rec<T>(&self, window_id: &str, f: impl FnOnce(&mut Rec) -> T) -> anyhow::Result<T> {
        let mut rec = self.0.rec.lock().unwrap();
        match rec.as_mut() {
            Some(r) if r.window.id == window_id => Ok(f(r)),
            _ => anyhow::bail!("{window_id} isn't being recorded"),
        }
    }

    pub fn stop_recording(&self, window_id: &str) -> anyhow::Result<()> {
        self.with_rec(window_id, |r| r.stop.cancel())
    }

    pub fn pause_recording(&self, window_id: &str) -> anyhow::Result<()> {
        self.with_rec(window_id, |_| self.0.paused.store(true, Ordering::Relaxed))
    }

    pub fn resume_recording(&self, window_id: &str) -> anyhow::Result<()> {
        self.with_rec(window_id, |_| self.0.paused.store(false, Ordering::Relaxed))
    }

    pub fn set_muted(&self, on: bool) {
        self.0.muted.store(on, Ordering::Relaxed);
        if let Some(w) = self.0.rec.lock().unwrap().as_ref().map(|r| r.window.clone()) {
            self.emit(Event::MuteStatus { window: w, muted: on });
        }
    }

    /// The token for a recording started without one, or a fresh one for one whose token expired: the recording
    /// going on sends with it right away; one already over is transcribed (if needed) and sent.
    pub fn set_upload_token(&self, recording_id: &str, token: &str) -> anyhow::Result<()> {
        if let Some(r) = recordem_desktop::token_recording(token) {
            anyhow::ensure!(r == recording_id, "the upload token is for recording {r}, not {recording_id}");
        }
        let dir = config::recordings_dir().join(recording_id);
        let mut meta: Meta = read_json(&dir.join("meta.json")).map_err(|_| anyhow::anyhow!("no recording {recording_id} on this computer"))?;
        meta.upload_token = Some(token.to_string());
        write_json(&dir.join("meta.json"), &meta)?;
        let live = self.0.rec.lock().unwrap().as_ref().filter(|r| r.id == recording_id).map(|r| r.token.clone());
        match live {
            Some(t) => *t.lock().unwrap() = Some(token.to_string()),
            None if !dir.join("sent").exists() && meta.ended_at.is_some() => {
                let e = self.clone();
                self.0.rt.spawn(async move { e.process(dir).await });
            }
            None => {}
        }
        Ok(())
    }

    // --- after a recording ------------------------------------------------------------------------------------------

    fn status(&self, id: &str, status: UploadStatus, progress: Option<u32>, error: Option<String>) {
        self.emit(Event::UploadStatus { recording_id: id.into(), status, progress, error });
    }

    /// Tells the server the recording failed for good (best effort: with no usable token, only the host knows).
    async fn report(&self, dir: &Path, reason: &str, message: &str) {
        let Ok(meta) = read_json::<Meta>(&dir.join("meta.json")) else { return };
        let Some(token) = usable(meta.upload_token.as_deref()) else { return };
        let target = api::Target { api_url: config::load().api_url, token };
        if let Err(e) = api::report_failure(&target, &meta.id, reason, message).await {
            eprintln!("failure of {} not reported: {e}", meta.id);
        }
    }

    /// Transcribe (unless already done) and send one recording directory. One at a time.
    async fn process(&self, dir: PathBuf) {
        let Some(id) = dir.file_name().map(|n| n.to_string_lossy().to_string()) else { return };
        {
            let mut busy = self.0.busy.lock().unwrap();
            if busy.contains(&id) {
                return;
            }
            busy.push(id.clone());
        }
        let _turn = self.0.queue.lock().await;
        self.process_one(&id, &dir).await;
        self.0.busy.lock().unwrap().retain(|b| b != &id);
    }

    async fn process_one(&self, id: &str, dir: &Path) {
        let meta: Meta = match read_json(&dir.join("meta.json")) {
            Ok(m) => m,
            Err(e) => return self.status(id, UploadStatus::Failed, None, Some(format!("unreadable recording: {e}"))),
        };
        let tpath = dir.join("transcript.json");
        let transcript: api::Transcript = match read_json(&tpath) {
            Ok(t) => t,
            Err(_) => {
                let progress = Arc::new(AtomicU32::new(0));
                self.status(id, UploadStatus::Transcribing, Some(0), None);
                let ticker = {
                    let (e, p, id) = (self.clone(), progress.clone(), id.to_string());
                    self.0.rt.spawn(async move {
                        loop {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            e.status(&id, UploadStatus::Transcribing, Some(p.load(Ordering::Relaxed)), None);
                        }
                    })
                };
                let (d, lang) = (dir.to_path_buf(), meta.language.clone());
                let c = config::load();
                let (vocabulary, engine) = (c.vocabulary, c.asr_final);
                let model = format!("{} + silero vad + pyannote", asr::describe(&engine));
                let p = progress.clone();
                let segs = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                    let f = capture::files(&d);
                    transcribe::run(&capture::read(&f.mic)?, &capture::read(&f.system)?, diarize::found(&d), &lang, &vocabulary, &engine, &p)
                })
                .await;
                ticker.abort();
                let mut segments = match segs.map_err(anyhow::Error::from).and_then(|r| r) {
                    Ok(s) => s,
                    Err(e) => {
                        let m = format!("transcription failed: {e}");
                        self.report(dir, "transcription_failed", &m).await;
                        return self.status(id, UploadStatus::Failed, None, Some(m));
                    }
                };
                // Names for the lines, from who the call app showed speaking.
                let speakers = participants::name_lines(&participants::read(dir), &mut segments);
                let t = api::Transcript { ended_at: meta.ended_at.clone().unwrap_or_else(now), language: Some(spoken(&segments)), model: Some(model), segments, speakers };
                if let Err(e) = write_json(&tpath, &t) {
                    return self.status(id, UploadStatus::Failed, None, Some(e.to_string()));
                }
                t
            }
        };
        let cfg = config::load();
        let Some(token) = usable(meta.upload_token.as_deref()) else {
            return self.status(id, UploadStatus::Waiting, None, Some(if meta.upload_token.is_some() { "the upload token expired" } else { "no upload token yet" }.into()));
        };
        self.status(id, UploadStatus::Uploading, Some(1000), None);
        let target = api::Target { api_url: cfg.api_url.clone(), token };
        // The text first (the part that matters), then the audio: the whole call, and your own voice in high quality.
        // All idempotent, so a retry resends everything.
        let sent = async {
            api::deliver(&target, id, &transcript).await?;
            let d = dir.to_path_buf();
            let (mix, voice) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> { Ok((capture::call_m4a(&d)?, capture::voice_m4a(&d)?)) }).await??;
            api::upload_track(&target, id, "mix", &mix).await?;
            if let Some(v) = voice {
                api::upload_track(&target, id, "mic_hq", &v).await?;
            }
            // The reader's broken snapshots, when allowed: a failure here doesn't hold the recording back.
            if cfg.share_reader_dumps {
                for (name, bytes) in dumps::all(dir) {
                    if let Err(e) = api::put_reader_dump(&target, id, &name, bytes).await {
                        eprintln!("reader dump {name} not sent: {e}");
                    }
                }
            }
            anyhow::Ok(())
        }
        .await;
        match sent {
            Ok(()) => {
                let _ = std::fs::write(dir.join("sent"), now());
                if !cfg.keep_audio {
                    let f = capture::files(dir);
                    for p in [f.mic, f.system, f.mic_hq, dir.join("call.m4a"), dir.join("voice.m4a")] {
                        let _ = std::fs::remove_file(p);
                    }
                }
                self.status(id, UploadStatus::Done, Some(1000), None);
            }
            Err(e) if e.is::<api::Unauthorized>() => self.status(id, UploadStatus::Waiting, None, Some(e.to_string())),
            Err(e) => self.status(id, UploadStatus::Failed, None, Some(format!("couldn't send it ({e}); it stays on this computer and is tried again at the next start"))),
        }
    }

    /// Recordings not sent yet (no `sent` marker), oldest first; the one being recorded isn't one.
    fn unsent(&self) -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(config::recordings_dir()) else { return vec![] };
        let current = self.0.rec.lock().unwrap().as_ref().map(|r| r.id.clone());
        let mut v: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join("meta.json").exists() && !p.join("sent").exists())
            .filter(|p| current.as_deref() != p.file_name().and_then(|n| n.to_str()))
            .collect();
        v.sort();
        v
    }

    pub fn pending(&self) -> Vec<Pending> {
        self.unsent()
            .into_iter()
            .filter_map(|d| read_json::<Meta>(&d.join("meta.json")).ok())
            .map(|m| {
                let mut metadata = m.metadata.clone();
                // Recordings the app made before the engine: what it knew, for the host to create them on the server.
                if metadata.is_null() && (m.meeting_id.is_some() || m.title.is_some()) {
                    metadata = serde_json::json!({ "meeting_id": m.meeting_id, "title": m.title });
                }
                Pending {
                    recording_id: m.id.clone(),
                    window: m.call.window(m.window_id.as_deref().unwrap_or("")),
                    started_at: m.started_at.clone(),
                    ended_at: m.ended_at.clone(),
                    needs_token: usable(m.upload_token.as_deref()).is_none(),
                    metadata,
                }
            })
            .collect()
    }

    /// At `init`: finish whatever an earlier run left behind (quit mid-transcription, offline, …).
    async fn resume(&self) {
        for dir in self.unsent() {
            let meta: Option<Meta> = read_json(&dir.join("meta.json")).ok();
            // A recording without an end was cut short by a crash: its audio is still usable up to where it stopped,
            // but hound leaves the header unfinished; skip it rather than send half a file silently.
            if meta.as_ref().map(|m| m.ended_at.is_none()).unwrap_or(true) {
                continue;
            }
            self.process(dir).await;
        }
    }

    // --- permissions, devices, models -------------------------------------------------------------------------------

    pub fn permissions(&self) -> Vec<PermissionItem> {
        permissions::all().into_iter().map(permissions::Item::public).collect()
    }

    /// Tells the host each permission's status at the start and whenever it changes (granted in System Settings…).
    fn watch_permissions(&self) {
        let e = self.clone();
        self.0.rt.spawn(async move {
            let mut last: Vec<PermissionItem> = vec![];
            loop {
                let now = tokio::task::spawn_blocking(permissions::all).await.unwrap_or_default();
                for i in now.into_iter().map(permissions::Item::public) {
                    if !last.iter().any(|l| l.permission == i.permission && l.status == i.status) {
                        e.emit(Event::PermissionStatus { permission: i.permission, status: i.status });
                    }
                    last.retain(|l| l.permission != i.permission);
                    last.push(i);
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    pub async fn request_permission(&self, p: recordem_desktop::Permission) {
        let _ = tokio::task::spawn_blocking(move || permissions::request(permissions::id(p))).await;
    }

    pub fn open_permission_settings(&self, p: recordem_desktop::Permission) {
        permissions::open_settings(permissions::id(p));
    }

    pub fn list_devices(&self) -> Vec<String> {
        capture::mics()
    }

    /// Downloads the speech models, telling the host how far along it is.
    pub fn download_models(&self) {
        let e = self.clone();
        self.0.rt.spawn(async move {
            let done = Arc::new(AtomicU64::new(0));
            let total = models::total();
            let ticker = {
                let (e, done) = (e.clone(), done.clone());
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        e.emit(Event::ModelDownload { done: done.load(Ordering::Relaxed), total, ready: false, error: None });
                    }
                })
            };
            let r = models::download(done.clone()).await;
            ticker.abort();
            let error = r.err().map(|e| e.to_string());
            e.emit(Event::ModelDownload { done: done.load(Ordering::Relaxed), total, ready: error.is_none(), error });
        });
    }

    /// Stops the recording going on, and waits for it and every recording in line to be processed.
    pub async fn shutdown(&self) {
        let stop = self.0.rec.lock().unwrap().as_ref().map(|r| r.stop.clone());
        if let Some(s) = stop {
            s.cancel();
        }
        loop {
            let idle = self.0.active.load(Ordering::Relaxed) == 0 && self.0.busy.lock().unwrap().is_empty();
            if idle {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}
