// Local transcription: whisper.cpp (Metal) transcribes each turn of each channel, and the two channels are merged by
// time. mic = whoever runs the recorder: Silero VAD finds its turns. system = everyone else on the call: Anarlog's
// pyannote diarization (CoreML) finds who spoke when, so each turn is one voice and a line never mixes two people.
use crate::api::Segment;
use crate::models;
use anlg_pyannote::{DiarizationConfig, DiarizeRequest, Diarizer, SpeakerSegment};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use whisper_rs::{get_lang_id, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperVadContext, WhisperVadContextParams, WhisperVadParams, WhisperState};

static CTX: OnceLock<WhisperContext> = OnceLock::new();

pub(crate) fn ctx() -> anyhow::Result<&'static WhisperContext> {
    if let Some(c) = CTX.get() {
        return Ok(c);
    }
    let path = models::path(&models::WHISPER);
    let c = WhisperContext::new_with_params(path.to_str().unwrap(), WhisperContextParameters::default())
        .map_err(|e| anyhow::anyhow!("couldn't load the model: {e}"))?;
    Ok(CTX.get_or_init(|| c))
}

/// Speech in a channel as sample ranges: Silero VAD, then turns closer than MERGE_GAP joined up to MAX_CHUNK.
/// (whisper.cpp's own VAD option returns timestamps with the silence removed, which loses where a turn was.)
fn speech(audio: &[f32]) -> anyhow::Result<Vec<(usize, usize)>> {
    speech_in(&mut vad()?, audio)
}

pub(crate) fn vad() -> anyhow::Result<WhisperVadContext> {
    let path = models::path(&models::VAD);
    WhisperVadContext::new(path.to_str().unwrap(), WhisperVadContextParams::new()).map_err(|e| anyhow::anyhow!("vad: {e}"))
}

/// `speech` with a VAD context the caller keeps (the live transcript runs it every second).
pub(crate) fn speech_in(ctx: &mut WhisperVadContext, audio: &[f32]) -> anyhow::Result<Vec<(usize, usize)>> {
    const RATE: usize = crate::capture::RATE as usize;
    const MERGE_GAP: usize = RATE;          // 1 s
    const MAX_CHUNK: usize = 25 * RATE;     // whisper's window is 30 s
    const PAD: usize = RATE / 5;            // 200 ms around each turn
    let mut v = WhisperVadParams::new();
    v.set_threshold(0.5);
    v.set_min_silence_duration(300);
    v.set_speech_pad(100);
    let segs = ctx.segments_from_samples(v, audio).map_err(|e| anyhow::anyhow!("vad: {e}"))?;
    let mut out: Vec<(usize, usize)> = vec![];
    for seg in segs {
        // centiseconds → samples
        let a = ((seg.start * (RATE as f32) / 100.0) as usize).saturating_sub(PAD);
        let b = (((seg.end * (RATE as f32) / 100.0) as usize) + PAD).min(audio.len());
        if b <= a {
            continue;
        }
        match out.last_mut() {
            Some(last) if a <= last.1 + MERGE_GAP && b - last.0 <= MAX_CHUNK => last.1 = b.max(last.1),
            _ => out.push((a, b)),
        }
    }
    // A single turn longer than a window still has to fit one: cut it.
    Ok(out.into_iter().flat_map(|(a, b)| (a..b).step_by(MAX_CHUNK).map(move |x| (x, (x + MAX_CHUNK).min(b)))).collect())
}

