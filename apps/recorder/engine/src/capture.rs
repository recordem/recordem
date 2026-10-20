// One recording: the microphone (echo-cancelled, so the call coming out of the speakers doesn't leak into it) and
// the system audio, as two 16 kHz mono WAV files. Two channels is what tells "you" from "them" in a 1:1 call, with no
// diarization model. Capture itself is Anarlog's (audio-actual): Core Audio tap + AEC.
//
// Optionally a third file, mic_hq.wav: the same microphone at 48 kHz from a capture of its own, without echo
// cancellation (which runs at 16 kHz and colours the voice). It is the person's own voice in full quality, to listen
// back to (pronunciation, pace) — with headphones it is clean; on speakers some of the call leaks in.
use anlg_audio::{CaptureChannel, CaptureConfig, CaptureEvent};
use anlg_audio_actual::AudioInput;
use anlg_device_monitor::{DeviceSwitch, DeviceSwitchMonitor};
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub const RATE: u32 = 16_000;
pub const HQ_RATE: u32 = 48_000;

pub struct Files {
    pub mic: PathBuf,
    pub system: PathBuf,
    pub mic_hq: PathBuf,
}

pub fn files(dir: &Path) -> Files {
    Files { mic: dir.join("mic.wav"), system: dir.join("system.wav"), mic_hq: dir.join("mic_hq.wav") }
}

fn writer(p: &Path) -> anyhow::Result<hound::WavWriter<std::io::BufWriter<std::fs::File>>> {
    writer_at(p, RATE)
}

fn writer_at(p: &Path, rate: u32) -> anyhow::Result<hound::WavWriter<std::io::BufWriter<std::fs::File>>> {
    let spec = hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    Ok(hound::WavWriter::create(p, spec)?)
}

