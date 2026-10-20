//! Records calls from your app with the Recordem Desktop SDK. This crate starts the `recordem-desktop` engine (the
//! Recordem recorder without its window, shipped next to your app) and talks to it: typed commands, and its events on
//! a callback. If the engine exits on its own, it is started again and given the same `init`.
//!
//! ```no_run
//! use recordem_desktop::{Client, Event, Options, Settings, StartRecording};
//! let client = Client::start(Options::new("/Applications/MyApp.app/Contents/MacOS/recordem-desktop"), |e| {
//!     if let Event::MeetingDetected { window } = e { println!("a call: {:?}", window.url) }
//! })?;
//! client.init(&Settings { api_url: Some("https://api.recordem.app".into()), data_dir: Some("/tmp/rec".into()), ..Default::default() })?;
//! // Your backend creates the recording (POST /v1/recordings with upload: true) and hands back its token:
//! client.start_recording(&StartRecording { window_id: "w1".into(), upload_token: Some("eyJ…".into()), ..Default::default() })?;
//! # Ok::<(), recordem_desktop::Error>(())
//! ```
pub mod protocol;
mod title;

pub use protocol::*;
pub use title::meeting_title;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug)]
pub enum Error {
    /// The engine couldn't be started, or its pipe broke.
    Io(std::io::Error),
    /// The engine refused the command, with why.
    Engine(String),
    /// No answer in time (the engine is restarting, or stuck).
    Timeout,
    /// The engine speaks a protocol this client doesn't know.
    Protocol(u32),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "recordem-desktop: {e}"),
            Error::Engine(e) => write!(f, "{e}"),
            Error::Timeout => write!(f, "recordem-desktop didn't answer"),
            Error::Protocol(p) => write!(f, "recordem-desktop speaks protocol {p}, this client {PROTOCOL}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub struct Options {
    /// The `recordem-desktop` executable.
    pub path: PathBuf,
    /// Start it again when it exits on its own; default true.
    pub restart: bool,
    /// How long a command may wait for its answer; default 30 s.
    pub timeout: Duration,
    /// Environment for the engine (RECORDEM_PARAKEET, …), on top of this process's.
    pub env: Vec<(String, String)>,
}

impl Options {
    pub fn new(path: impl Into<PathBuf>) -> Options {
        Options { path: path.into(), restart: true, timeout: Duration::from_secs(30), env: vec![] }
    }
}

type OnEvent = Box<dyn Fn(Event) + Send + Sync>;

struct Inner {
    opts: Options,
    on_event: OnEvent,
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    waiting: Mutex<HashMap<u64, Sender<Response>>>,
    next: AtomicU64,
    /// The last `init`, given again to an engine started after a crash.
    init: Mutex<Option<Value>>,
    closing: AtomicBool,
}

/// A running engine. Cheap to clone; every clone talks to the same process.
#[derive(Clone)]
pub struct Client(Arc<Inner>);

impl Client {
    /// Starts the engine; `on_event` gets every event, on a thread of the client's.
    pub fn start(opts: Options, on_event: impl Fn(Event) + Send + Sync + 'static) -> Result<Client, Error> {
        let inner = Arc::new(Inner {
            opts,
            on_event: Box::new(on_event),
            child: Mutex::new(None),
            stdin: Mutex::new(None),
            waiting: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            init: Mutex::new(None),
            closing: AtomicBool::new(false),
        });
        spawn(&inner)?;
        Ok(Client(inner))
    }

    /// Sends one command and waits for its answer: the raw form of every method below.
    pub fn call(&self, cmd: &str, params: Value) -> Result<Value, Error> {
        call(&self.0, cmd, params)
    }

    fn typed<T: DeserializeOwned>(&self, cmd: &str, params: Value) -> Result<T, Error> {
        serde_json::from_value(self.call(cmd, params)?).map_err(|e| Error::Engine(format!("{cmd}: unexpected answer: {e}")))
    }

    pub fn init(&self, s: &Settings) -> Result<Ready, Error> {
        let params = serde_json::to_value(s).expect("settings");
        let ready: Ready = self.typed("init", params.clone())?;
        *self.0.init.lock().unwrap() = Some(params);
        Ok(ready)
    }

    /// Changes some settings; the next recording uses them.
    pub fn configure(&self, s: &Settings) -> Result<(), Error> {
        let params = serde_json::to_value(s).expect("settings");
        self.call("configure", params.clone())?;
        if let Some(Value::Object(init)) = self.0.init.lock().unwrap().as_mut()
            && let Value::Object(p) = params
        {
            init.extend(p);
        }
        Ok(())
    }

    /// Returns the recording's id.
    pub fn start_recording(&self, s: &StartRecording) -> Result<String, Error> {
        let r: Value = self.call("startRecording", serde_json::to_value(s).expect("params"))?;
        Ok(r["recordingId"].as_str().unwrap_or_default().to_string())
    }

    pub fn stop_recording(&self, window_id: &str) -> Result<(), Error> {
        self.call("stopRecording", json!({ "windowId": window_id })).map(drop)
    }

    pub fn pause_recording(&self, window_id: &str) -> Result<(), Error> {
        self.call("pauseRecording", json!({ "windowId": window_id })).map(drop)
    }

    pub fn resume_recording(&self, window_id: &str) -> Result<(), Error> {
        self.call("resumeRecording", json!({ "windowId": window_id })).map(drop)
    }