/// The system channel's turns by voice: (start, end, speaker), speakers numbered from 0 by first appearance. Turns of one
/// voice closer than MERGE_GAP are joined up to MAX_CHUNK; the padding never reaches into a neighbouring turn of another
/// voice (it would put their words in this line). None when diarization finds no one, or fails: then VAD, no voices.
fn voices(audio: &[f32], progress: &Arc<AtomicU32>, base: f32, weight: f32) -> Option<Vec<(usize, usize, usize)>> {
    const RATE: usize = crate::capture::RATE as usize;
    const MERGE_GAP: usize = RATE;          // 1 s
    const MAX_CHUNK: usize = 25 * RATE;     // whisper's window is 30 s
    const PAD: usize = RATE * 15 / 100;     // 150 ms
    let on_progress = |f: f32| progress.store(((base + weight * f) * 1000.0) as u32, Ordering::Relaxed);
    let mut src = audio;
    let found = Diarizer::new(DiarizationConfig::default())
        .and_then(|mut d| d.diarize(&mut src, &DiarizeRequest { on_progress: Some(&on_progress), ..Default::default() }));
    let mut segs: Vec<SpeakerSegment> = match found {
        Ok(d) if !d.segments.is_empty() => d.segments,
        Ok(_) => return None,
        Err(e) => {
            eprintln!("diarization failed, falling back to VAD: {e}");
            return None;
        }
    };
    segs.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut order: Vec<usize> = vec![];
    let mut turns: Vec<(usize, usize, usize)> = vec![];
    for s in segs {
        let k = order.iter().position(|&x| x == s.speaker).unwrap_or_else(|| { order.push(s.speaker); order.len() - 1 });
        let a = ((s.start * RATE as f64) as usize).min(audio.len());
        let b = ((s.end * RATE as f64) as usize).min(audio.len());
        if b <= a {
            continue;
        }
        match turns.last_mut() {
            Some(last) if last.2 == k && a <= last.1 + MERGE_GAP && b - last.0 <= MAX_CHUNK => last.1 = b.max(last.1),
            _ => turns.push((a, b, k)),
        }
    }
    let padded: Vec<(usize, usize, usize)> = (0..turns.len()).map(|i| {
        let (a, b, k) = turns[i];
        let floor = turns[..i].iter().rev().find(|t| t.2 != k).map_or(0, |t| t.1.min(a));
        let ceil = turns[i + 1..].iter().find(|t| t.2 != k).map_or(audio.len(), |t| t.0.max(b));
        (a.saturating_sub(PAD).max(floor), (b + PAD).min(ceil), k)
    }).collect();
    // A single turn longer than a window still has to fit one: cut it.
    Some(padded.into_iter().flat_map(|(a, b, k)| (a..b).step_by(MAX_CHUNK).map(move |x| (x, (x + MAX_CHUNK).min(b), k))).collect())
}

/// Which of `langs` a turn is in. One language: that one, no detection. Several: Whisper's language detection over the
/// turn, restricted to `langs`; a short turn ("ok", "yeah") or an unsure one keeps the channel's previous language.
pub(crate) fn pick(state: &mut WhisperState, audio: &[f32], langs: &[String], prev: Option<&str>, threads: usize) -> anyhow::Result<String> {
    const RATE: usize = crate::capture::RATE as usize;
    const MIN_DETECT: usize = 2 * RATE; // shorter than this, detection is a coin toss
    const SURE: f32 = 0.7;              // share of the allowed languages' probability the winner needs
    if langs.len() == 1 {
        return Ok(langs[0].clone());
    }
    if let Some(p) = prev.filter(|_| audio.len() < MIN_DETECT) {
        return Ok(p.into());
    }
    state.pcm_to_mel(audio, threads).map_err(|e| anyhow::anyhow!("whisper: {e}"))?;
    let (_, probs) = state.lang_detect(0, threads).map_err(|e| anyhow::anyhow!("whisper: {e}"))?;
    let scored: Vec<(&String, f32)> = langs.iter()
        .map(|l| (l, get_lang_id(l).and_then(|i| probs.get(i as usize).copied()).unwrap_or(0.0)))
        .collect();
    let sum: f32 = scored.iter().map(|(_, p)| p).sum::<f32>().max(f32::EPSILON);
    let (best, p) = scored.into_iter().fold((&langs[0], -1.0), |a, b| if b.1 > a.1 { b } else { a });
    Ok(match prev {
        Some(prev) if p / sum < SURE => prev.into(),
        _ => best.clone(),
    })
}

/// `turns`: (start, end, speaker label) in samples. `progress` is in thousandths of the whole job; this channel covers
/// [base, base + weight).
fn channel(audio: &[f32], turns: Vec<(usize, usize, Option<String>)>, langs: &[String], vocabulary: &str, name: &str, progress: &Arc<AtomicU32>, base: f32, weight: f32) -> anyhow::Result<Vec<Segment>> {
    const RATE: usize = crate::capture::RATE as usize;
    let total: usize = turns.iter().map(|(a, b, _)| b - a).sum::<usize>().max(1);
    let mut state = ctx()?.create_state().map_err(|e| anyhow::anyhow!("whisper: {e}"))?;
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    let mut out = vec![];
    // The language each voice spoke last: a short turn keeps it (people don't switch on "ok").
    let mut prev: HashMap<Option<String>, String> = HashMap::new();
    let mut done = 0usize;
    for (a, b, speaker) in turns {
        if b - a < RATE / 2 {
            continue;
        }
        let lang = pick(&mut state, &audio[a..b], langs, prev.get(&speaker).map(String::as_str), threads)?;
        out.extend(turn(&mut state, &audio[a..b], a, &lang, vocabulary, name, speaker.as_deref(), threads)?);
        prev.insert(speaker, lang);
        done += b - a;
        progress.store(((base + weight * done as f32 / total as f32) * 1000.0) as u32, Ordering::Relaxed);
    }
    Ok(out)
}