fn write(w: &mut hound::WavWriter<std::io::BufWriter<std::fs::File>>, samples: &[f32]) -> anyhow::Result<()> {
    for s in samples {
        w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    Ok(())
}

/// Records until `stop` is cancelled (or the capture fails). Returns the peak level seen on each channel, so the
/// app can warn when a channel stayed silent (usually a missing macOS permission). `live`, when given, gets a copy of
/// every frame (the live transcript); it is dropped, which ends that transcript, when the recording ends. While
/// `muted` is on, the mic is written (and sent live) as silence; its peak still counts, as a check of the permission.
/// While `paused` is on, both channels are silence: the files keep the call's clock, so the names still line up.
/// `voice_hq` adds mic_hq.wav (48 kHz, no AEC); if that capture can't open, the recording goes on without it.
/// `voices` gets the system channel as it is written, for diarize.rs.
///
/// The mic and the system audio recover apart (Anarlog's capture): when one stops, the other goes on and the missing
/// time is written as silence, so both files stay on the call's clock. On top of that, with no `mic_device` chosen
/// the mic follows the macOS default when it changes mid-call (a headset put on, a pick in Meet), and a mic that
/// hands over nothing but exact zeros — a device gone away under the stream, never a quiet room — is opened again a
/// few times, slower each time. Only the mic restarts; the call's audio never stops.
pub async fn record(dir: &Path, mic_device: Option<String>, voice_hq: bool, stop: CancellationToken, live: Option<std::sync::mpsc::Sender<crate::live::Frame>>, voices: std::sync::mpsc::Sender<Arc<[f32]>>, muted: Arc<AtomicBool>, paused: Arc<AtomicBool>) -> anyhow::Result<(f32, f32)> {
    std::fs::create_dir_all(dir)?;
    let f = files(dir);
    let mut mic = writer(&f.mic)?;
    let mut system = writer(&f.system)?;
    let mut hq_file = if voice_hq { Some(writer_at(&f.mic_hq, HQ_RATE)?) } else { None };
    let open_hq = || {
        AudioInput::from_mic_capture(mic_device.clone(), HQ_RATE, (HQ_RATE / 10) as usize)
            .map_err(|e| eprintln!("high-quality voice track unavailable: {e}"))
            .ok()
    };
    let mut hq = hq_file.as_ref().and_then(|_| open_hq());
    let mut stream = AudioInput::from_mic_and_speaker(CaptureConfig {
        sample_rate: RATE,
        chunk_size: (RATE / 10) as usize,
        mic_device: mic_device.clone(),
        enable_aec: true,
    })
    .map_err(|e| anyhow::anyhow!("couldn't open the audio: {e}"))?;
    let started = Instant::now();
    let mut written_hq = 0u64;
    let (switch_tx, switches) = std::sync::mpsc::channel();
    let _monitor = mic_device.is_none().then(|| DeviceSwitchMonitor::spawn_debounced(switch_tx));
    // The macOS default mic when the mic channel last opened (after any Bluetooth handoff it caused): a switch to the
    // same one is our own doing, not the person's.
    let mut opened_on = anlg_audio_device::default_input_device_name();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let (mut zeros, mut retries, mut retry_at) = (Duration::ZERO, 0u32, None::<Instant>);
    let (mut peak_mic, mut peak_sys) = (0f32, 0f32);
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            event = stream.next_event() => match event {
                Some(Ok(CaptureEvent::Frame(fr))) => {
                    if fr.raw_mic.iter().all(|x| *x == 0.0) {
                        zeros += Duration::from_secs_f64(fr.raw_mic.len() as f64 / RATE as f64);
                    } else {
                        (zeros, retries, retry_at) = (Duration::ZERO, 0, None);
                    }
                    let mut m = fr.preferred_mic();
                    let mine = m.iter().fold(0f32, |a, x| a.max(x.abs()));
                    peak_mic = peak_mic.max(mine);
                    if mine > 0.05 && !muted.load(Ordering::Relaxed) {
                        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
                        crate::participants::ME_AUDIBLE_AT.store(now, Ordering::Relaxed);
                    }
                    let pause = paused.load(Ordering::Relaxed);
                    if muted.load(Ordering::Relaxed) || pause {
                        m = vec![0.0; m.len()].into();
                    }
                    let loud = fr.raw_speaker.iter().fold(0f32, |a, x| a.max(x.abs()));
                    peak_sys = peak_sys.max(loud);
                    if loud > 0.02 {
                        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
                        crate::participants::REMOTE_AUDIBLE_AT.store(now, Ordering::Relaxed);
                    }
                    let speaker: Arc<[f32]> = if pause { vec![0.0; fr.raw_speaker.len()].into() } else { fr.raw_speaker.clone() };
                    write(&mut mic, &m)?;
                    write(&mut system, &speaker)?;
                    if let Some(tx) = &live {
                        let _ = tx.send(crate::live::Frame { mic: m.to_vec(), system: speaker.to_vec() });
                    }
                    let _ = voices.send(speaker);
                }
                // Both channels down at once: silence for the time they were gone.
                Some(Ok(CaptureEvent::Gap(fr))) => {
                    write(&mut mic, &fr.raw_mic)?;
                    write(&mut system, &fr.raw_speaker)?;
                    if let Some(tx) = &live {
                        let _ = tx.send(crate::live::Frame { mic: fr.raw_mic.to_vec(), system: fr.raw_speaker.to_vec() });
                    }
                    let _ = voices.send(fr.raw_speaker.clone());
                }
                Some(Ok(CaptureEvent::ChannelReady { channel, device })) => {
                    eprintln!("capture: {channel:?} ready on {}", device.as_deref().unwrap_or("the default device"));
                    if channel == CaptureChannel::Mic {
                        opened_on = anlg_audio_device::default_input_device_name();
                    }
                }
                Some(Ok(CaptureEvent::ChannelFailed { channel, error })) => eprintln!("capture: {channel:?} stopped ({error}), retrying"),
                Some(Err(e)) => anyhow::bail!("the capture stopped: {e}"),
                None => break,
            },
            frame = async { match hq.as_mut() { Some(s) => s.next().await, None => std::future::pending().await } } => match (frame, hq_file.as_mut()) {
                (Some(Ok(fr)), Some(w)) => {
                    if muted.load(Ordering::Relaxed) || paused.load(Ordering::Relaxed) {
                        write(w, &vec![0.0; fr.raw_mic.len()])?;
                    } else {
                        write(w, &fr.raw_mic)?;
                    }
                    written_hq += fr.raw_mic.len() as u64;
                }
                // The high-quality track is a bonus: when its capture fails, it stops and the recording goes on.
                (Some(Err(e)), _) => { eprintln!("high-quality voice track stopped: {e}"); hq = None; }
                _ => hq = None,
            },
            _ = tick.tick() => {
                let switched = switches.try_iter().any(|s| matches!(s, DeviceSwitch::DefaultInputChanged))
                    && anlg_audio_device::default_input_device_name() != opened_on;
                let dead = zeros >= DEAD_MIC && retry_at.is_none_or(|t| Instant::now() >= t) && retries < 4;
                if !switched && !dead {
                    continue;
                }
                eprintln!("mic: reopening ({})", if switched { "the default mic changed" } else { "it only gives zeros" });
                if dead {
                    retries += 1;
                    retry_at = Some(Instant::now() + DEAD_MIC * 3u32.pow(retries));
                }
                zeros = Duration::ZERO;
                stream.retry(mic_device.clone(), true);
                // The 48 kHz track is a capture of its own: closed first (it holds the device), opened again, and the
                // time in between written as silence.
                if let Some(w) = hq_file.as_mut() {
                    drop(hq.take());
                    hq = open_hq();
                    let due = (started.elapsed().as_secs_f64() * HQ_RATE as f64) as u64;
                    if let Some(n) = due.checked_sub(written_hq).filter(|n| *n > 0) {
                        write(w, &vec![0.0; n as usize])?;
                        written_hq += n;
                    }
                }
            }
        }
    }
    mic.finalize()?;
    system.finalize()?;
    if let Some(w) = hq_file {
        w.finalize()?;
    }
    Ok((peak_mic, peak_sys))
}

