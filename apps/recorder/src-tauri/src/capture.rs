// One recording: the microphone (echo-cancelled, so the call coming out of the speakers doesn't leak into it) and
// the system audio, as two 16 kHz mono WAV files. Two channels is what tells "you" from "them" in a 1:1 call, with no
// diarization model. Capture itself is Anarlog's (audio-actual): Core Audio tap + AEC.
//
// Optionally a third file, mic_hq.wav: the same microphone at 48 kHz from a capture of its own, without echo
// cancellation (which runs at 16 kHz and colours the voice). It is the person's own voice in full quality, to listen
// back to (pronunciation, pace) — with headphones it is clean; on speakers some of the call leaks in.
use anlg_audio::CaptureConfig;
use anlg_audio_actual::AudioInput;
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
/// `voice_hq` adds mic_hq.wav (48 kHz, no AEC); if that capture can't open, the recording goes on without it.
pub async fn record(dir: &Path, mic_device: Option<String>, voice_hq: bool, stop: CancellationToken, live: Option<std::sync::mpsc::Sender<crate::live::Frame>>, muted: Arc<AtomicBool>) -> anyhow::Result<(f32, f32)> {
    std::fs::create_dir_all(dir)?;
    let f = files(dir);
    let mut mic = writer(&f.mic)?;
    let mut system = writer(&f.system)?;
    let mut hq = None;
    if voice_hq {
        match AudioInput::from_mic_capture(mic_device.clone(), HQ_RATE, (HQ_RATE / 10) as usize) {
            Ok(s) => hq = Some((s, writer_at(&f.mic_hq, HQ_RATE)?)),
            Err(e) => eprintln!("high-quality voice track unavailable: {e}"),
        }
    }
    let mut stream = AudioInput::from_mic_and_speaker(CaptureConfig {
        sample_rate: RATE,
        chunk_size: (RATE / 10) as usize,
        mic_device,
        enable_aec: true,
    })
    .map_err(|e| anyhow::anyhow!("couldn't open the audio: {e}"))?;
    let (mut peak_mic, mut peak_sys) = (0f32, 0f32);
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            frame = stream.next() => match frame {
                Some(Ok(fr)) => {
                    let mut m = fr.preferred_mic();
                    peak_mic = m.iter().fold(peak_mic, |a, x| a.max(x.abs()));
                    if muted.load(Ordering::Relaxed) {
                        m = vec![0.0; m.len()].into();
                    }
                    peak_sys = fr.raw_speaker.iter().fold(peak_sys, |a, x| a.max(x.abs()));
                    write(&mut mic, &m)?;
                    write(&mut system, &fr.raw_speaker)?;
                    if let Some(tx) = &live {
                        let _ = tx.send(crate::live::Frame { mic: m.to_vec(), system: fr.raw_speaker.to_vec() });
                    }
                }
                Some(Err(e)) => anyhow::bail!("the capture stopped: {e}"),
                None => break,
            },
            frame = async { match hq.as_mut() { Some((s, _)) => s.next().await, None => std::future::pending().await } } => match frame {
                Some(Ok(fr)) => {
                    let w = &mut hq.as_mut().expect("hq").1;
                    if muted.load(Ordering::Relaxed) {
                        write(w, &vec![0.0; fr.raw_mic.len()])?;
                    } else {
                        write(w, &fr.raw_mic)?;
                    }
                }
                // The high-quality track is a bonus: when its capture fails, it stops and the recording goes on.
                Some(Err(e)) => { eprintln!("high-quality voice track stopped: {e}"); if let Some((_, w)) = hq.take() { w.finalize()?; } }
                None => { if let Some((_, w)) = hq.take() { w.finalize()?; } }
            }
        }
    }
    mic.finalize()?;
    system.finalize()?;
    if let Some((_, w)) = hq {
        w.finalize()?;
    }
    Ok((peak_mic, peak_sys))
}

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
