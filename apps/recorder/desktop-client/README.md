# recordem-desktop

Record calls from your Rust or Tauri app with [Recordem](https://github.com/recordem/recordem): the client of the
`recordem-desktop` engine, the same one `@recordem/desktop-sdk` drives from Node and the Recordem app itself records
with. The engine notices calls, records the microphone and the call's audio apart, reads who is in the call, transcribes
on the computer and sends it all to Recordem with an upload token your backend got for that one recording.

```rust
use recordem_desktop::{Client, Event, Options, Settings, StartRecording};

let client = Client::start(Options::new(engine_path), |e| match e {
    Event::MeetingDetected { window } => println!("a call in {:?}", window.app_name),
    Event::RealtimeEvent { event, data, .. } if event == "transcript.data" => println!("{}", data["text"]),
    _ => {}
})?;
client.init(&Settings { api_url: Some("https://api.recordem.dev".into()), data_dir: Some(data_dir), ..Default::default() })?;
let window = client.prepare_desktop_audio_recording()?;
client.start_recording(&StartRecording { window_id: window, upload_token: Some(token), ..Default::default() })?;
```

In a Tauri app, ship the engine as a sidecar (`bundle.externalBin`); it is signed with your app and records in its name.
`cargo run -p recordem-desktop --example record -- <engine> <api url> <data dir> <upload token>` is a whole host in
40 lines. The protocol (one JSON object per line over stdin/stdout) is in `src/protocol.rs`.

MIT