/// How long a mic may give only exact zeros before it is opened again.
const DEAD_MIC: Duration = Duration::from_secs(3);

pub fn read(p: &Path) -> anyhow::Result<Vec<f32>> {
    let mut r = hound::WavReader::open(p)?;
    Ok(r.samples::<i16>().map(|s| s.map(|v| v as f32 / i16::MAX as f32)).collect::<Result<_, _>>()?)
}

/// Both channels mixed to one track, encoded as AAC with macOS's own encoder: what the portal plays. With the
/// high-quality voice track, the mix is made at 48 kHz from it (the other side, captured at 16 kHz, upsampled), 96 kbps,
/// ~43 MB per hour: your voice sounds like you. Without it, the 16 kHz channels at 32 kbps (~14 MB per hour). Made
/// once; the mix WAV in between is removed. Whisper never reads this file, only the 16 kHz WAVs.
pub fn call_m4a(dir: &Path) -> anyhow::Result<PathBuf> {
    let out = dir.join("call.m4a");
    if out.exists() {
        return Ok(out);
    }
    let f = files(dir);
    let sys = read(&f.system)?;
    let hq = if f.mic_hq.exists() && std::fs::metadata(&f.mic_hq)?.len() > 44 { Some(read(&f.mic_hq)?) } else { None };
    let mix_path = dir.join("mix.wav");
    let (rate, bitrate) = match &hq {
        Some(mic) => {
            const K: usize = (HQ_RATE / RATE) as usize;
            // Linear interpolation is enough: the other side carries nothing above 8 kHz anyway.
            let up = |i: usize| -> f32 {
                let (j, frac) = (i / K, (i % K) as f32 / K as f32);
                let a = sys.get(j).copied().unwrap_or(0.0);
                let b = sys.get(j + 1).copied().unwrap_or(a);
                a + (b - a) * frac
            };
            let mut w = writer_at(&mix_path, HQ_RATE)?;
            let n = mic.len().max(sys.len() * K);
            let mixed: Vec<f32> = (0..n).map(|i| mic.get(i).copied().unwrap_or(0.0) + up(i)).collect();
            write(&mut w, &mixed)?;
            w.finalize()?;
            ("aac@48000", "96000")
        }
        None => {
            let mic = read(&f.mic)?;
            let mut w = writer(&mix_path)?;
            let mixed: Vec<f32> = (0..mic.len().max(sys.len()))
                .map(|i| mic.get(i).copied().unwrap_or(0.0) + sys.get(i).copied().unwrap_or(0.0))
                .collect();
            write(&mut w, &mixed)?;
            w.finalize()?;
            ("aac", "32000")
        }
    };
    let st = std::process::Command::new("/usr/bin/afconvert")
        .args(["-f", "m4af", "-d", rate, "-b", bitrate])
        .arg(&mix_path)
        .arg(&out)
        .status()?;
    let _ = std::fs::remove_file(&mix_path);
    anyhow::ensure!(st.success(), "afconvert failed ({st})");
    Ok(out)
}

/// The high-quality voice track (mic_hq.wav) as AAC at 48 kHz, 128 kbps (.m4a, ~58 MB per hour); None when this
/// recording has none. Made once.
pub fn voice_m4a(dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    let out = dir.join("voice.m4a");
    if out.exists() {
        return Ok(Some(out));
    }
    let src = files(dir).mic_hq;
    if !src.exists() || std::fs::metadata(&src)?.len() <= 44 {
        return Ok(None);
    }
    let st = std::process::Command::new("/usr/bin/afconvert")
        .args(["-f", "m4af", "-d", "aac@48000", "-b", "128000"])
        .arg(&src)
        .arg(&out)
        .status()?;
    anyhow::ensure!(st.success(), "afconvert failed ({st})");
    Ok(Some(out))
}

pub fn mics() -> Vec<String> {
    AudioInput::list_mic_devices()
}