    /// The token for a recording started without one, or a fresh one for a recording whose token expired.
    pub fn set_upload_token(&self, recording_id: &str, upload_token: &str) -> Result<(), Error> {
        self.call("setUploadToken", json!({ "recordingId": recording_id, "uploadToken": upload_token })).map(drop)
    }

    /// A window to record with no call (in person): returns its id.
    pub fn prepare_desktop_audio_recording(&self) -> Result<String, Error> {
        let r: Value = self.call("prepareDesktopAudioRecording", json!({}))?;
        Ok(r["windowId"].as_str().unwrap_or_default().to_string())
    }

    pub fn permissions(&self) -> Result<Vec<PermissionItem>, Error> {
        self.typed("permissions", json!({}))
    }

    /// Asks macOS (its prompt, or nothing once refused: then `open_permission_settings`).
    pub fn request_permission(&self, p: Permission) -> Result<(), Error> {
        self.call("requestPermission", json!({ "permission": p })).map(drop)
    }

    pub fn open_permission_settings(&self, p: Permission) -> Result<(), Error> {
        self.call("openPermissionSettings", json!({ "permission": p })).map(drop)
    }

    pub fn set_muted(&self, muted: bool) -> Result<(), Error> {
        self.call("setMuted", json!({ "muted": muted })).map(drop)
    }

    /// Microphones by name.
    pub fn list_devices(&self) -> Result<Vec<String>, Error> {
        self.typed("listDevices", json!({}))
    }

    /// Downloads the speech models; progress comes as `model-download` events.
    pub fn download_models(&self) -> Result<(), Error> {
        self.call("downloadModels", json!({})).map(drop)
    }

    pub fn pending(&self) -> Result<Vec<Pending>, Error> {
        self.typed("pending", json!({}))
    }

    /// Stops the engine: a recording in progress is stopped and processed first.
    pub fn shutdown(&self) -> Result<(), Error> {
        self.0.closing.store(true, Ordering::Relaxed);
        let r = self.call("shutdown", json!({}));
        if let Some(mut c) = self.0.child.lock().unwrap().take() {
            let _ = c.wait();
        }
        r.map(drop)
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
        }
    }
}

fn call(inner: &Arc<Inner>, cmd: &str, params: Value) -> Result<Value, Error> {
    let id = inner.next.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = channel();
    inner.waiting.lock().unwrap().insert(id, tx);
    let line = serde_json::to_string(&Request { id, cmd: cmd.into(), params }).expect("request");
    let sent = match inner.stdin.lock().unwrap().as_mut() {
        Some(w) => writeln!(w, "{line}").and_then(|_| w.flush()),
        None => Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the engine isn't running")),
    };
    if let Err(e) = sent {
        inner.waiting.lock().unwrap().remove(&id);
        return Err(e.into());
    }
    let r = rx.recv_timeout(inner.opts.timeout);
    inner.waiting.lock().unwrap().remove(&id);
    match r {
        Ok(Response { ok: true, result, .. }) => Ok(result.unwrap_or(Value::Null)),
        Ok(Response { error, .. }) => Err(Error::Engine(error.unwrap_or_else(|| format!("{cmd} failed")))),
        Err(_) => Err(Error::Timeout),
    }
}

fn spawn(inner: &Arc<Inner>) -> Result<(), Error> {
    let mut child = Command::new(&inner.opts.path)
        .envs(inner.opts.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout");
    *inner.stdin.lock().unwrap() = child.stdin.take();
    *inner.child.lock().unwrap() = Some(child);
    let weak = Arc::downgrade(inner);
    let (hello_tx, hello_rx) = channel::<u32>();
    std::thread::spawn(move || {
        let mut hello = Some(hello_tx);
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Some(inner) = weak.upgrade() else { return };
            match serde_json::from_str::<Message>(&line) {
                Ok(Message::Response(r)) => {
                    if let Some(tx) = inner.waiting.lock().unwrap().remove(&r.id) {
                        let _ = tx.send(r);
                    }
                }
                Ok(Message::Event(e)) => {
                    if let (Event::Hello { protocol, .. }, Some(tx)) = (&e, hello.take()) {
                        let _ = tx.send(*protocol);
                    }
                    (inner.on_event)(e);
                }
                Err(_) => eprintln!("recordem-desktop: {line}"),
            }
        }
        let Some(inner) = weak.upgrade() else { return };
        let code = inner.child.lock().unwrap().as_mut().and_then(|c| c.wait().ok()).and_then(|s| s.code());
        *inner.stdin.lock().unwrap() = None;
        inner.waiting.lock().unwrap().clear();
        (inner.on_event)(Event::Shutdown { code });
        if inner.closing.load(Ordering::Relaxed) || !inner.opts.restart {
            return;
        }
        // Gone on its own: again, with the same settings, after a pause (a crash at start doesn't spin).
        std::thread::sleep(Duration::from_secs(1));
        if let Err(e) = spawn(&inner) {
            (inner.on_event)(Event::Error { kind: "restart".into(), message: e.to_string() });
            return;
        }
        let init = inner.init.lock().unwrap().clone();
        if let Some(init) = init
            && let Err(e) = call(&inner, "init", init)
        {
            (inner.on_event)(Event::Error { kind: "restart".into(), message: e.to_string() });
        }
    });
    match hello_rx.recv_timeout(inner.opts.timeout) {
        Ok(PROTOCOL) => Ok(()),
        Ok(p) => Err(Error::Protocol(p)),
        Err(_) => Err(Error::Timeout),
    }
}
