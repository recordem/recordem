// Local transcription: an engine (asr.rs: Whisper or Parakeet, a setting) transcribes each turn of each channel, and
// the two channels are merged by time. mic = whoever runs the recorder: Silero VAD finds its turns. system = everyone
// else on the call: Anarlog's pyannote diarization (CoreML) finds who spoke when, so each turn is one voice and a line
// never mixes two people.
use crate::api::Segment;
use crate::asr::Engine;
use crate::models;
use anlg_pyannote::SpeakerSegment;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

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
/// `found`: what diarize.rs found during the call; diarized here only without it.
fn voices(audio: &[f32], found: Option<Vec<SpeakerSegment>>, progress: &Arc<AtomicU32>, base: f32, weight: f32) -> Option<Vec<(usize, usize, usize)>> {
    const RATE: usize = crate::capture::RATE as usize;
    const MERGE_GAP: usize = RATE;          // 1 s
    const MAX_CHUNK: usize = 25 * RATE;     // whisper's window is 30 s
    const PAD: usize = RATE * 15 / 100;     // 150 ms
    let mut segs: Vec<SpeakerSegment> = match found {
        Some(segs) if !segs.is_empty() => segs,
        Some(_) => return None,
        // The same pipeline as during the call (diarize.rs: one voice at a time), over the whole file.
        None => match whole(audio, progress, base, weight) {
            Some(segs) if !segs.is_empty() => segs,
            _ => return None,
        },
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

/// The voices of the whole channel, when nothing was found during the call; None when that fails.
fn whole(audio: &[f32], progress: &Arc<AtomicU32>, base: f32, weight: f32) -> Option<Vec<SpeakerSegment>> {
    let run = || -> anyhow::Result<Vec<SpeakerSegment>> {
        let mut d = crate::diarize::Streaming::new()?;
        let chunks = audio.chunks(10 * crate::capture::RATE as usize);
        let n = chunks.len().max(1) as f32;
        for (i, c) in chunks.enumerate() {
            d.push(c)?;
            progress.store(((base + weight * (i + 1) as f32 / n) * 1000.0) as u32, Ordering::Relaxed);
        }
        d.finish()
    };
    run().map_err(|e| eprintln!("diarization failed, falling back to VAD: {e}")).ok()
}

/// `turns`: (start, end, speaker label) in samples. `progress` is in thousandths of the whole job; this channel covers
/// [base, base + weight).
fn channel(engine: &mut dyn Engine, audio: &[f32], turns: Vec<(usize, usize, Option<String>)>, langs: &[String], vocabulary: &str, name: &str, progress: &Arc<AtomicU32>, base: f32, weight: f32) -> anyhow::Result<Vec<Segment>> {
    const RATE: usize = crate::capture::RATE as usize;
    let total: usize = turns.iter().map(|(a, b, _)| b - a).sum::<usize>().max(1);
    let mut out = vec![];
    // The language each voice spoke last: a short turn keeps it (people don't switch on "ok").
    let mut prev: HashMap<Option<String>, String> = HashMap::new();
    let mut done = 0usize;
    for (a, b, speaker) in turns {
        if b - a < RATE / 2 {
            continue;
        }
        let lang = engine.language(&audio[a..b], langs, prev.get(&speaker).map(String::as_str))?;
        out.extend(turn(engine, &audio[a..b], a, &lang, vocabulary, name, speaker.as_deref())?);
        prev.insert(speaker, lang);
        done += b - a;
        progress.store(((base + weight * done as f32 / total as f32) * 1000.0) as u32, Ordering::Relaxed);
    }
    Ok(out)
}

/// One turn through the engine, in `lang`. `at` is where the turn starts in the channel, in samples: the segments'
/// times are from the start of the recording.
pub(crate) fn turn(engine: &mut dyn Engine, audio: &[f32], at: usize, lang: &str, vocabulary: &str, name: &str, speaker: Option<&str>) -> anyhow::Result<Vec<Segment>> {
    const RATE: usize = crate::capture::RATE as usize;
    let offset_ms = (at * 1000 / RATE) as u64;
    Ok(engine
        .transcribe(audio, lang, vocabulary)?
        .into_iter()
        .map(|p| Segment {
            start_ms: offset_ms + p.start_ms,
            end_ms: offset_ms + p.end_ms,
            channel: name.into(),
            text: p.text,
            language: Some(lang.into()),
            speaker: speaker.map(String::from),
        })
        .collect())
}

/// How the other side's words get their speaker. `Turns`: each voice's turn (the diarization's) is transcribed on its
/// own. `Words`: the speech is transcribed in long stretches, as one hears it, and each word goes to the voice
/// speaking when it was said: no word is cut at a turn's edge, nor lost where two voices overlap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Assign {
    Turns,
    Words,
}

/// Both channels, merged by start time. `progress` goes 0..1000 over the whole job.
/// `languages`: the ones the calls may be in, comma-separated ("pt,en"); each turn is transcribed in one of them.
/// `engine`: the setting (asr::open), e.g. "whisper".
pub fn run(mic: &[f32], system: &[f32], found: Option<Vec<SpeakerSegment>>, languages: &str, vocabulary: &str, engine: &str, progress: &Arc<AtomicU32>) -> anyhow::Result<Vec<Segment>> {
    run_with(mic, system, found, languages, vocabulary, engine, progress, Assign::Turns)
}

#[allow(clippy::too_many_arguments)]
pub fn run_with(mic: &[f32], system: &[f32], found: Option<Vec<SpeakerSegment>>, languages: &str, vocabulary: &str, engine: &str, progress: &Arc<AtomicU32>, assign: Assign) -> anyhow::Result<Vec<Segment>> {
    let langs = crate::config::languages(languages);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    let mut engine = crate::asr::open(engine, threads)?;
    let total = (mic.len() + system.len()).max(1) as f32;
    let wm = mic.len() as f32 / total;
    let mic_turns = speech(mic)?.into_iter().map(|(a, b)| (a, b, Some("mic".to_string()))).collect();
    let mut segs = channel(engine.as_mut(), mic, mic_turns, &langs, vocabulary, "mic", progress, 0.0, wm)?;
    // The other side: about a third of its time goes to telling the voices apart, the rest to the engine.
    let ws = 1.0 - wm;
    if assign == Assign::Words {
        let voices = match found {
            Some(v) => v,
            None => whole(system, progress, wm, ws / 3.0).unwrap_or_default(),
        };
        segs.extend(by_word(engine.as_mut(), system, &voices, &langs, vocabulary, progress, wm + ws / 3.0, ws * 2.0 / 3.0)?);
    } else {
        let system_turns = match voices(system, found, progress, wm, ws / 3.0) {
            Some(v) => v.into_iter().map(|(a, b, k)| (a, b, Some(format!("system:{}", k + 1)))).collect(),
            None => speech(system)?.into_iter().map(|(a, b)| (a, b, None)).collect(),
        };
        segs.extend(channel(engine.as_mut(), system, system_turns, &langs, vocabulary, "system", progress, wm + ws / 3.0, ws * 2.0 / 3.0)?);
    }
    segs.sort_by_key(|s| s.start_ms);
    progress.store(1000, Ordering::Relaxed);
    Ok(segs)
}

/// The system channel by word: its speech (VAD, up to 25 s at a time) through the engine word by word, each word
/// given to the voice that overlaps it most (or the nearest one), then the words of one voice in a row made into lines.
/// A stretch is cut only where the language changes: each voice's turns in it get a language (as `channel` does), and
/// the engine hears each part in its own.
#[allow(clippy::too_many_arguments)]
fn by_word(engine: &mut dyn Engine, audio: &[f32], voices: &[SpeakerSegment], langs: &[String], vocabulary: &str, progress: &Arc<AtomicU32>, base: f32, weight: f32) -> anyhow::Result<Vec<Segment>> {
    const RATE: usize = crate::capture::RATE as usize;
    let stretches = speech(audio)?;
    let total: usize = stretches.iter().map(|(a, b)| b - a).sum::<usize>().max(1);
    let mut done = 0usize;
    // The language each voice spoke last: a short turn keeps it.
    let mut prev: HashMap<Option<usize>, String> = HashMap::new();
    let mut out = vec![];
    for (a, b) in stretches {
        if b - a < RATE / 2 {
            continue;
        }
        let mut parts: Vec<(usize, usize, String)> = vec![];
        let turns = voices.iter().filter_map(|v| {
            let (s, e) = (((v.start * RATE as f64) as usize).max(a), ((v.end * RATE as f64) as usize).min(b));
            (e > s).then_some((s, e, Some(v.speaker)))
        });
        let turns: Vec<_> = turns.collect();
        for (s, e, who) in if turns.is_empty() { vec![(a, b, None)] } else { turns } {
            let lang = engine.language(&audio[s..e], langs, prev.get(&who).map(String::as_str))?;
            prev.insert(who, lang.clone());
            match parts.last_mut() {
                Some(p) if p.2 == lang => p.1 = e.max(p.1),
                Some(p) => {
                    p.1 = s.max(p.0); // the new language starts with its turn
                    parts.push((s, e, lang));
                }
                None => parts.push((a, e, lang)),
            }
        }
        if let Some(p) = parts.last_mut() {
            p.1 = b;
        }
        for (s, e, lang) in parts {
            if e - s < RATE / 4 {
                continue;
            }
            let offset = (s * 1000 / RATE) as u64;
            let words: Vec<crate::asr::Word> = engine
                .words(&audio[s..e], &lang, vocabulary)?
                .into_iter()
                .map(|w| crate::asr::Word { start_ms: offset + w.start_ms, end_ms: offset + w.end_ms, ..w })
                .collect();
            let spans: Vec<(f64, f64)> = words.iter().map(|w| (w.start_ms as f64 / 1000.0, w.end_ms as f64 / 1000.0)).collect();
            let who = anlg_pyannote::assign_words(&spans, voices);
            let mut i = 0;
            while i < words.len() {
                let j = (i..words.len()).find(|&j| who[j] != who[i]).unwrap_or(words.len());
                let speaker = who[i].map(|k| format!("system:{}", k + 1));
                for p in crate::asr::sentences(&words[i..j]) {
                    out.push(Segment { start_ms: p.start_ms, end_ms: p.end_ms, channel: "system".into(), text: p.text, language: Some(lang.clone()), speaker: speaker.clone() });
                }
                i = j;
            }
        }
        done += b - a;
        progress.store(((base + weight * done as f32 / total as f32) * 1000.0) as u32, Ordering::Relaxed);
    }
    Ok(out)
}
