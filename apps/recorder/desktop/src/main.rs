// recordem-desktop: the Recordem engine for any app to ship and drive (the Desktop SDK). One JSON object per line:
// requests on stdin, responses and events on stdout, logs on stderr (desktop-client/src/protocol.rs has the shapes).
// The first line out is `hello`. When stdin closes (the host quit) or on `shutdown`, the recording going on is
// stopped and processed, then the engine exits.
use recordem_desktop::{Event, Message, PROTOCOL, Request, Response, Settings, StartRecording};
use recordem_engine::Engine;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowId {
    window_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Token {
    recording_id: String,
    upload_token: String,
}

#[derive(Deserialize)]
struct Muted {
    muted: bool,
}

#[derive(Deserialize)]
struct Perm {
    permission: recordem_desktop::Permission,
}

static READY: AtomicBool = AtomicBool::new(false);

fn params<T: DeserializeOwned>(v: Value) -> anyhow::Result<T> {
    serde_json::from_value(if v.is_null() { json!({}) } else { v }).map_err(|e| anyhow::anyhow!("bad params: {e}"))
}

async fn handle(engine: &Engine, cmd: &str, p: Value) -> anyhow::Result<Value> {
    let needs_init = !matches!(cmd, "init" | "shutdown" | "permissions" | "requestPermission" | "openPermissionSettings" | "listDevices");
    anyhow::ensure!(!needs_init || READY.load(Ordering::Relaxed), "{cmd}: init first");
    Ok(match cmd {
        "init" => {
            let r = engine.init(&params::<Settings>(p)?)?;
            READY.store(true, Ordering::Relaxed);
            serde_json::to_value(r)?
        }
        "configure" => {
            engine.configure(&params::<Settings>(p)?);
            Value::Null
        }
        "startRecording" => json!({ "recordingId": engine.start_recording(&params::<StartRecording>(p)?)? }),
        "stopRecording" => engine.stop_recording(&params::<WindowId>(p)?.window_id).map(|_| Value::Null)?,
        "pauseRecording" => engine.pause_recording(&params::<WindowId>(p)?.window_id).map(|_| Value::Null)?,
        "resumeRecording" => engine.resume_recording(&params::<WindowId>(p)?.window_id).map(|_| Value::Null)?,
        "setUploadToken" => {
            let t: Token = params(p)?;
            engine.set_upload_token(&t.recording_id, &t.upload_token)?;
            Value::Null
        }
        "prepareDesktopAudioRecording" => json!({ "windowId": engine.prepare_desktop_audio_recording() }),
        "permissions" => serde_json::to_value(engine.permissions())?,
        "requestPermission" => {
            engine.request_permission(params::<Perm>(p)?.permission).await;
            Value::Null
        }
        "openPermissionSettings" => {
            engine.open_permission_settings(params::<Perm>(p)?.permission);
            Value::Null
        }
        "setMuted" => {
            engine.set_muted(params::<Muted>(p)?.muted);
            Value::Null
        }
        "listDevices" => serde_json::to_value(engine.list_devices())?,
        "downloadModels" => {
            engine.download_models();
            Value::Null
        }
        "pending" => serde_json::to_value(engine.pending())?,
        // Answered right away; the engine exits once the recording going on is processed (quit).
        "shutdown" => Value::Null,
        _ => anyhow::bail!("unknown command {cmd}"),
    })
}

fn line(m: &Message) -> String {
    serde_json::to_string(m).expect("message")
}

async fn quit(engine: Engine, out: Sender<String>) {
    engine.shutdown().await;
    let _ = out.send(line(&Message::Event(Event::Shutdown { code: Some(0) })));
    // Time for the writer thread to empty its queue.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    std::process::exit(0);
}

fn main() {
    // stdout is the protocol: one writer, lines whole.
    let (out, lines) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut w = std::io::stdout().lock();
        for l in lines {
            if l.is_empty() {
                continue;
            }
            if writeln!(w, "{l}").and_then(|_| w.flush()).is_err() {
                break;
            }
        }
    });
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("runtime");
    let engine = {
        let _g = rt.enter();
        let out = out.clone();
        Engine::new(move |e| {
            let _ = out.send(line(&Message::Event(e)));
        })
    };
    let _ = out.send(line(&Message::Event(Event::Hello { protocol: PROTOCOL, version: recordem_engine::VERSION.into() })));
    let handle = rt.handle().clone();
    std::thread::spawn(move || {
        for l in std::io::stdin().lock().lines() {
            let Ok(l) = l else { break };
            if l.trim().is_empty() {
                continue;
            }
            let req: Request = match serde_json::from_str(&l) {
                Ok(r) => r,
                Err(e) => {
                    let _ = out.send(line(&Message::Event(Event::Error { kind: "protocol".into(), message: format!("not a request: {e}") })));
                    continue;
                }
            };
            // In order, one at a time (`init` before what follows it); a permission prompt waits for the person, so
            // it alone runs on the side.
            let (engine, out) = (engine.clone(), out.clone());
            let (shutdown, side) = (req.cmd == "shutdown", req.cmd == "requestPermission");
            let work = async move {
                let r = handle_one(&engine, req).await;
                let _ = out.send(line(&Message::Response(r)));
                if shutdown {
                    quit(engine, out).await;
                }
            };
            if side {
                handle.spawn(work);
            } else {
                handle.block_on(work);
            }
        }
        // The host is gone: finish what was recorded, then go.
        handle.spawn(quit(engine, out));
    });
    run_loop(rt);
}

async fn handle_one(engine: &Engine, req: Request) -> Response {
    match handle(engine, &req.cmd, req.params).await {
        Ok(result) => Response { id: req.id, ok: true, result: (!result.is_null()).then_some(result), error: None },
        Err(e) => Response { id: req.id, ok: false, result: None, error: Some(e.to_string()) },
    }
}

/// The main thread runs a Core Foundation run loop, as an app's would: Core Audio, Apple Events and the list of running
/// apps (the call app's mute button) are delivered through it.
#[cfg(target_os = "macos")]
fn run_loop(rt: tokio::runtime::Runtime) {
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRunLoopRun();
    }
    let _keep = rt;
    unsafe { CFRunLoopRun() };
}

#[cfg(not(target_os = "macos"))]
fn run_loop(rt: tokio::runtime::Runtime) {
    rt.block_on(std::future::pending::<()>());
}