/// One turn through Whisper, in `lang`. `at` is where the turn starts in the channel, in samples: the segments' times
/// are from the start of the recording.
pub(crate) fn turn(state: &mut WhisperState, audio: &[f32], at: usize, lang: &str, vocabulary: &str, name: &str, speaker: Option<&str>, threads: usize) -> anyhow::Result<Vec<Segment>> {
    const RATE: usize = crate::capture::RATE as usize;
    let prompt = prompt(vocabulary, lang);
    let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    p.set_language(Some(lang));
    p.set_n_threads(threads as i32);
    p.set_print_progress(false);
    p.set_print_realtime(false);
    p.set_print_special(false);
    p.set_print_timestamps(false);
    p.set_suppress_blank(true);
    p.set_suppress_nst(true);
    p.set_no_context(true);
    p.set_no_speech_thold(0.6);
    if !prompt.is_empty() {
        p.set_initial_prompt(&prompt);
    }
    state.full(p, audio).map_err(|e| anyhow::anyhow!("whisper: {e}"))?;
    let offset_ms = (at * 1000 / RATE) as u64;
    let mut out = vec![];
    for seg in state.as_iter() {
        let text = seg.to_str_lossy().map(|s| s.trim().to_string()).unwrap_or_default();
        if text.is_empty() || seg.no_speech_probability() > 0.8 {
            continue;
        }
        // whisper timestamps are centiseconds from the start of this chunk
        out.push(Segment {
            start_ms: offset_ms + (seg.start_timestamp().max(0) as u64) * 10,
            end_ms: offset_ms + (seg.end_timestamp().max(0) as u64) * 10,
            channel: name.into(),
            text,
            language: Some(lang.into()),
            speaker: speaker.map(String::from),
        });
    }
    Ok(out)
}

/// Whisper's initial prompt: the person's own words (settings), nothing else. Whisper reads only the last ~224 tokens
/// of it; keep it short. Nothing about a meeting or a person goes in: Whisper sometimes echoes prompt words into the
/// text over silence.
fn prompt(vocabulary: &str, _lang: &str) -> String {
    let mut out = String::new();
    for t in vocabulary.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if out.len() + t.len() > 600 {
            break;
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(t);
    }
    if out.is_empty() { out } else { format!("{out}.") }
}

/// Both channels, merged by start time. `progress` goes 0..1000 over the whole job.
/// `languages`: the ones the calls may be in, comma-separated ("pt,en"); each turn is transcribed in one of them.
pub fn run(mic: &[f32], system: &[f32], languages: &str, vocabulary: &str, progress: &Arc<AtomicU32>) -> anyhow::Result<Vec<Segment>> {
    let langs = crate::config::languages(languages);
    let total = (mic.len() + system.len()).max(1) as f32;
    let wm = mic.len() as f32 / total;
    let mic_turns = speech(mic)?.into_iter().map(|(a, b)| (a, b, Some("mic".to_string()))).collect();
    let mut segs = channel(mic, mic_turns, &langs, vocabulary, "mic", progress, 0.0, wm)?;
    // The other side: about a third of its time goes to telling the voices apart, the rest to Whisper.
    let ws = 1.0 - wm;
    let system_turns = match voices(system, progress, wm, ws / 3.0) {
        Some(v) => v.into_iter().map(|(a, b, k)| (a, b, Some(format!("system:{}", k + 1)))).collect(),
        None => speech(system)?.into_iter().map(|(a, b)| (a, b, None)).collect(),
    };
    segs.extend(channel(system, system_turns, &langs, vocabulary, "system", progress, wm + ws / 3.0, ws * 2.0 / 3.0)?);
    segs.sort_by_key(|s| s.start_ms);
    progress.store(1000, Ordering::Relaxed);
    Ok(segs)
}
