// Opens the microphone the way capture.rs does and prints how loud it is: to tell a capture problem from a permission
// or device one. `--child` runs the same probe in a child process (as an app's sidecar would).
//   cargo run -p recordem-engine --example mic-probe -- [--child] [device name]
use anlg_audio::{CaptureConfig, CaptureEvent};
use anlg_audio_actual::AudioInput;
use std::time::Duration;

#[tokio::main]
async fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--child") {
        args.remove(0);
        let st = std::process::Command::new(std::env::current_exe().unwrap()).args(&args).status();
        println!("child: {st:?}");
        return;
    }
    let device = args.first().cloned();
    println!("pid {} device {device:?}", std::process::id());
    // 48 kHz, its own capture: mic_hq.
    match tokio::time::timeout(Duration::from_secs(10), tokio::task::spawn_blocking({
        let d = device.clone();
        move || AudioInput::from_mic_capture(d, 48_000, 4_800).map(|_| ())
    }))
    .await
    {
        Ok(Ok(r)) => println!("48 kHz mic open: {:?}", r.map_err(|e| e.to_string())),
        _ => println!("48 kHz mic open: hung for 10 s"),
    }
    // 16 kHz mic + system audio with echo cancellation: mic.wav and system.wav.
    let mut s = match AudioInput::from_mic_and_speaker(CaptureConfig { sample_rate: 16_000, chunk_size: 1_600, mic_device: device, enable_aec: true }) {
        Ok(s) => s,
        Err(e) => return println!("mic+speaker open: {e}"),
    };
    let (mut mic, mut sys, mut n) = (0f32, 0f32, 0);
    let end = tokio::time::Instant::now() + Duration::from_secs(8);
    while let Ok(Some(ev)) = tokio::time::timeout_at(end, s.next_event()).await {
        match ev {
            Ok(CaptureEvent::Frame(f)) => {
                n += 1;
                mic = mic.max(f.raw_mic.iter().fold(0f32, |a, x| a.max(x.abs())));
                sys = sys.max(f.raw_speaker.iter().fold(0f32, |a, x| a.max(x.abs())));
            }
            Ok(CaptureEvent::ChannelReady { channel, device }) => println!("ready: {channel:?} on {device:?}"),
            Ok(CaptureEvent::ChannelFailed { channel, error }) => println!("failed: {channel:?}: {error}"),
            Ok(_) => {}
            Err(e) => println!("error: {e}"),
        }
    }
    println!("frames {n} mic peak {mic:.4} system peak {sys:.4}");
}
