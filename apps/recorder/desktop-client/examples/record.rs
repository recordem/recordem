// Records with recordem-desktop and prints every event: the smallest host there is.
//
//   cargo run -p recordem-desktop --example record -- <path to recordem-desktop> <api url> <data dir> <upload token> [seconds]
//
// With no call going on it records "desktop audio" (prepareDesktopAudioRecording) for `seconds` (20), then waits
// until the recording is sent.
use recordem_desktop::{Client, Event, Options, Settings, StartRecording, UploadStatus};
use std::sync::mpsc::channel;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [exe, api, data, token, ..] = a.as_slice() else {
        return Err("usage: record <recordem-desktop> <api url> <data dir> <upload token> [seconds]".into());
    };
    let secs: u64 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(20);
    let (done_tx, done) = channel();
    let client = Client::start(Options::new(exe), move |e| {
        println!("{}", serde_json::to_string(&e).unwrap());
        if let Event::UploadStatus { status: UploadStatus::Done | UploadStatus::Failed | UploadStatus::Waiting, .. } = e {
            let _ = done_tx.send(());
        }
    })?;
    // RECORDEM_VOICE_HQ=0: without the 48 kHz voice track. RECORDEM_MIC=<name>: that microphone (see listDevices).
    let voice_hq = std::env::var("RECORDEM_VOICE_HQ").map_or(true, |v| v != "0");
    let mic_device = std::env::var("RECORDEM_MIC").ok();
    let ready = client.init(&Settings { api_url: Some(api.clone()), data_dir: Some(data.clone()), detect: Some(false), voice_hq: Some(voice_hq), mic_device, ..Default::default() })?;
    println!("ready: {}", serde_json::to_string(&ready)?);
    let window = client.prepare_desktop_audio_recording()?;
    let id = client.start_recording(&StartRecording { window_id: window.clone(), upload_token: Some(token.clone()), ..Default::default() })?;
    println!("recording {id} for {secs} s");
    std::thread::sleep(Duration::from_secs(secs));
    client.stop_recording(&window)?;
    let _ = done.recv_timeout(Duration::from_secs(600));
    client.shutdown()?;
    Ok(())
}
