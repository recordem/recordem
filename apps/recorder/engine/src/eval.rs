// eval (cargo run --release --features bench --bin eval -- …): calls through the recorder's own pipeline, without a
// call, to measure a change before it ships. A fixture is a folder laid out like a recording's (mic.wav, system.wav,
// both 16 kHz mono; either may be missing), so a recording kept with "Keep audio on this Mac" is one as it is:
//
//   eval ~/recordem-eval/public/* ~/recordem-eval/private/* --out after.json --baseline before.json
//
// Each fixture goes through what a call does: the live draft (live.rs, fed as fast as it takes it), the diarization
// that runs during the call (diarize.rs), then the pass after it (transcribe::run). With a reference next to the audio
// it is scored: reference.rttm (who spoke when on the system channel: DER) and/or reference.json (what was said:
// WER; its system-channel turns also give the DER when there is no .rttm). Without one there are still the times,
// the voices found, and the lines the final transcript says twice.
//
// Next to the report, <report>.d/<fixture>.txt holds each final transcript (and .live.txt the draft), to read what
// changed when a number moves.
//
// --diarize-only skips the speech recognition (DER only, in seconds), and --set name=value changes a diarization
// setting (DiarizationConfig: min_cluster_seconds, clustering_threshold, …) without a rebuild, to sweep one.
//
// --overlapping lets voices overlap, as before diarization went one voice at a time.
// --speakers max (or exact) bounds the voices by the reference's count, as the call app's participant list could.
//
// --baseline compares with an earlier report and exits 1 when something got worse (DER or WER up more than a point,
// more repeated lines, or a stage 25% slower), so a change can be kept or dropped on numbers.
use crate::api::Segment;
use anlg_pyannote::SpeakerSegment;
use anlg_pyannote::metrics::{DerReport, ReferenceTurn, diarization_error_rate};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Instant;

const RATE: usize = crate::capture::RATE as usize;
const COLLAR: f64 = 0.25; // seconds around each reference boundary that aren't scored (pyannote's convention)

#[derive(Serialize, Deserialize, Default, Clone)]
struct Report {
    engine: String,
    #[serde(default)]
    settings: Vec<String>,
    fixtures: Vec<Scored>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct Scored {
    name: String,
    audio_s: f64,
    /// Seconds each stage took; `final_s` includes diarizing the system channel (as the pass after the call does when
    /// nothing was found during it), `diarize_s` is the during-the-call diarization alone.
    live_s: Option<f64>,
    diarize_s: f64,
    final_s: f64,
    voices: usize,
    /// Seconds where the diarization has two voices at once: each of them is transcribed, so the same words twice.
    overlap_s: f64,
    lines: usize,
    /// Lines of the final transcript that repeat a line of the same channel they overlap in time.
    repeated: usize,
    der: Option<Der>,
    wer: Option<f64>,
    live_wer: Option<f64>,
    /// How much of the live draft's text the final transcript agrees with (word level, 1.0 = the same).
    live_agreement: Option<f64>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct Der {
    der: f64,
    missed: f64,
    false_alarm: f64,
    confusion: f64,
    reference_voices: usize,
}

pub fn main(args: Vec<String>) -> anyhow::Result<()> {
    let mut fixtures: Vec<PathBuf> = vec![];
    let mut engine = "whisper".to_string();
    let mut langs = "en,pt".to_string();
    let mut data = dirs::data_dir().unwrap_or_default().join("Recordem Dev");
    let mut out = PathBuf::from("eval.json");
    let mut baseline: Option<PathBuf> = None;
    let mut live = true;
    let mut asr = true;
    let mut settings: Vec<String> = vec![];
    let mut speakers: Option<String> = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--engine" => engine = it.next().unwrap_or(engine),
            "--lang" => langs = it.next().unwrap_or(langs),
            "--data" => data = it.next().map(Into::into).unwrap_or(data),
            "--out" => out = it.next().map(Into::into).unwrap_or(out),
            "--baseline" => baseline = it.next().map(Into::into),
            "--no-live" => live = false,
            "--diarize-only" => (live, asr) = (false, false),
            "--set" => settings.extend(it.next()),
            "--speakers" => speakers = it.next(),
            "--overlapping" => settings.push("overlapping=1".into()),
            f => fixtures.push(f.into()),
        }
    }
    anyhow::ensure!(
        !fixtures.is_empty(),
        "usage: eval <fixture dir>... [--engine whisper] [--lang en,pt] [--data <app data dir>] [--out eval.json] [--baseline earlier.json] [--no-live] [--diarize-only] [--set name=value]..."
    );
    // The models the app downloaded (Recordem Dev's by default).
    crate::config::apply(&recordem_desktop::protocol::Settings { data_dir: Some(data.to_string_lossy().into()), ..Default::default() });
    anyhow::ensure!(crate::models::ready(), "no models in {}: open the app once, or pass --data", data.display());

    if let Some(b) = &speakers {
        anyhow::ensure!(b == "max" || b == "exact", "--speakers max | exact");
        settings.push(format!("speakers={b}"));
    }
    let config = diarization(&settings)?;
    let overlapping = settings.iter().any(|s| s == "overlapping=1");
    let mut report = Report { engine: if asr { crate::asr::describe(&engine) } else { "diarization only".into() }, settings, fixtures: vec![] };
    let texts = out.with_extension("d");
    std::fs::create_dir_all(&texts)?;
    for dir in &fixtures {
        let name = dir.file_name().map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().into());
        match fixture(dir, &name, &engine, &langs, live, asr, &config, speakers.as_deref(), overlapping, &texts) {
            Ok(s) => {
                print_row(&s);
                report.fixtures.push(s);
            }
            Err(e) => eprintln!("{name}: skipped ({e:#})"),
        }
    }
    print_totals(&report);
    std::fs::write(&out, serde_json::to_string_pretty(&report)?)?;
    println!("report: {}", out.display());
    if let Some(b) = baseline {
        let before: Report = serde_json::from_slice(&std::fs::read(&b)?)?;
        if !compare(&before, &report) {
            std::process::exit(1);
        }
    }
    Ok(())
}

/// The default diarization settings with `name=value` overrides.
fn diarization(settings: &[String]) -> anyhow::Result<anlg_pyannote::DiarizationConfig> {
    let mut c = anlg_pyannote::DiarizationConfig::default();
    for s in settings.iter().filter(|s| !s.starts_with("speakers=") && !s.starts_with("overlapping=")) {
        let (k, v) = s.split_once('=').ok_or_else(|| anyhow::anyhow!("--set {s}: name=value"))?;
        let f: f32 = v.parse()?;
        match k {
            "step_seconds" => c.step_seconds = f,
            "min_embedding_seconds" => c.min_embedding_seconds = f,
            "min_activity_seconds" => c.min_activity_seconds = f,
            "clustering_threshold" => c.clustering_threshold = f,
            "min_cluster_seconds" => c.min_cluster_seconds = f,
            "min_duration_on" => c.min_duration_on = f,
            "min_duration_off" => c.min_duration_off = f,
            _ => anyhow::bail!("--set {k}: not a diarization setting"),
        }
    }
    Ok(c)
}

#[allow(clippy::too_many_arguments)]
fn fixture(dir: &Path, name: &str, engine: &str, langs: &str, live: bool, asr: bool, config: &anlg_pyannote::DiarizationConfig, speakers: Option<&str>, overlapping: bool, texts: &Path) -> anyhow::Result<Scored> {
    let f = crate::capture::files(dir);
    let read = |p: &Path| if p.exists() { crate::capture::read(p) } else { Ok(vec![]) };
    let (mut mic, mut system) = (read(&f.mic)?, read(&f.system)?);
    anyhow::ensure!(!mic.is_empty() || !system.is_empty(), "no mic.wav or system.wav");
    // A missing channel is silence for the length of the other, as when nobody talks on it.
    let len = mic.len().max(system.len());
    mic.resize(len, 0.0);
    system.resize(len, 0.0);
    let audio_s = len as f64 / RATE as f64;
    eprintln!("{name}: {audio_s:.0} s");

    let (live_segs, live_s) = if live {
        let t = Instant::now();
        let segs = live_draft(&mic, &system, langs, engine)?;
        (Some(segs), Some(t.elapsed().as_secs_f64()))
    } else {
        (None, None)
    };

    let t = Instant::now();
    let reference = Reference::read(dir)?;
    let mut d = crate::diarize::Streaming::with(config.clone())?;
    d.exclusive = !overlapping;
    if let (Some(b), Some(turns)) = (speakers, &reference.turns) {
        let n = turns.iter().map(|t| &t.speaker).collect::<std::collections::HashSet<_>>().len();
        d.bounds = if b == "exact" { anlg_pyannote::SpeakerBounds::exact(n) } else { anlg_pyannote::SpeakerBounds { max_speakers: Some(n), ..Default::default() } };
    }
    for chunk in system.chunks(RATE / 10) {
        d.push(chunk)?;
    }
    let found = d.finish()?;
    let diarize_s = t.elapsed().as_secs_f64();

    let t = Instant::now();
    let progress = Arc::new(AtomicU32::new(0));
    let segs = if asr { crate::transcribe::run(&mic, &system, Some(found.clone()), langs, "", engine, &progress)? } else { vec![] };
    let final_s = t.elapsed().as_secs_f64();

    std::fs::write(texts.join(format!("{name}.txt")), lines(&segs))?;
    if let Some(l) = &live_segs {
        std::fs::write(texts.join(format!("{name}.live.txt")), lines(l))?;
    }

    let der = reference.turns.as_ref().map(|r| {
        let d = diarization_error_rate(r, &found, COLLAR);
        Der { der: d.der(), missed: d.missed, false_alarm: d.false_alarm, confusion: d.confusion, reference_voices: d.reference_speakers }
    });
    let text = |s: &[Segment]| s.iter().map(|x| x.text.as_str()).collect::<Vec<_>>().join(" ");
    let wer = reference.text.as_deref().filter(|_| asr).map(|r| word_error_rate(r, &text(&segs)));
    let live_wer = reference.text.as_deref().zip(live_segs.as_deref()).map(|(r, l)| word_error_rate(r, &text(l)));
    let live_agreement = live_segs.as_deref().map(|l| crate::bench::agreement(&text(l), &text(&segs)));
    Ok(Scored {
        name: name.into(),
        audio_s,
        live_s,
        diarize_s,
        final_s,
        voices: found.iter().map(|s| s.speaker + 1).max().unwrap_or(0),
        overlap_s: overlap(&found),
        lines: segs.len(),
        repeated: repeated(&segs),
        der,
        wer,
        live_wer,
        live_agreement,
    })
}

/// The live draft as the recorder makes it: 100 ms frames into live::transcribe, its batches collected.
fn live_draft(mic: &[f32], system: &[f32], langs: &str, engine: &str) -> anyhow::Result<Vec<Segment>> {
    let (tx, rx) = std::sync::mpsc::sync_channel(64);
    let (out, mut segs) = tokio::sync::mpsc::unbounded_channel();
    let (langs, engine) = (langs.to_string(), engine.to_string());
    let worker = std::thread::spawn(move || crate::live::transcribe(rx, out, &langs, "", &engine));
    for (m, s) in mic.chunks(RATE / 10).zip(system.chunks(RATE / 10)) {
        if tx.send(crate::live::Frame { mic: m.to_vec(), system: s.to_vec() }).is_err() {
            break;
        }
    }
    drop(tx);
    worker.join().map_err(|_| anyhow::anyhow!("the live draft panicked"))??;
    let mut all = vec![];
    while let Ok(batch) = segs.try_recv() {
        all.extend(batch);
    }
    Ok(all)
}

/// `[m:ss.s] channel speaker: text`, a line per segment.
fn lines(segs: &[Segment]) -> String {
    segs.iter()
        .map(|s| {
            let t = s.start_ms as f64 / 1000.0;
            format!("[{}:{:04.1}] {} {}: {}\n", (t / 60.0) as u64, t % 60.0, s.channel, s.speaker.as_deref().unwrap_or("?"), s.text.trim())
        })
        .collect()
}

/// What a fixture says really happened, when it says anything.
struct Reference {
    turns: Option<Vec<ReferenceTurn>>,
    text: Option<String>,
}

/// reference.json: what a synthetic call was made from (eval/synth), or a hand-checked transcript.
#[derive(Deserialize)]
struct ReferenceFile {
    segments: Vec<ReferenceSegment>,
}

#[derive(Deserialize)]
struct ReferenceSegment {
    start: f64,
    end: f64,
    speaker: String,
    /// "mic" or "system"; the diarization only sees the system channel, so only its turns score it.
    #[serde(default = "system")]
    channel: String,
    #[serde(default)]
    text: String,
}

fn system() -> String {
    "system".into()
}

impl Reference {
    fn read(dir: &Path) -> anyhow::Result<Self> {
        let mut r = Reference { turns: None, text: None };
        let rttm = dir.join("reference.rttm");
        if rttm.exists() {
            r.turns = Some(parse_rttm(&std::fs::read_to_string(rttm)?));
        }
        let json = dir.join("reference.json");
        if json.exists() {
            let f: ReferenceFile = serde_json::from_slice(&std::fs::read(json)?)?;
            let mut segs = f.segments;
            segs.sort_by(|a, b| a.start.total_cmp(&b.start));
            let text: Vec<&str> = segs.iter().map(|s| s.text.as_str()).filter(|t| !t.is_empty()).collect();
            if !text.is_empty() {
                r.text = Some(text.join(" "));
            }
            if r.turns.is_none() {
                r.turns = Some(
                    segs.iter()
                        .filter(|s| s.channel == "system")
                        .map(|s| ReferenceTurn { start: s.start, end: s.end, speaker: s.speaker.clone() })
                        .collect(),
                );
            }
        }
        Ok(r)
    }
}

/// `SPEAKER <file> 1 <start> <duration> <NA> <NA> <speaker> <NA> <NA>`
fn parse_rttm(s: &str) -> Vec<ReferenceTurn> {
    s.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.first() != Some(&"SPEAKER") || f.len() < 8 {
                return None;
            }
            let (start, dur): (f64, f64) = (f[3].parse().ok()?, f[4].parse().ok()?);
            Some(ReferenceTurn { start, end: start + dur, speaker: f[7].to_string() })
        })
        .collect()
}

/// Seconds covered by two or more voices at once.
fn overlap(segs: &[SpeakerSegment]) -> f64 {
    let mut edges: Vec<(f64, i32)> = segs.iter().flat_map(|s| [(s.start, 1), (s.end, -1)]).collect();
    edges.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let (mut active, mut since, mut total) = (0, 0.0, 0.0);
    for (t, d) in edges {
        if active >= 2 {
            total += t - since;
        }
        active += d;
        since = t;
    }
    total
}

/// Lines that overlap an earlier line of the same channel in time and say mostly the same words.
fn repeated(segs: &[Segment]) -> usize {
    let mut n = 0;
    for (i, s) in segs.iter().enumerate() {
        let twice = segs[..i].iter().rev().take(8).any(|p| {
            p.channel == s.channel && p.start_ms < s.end_ms && s.start_ms < p.end_ms && crate::bench::agreement(&p.text, &s.text) >= 0.8
        });
        n += twice as usize;
    }
    n
}

/// Word edits (substitutions, insertions, deletions) to turn `hyp` into `reference`, over the reference's words.
fn word_error_rate(reference: &str, hyp: &str) -> f64 {
    let (r, h) = (crate::bench::words(reference), crate::bench::words(hyp));
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    let mut row: Vec<usize> = (0..=h.len()).collect();
    for (i, x) in r.iter().enumerate() {
        let mut diag = row[0];
        row[0] = i + 1;
        for (j, y) in h.iter().enumerate() {
            let up = row[j + 1];
            row[j + 1] = if x == y { diag } else { 1 + diag.min(up).min(row[j]) };
            diag = up;
        }
    }
    row[h.len()] as f64 / r.len() as f64
}

fn pct(x: Option<f64>) -> String {
    x.map_or("—".into(), |v| format!("{:.1}%", v * 100.0))
}

fn print_row(s: &Scored) {
    println!(
        "{:<28} {:>6.0}s  live {:>6}  diarize {:>5.1}s  final {:>6.1}s ({:>3.0}x)  voices {:>2}/{:<2}  overlap {:>5.1}s  repeated {:>3}/{:<4}  DER {:>6}  WER {:>6}  live WER {:>6}  live≈final {:>6}",
        s.name,
        s.audio_s,
        s.live_s.map_or("—".into(), |x| format!("{x:.1}s")),
        s.diarize_s,
        s.final_s,
        s.audio_s / s.final_s.max(1e-9),
        s.voices,
        s.der.as_ref().map_or("?".into(), |d| d.reference_voices.to_string()),
        s.overlap_s,
        s.repeated,
        s.lines,
        pct(s.der.as_ref().map(|d| d.der)),
        pct(s.wer),
        pct(s.live_wer),
        pct(s.live_agreement),
    );
}

fn print_totals(r: &Report) {
    let audio: f64 = r.fixtures.iter().map(|s| s.audio_s).sum();
    let fin: f64 = r.fixtures.iter().map(|s| s.final_s).sum();
    let mut der = DerReport::default();
    for d in r.fixtures.iter().filter_map(|s| s.der.as_ref()) {
        // Totals weight each fixture by its speech: rebuilt from the components, which are in seconds.
        let total = (d.missed + d.false_alarm + d.confusion) / d.der.max(1e-12);
        der.accumulate(&DerReport { total, missed: d.missed, false_alarm: d.false_alarm, confusion: d.confusion, ..Default::default() });
    }
    println!(
        "{} fixtures, {:.0} min of audio, {}, DER {}, repeated lines {}  ({})",
        r.fixtures.len(),
        audio / 60.0,
        if fin > 0.01 { format!("final pass {:.0}x real time", audio / fin) } else { format!("diarization {:.0}x real time", audio / r.fixtures.iter().map(|s| s.diarize_s).sum::<f64>().max(1e-9)) },
        if der.total > 0.0 { format!("{:.1}%", der.der() * 100.0) } else { "—".into() },
        r.fixtures.iter().map(|s| s.repeated).sum::<usize>(),
        r.engine,
    );
}

/// Prints what changed per fixture; false when anything got worse.
fn compare(before: &Report, after: &Report) -> bool {
    let mut ok = true;
    for a in &after.fixtures {
        let Some(b) = before.fixtures.iter().find(|b| b.name == a.name) else { continue };
        let mut notes = vec![];
        let mut worse = |what: String, bad: bool| {
            if bad {
                ok = false;
            }
            notes.push(format!("{}{what}", if bad { "✗ " } else { "" }));
        };
        let points = |x: Option<f64>, y: Option<f64>| x.zip(y).map(|(x, y)| (y - x) * 100.0);
        if let Some(d) = points(b.der.as_ref().map(|d| d.der), a.der.as_ref().map(|d| d.der)) {
            worse(format!("DER {d:+.1}"), d > 1.0);
        }
        if let Some(d) = points(b.wer, a.wer) {
            worse(format!("WER {d:+.1}"), d > 1.0);
        }
        if a.repeated != b.repeated {
            worse(format!("repeated {} → {}", b.repeated, a.repeated), a.repeated > b.repeated);
        }
        let slower = |x: f64, y: f64| y > x * 1.25 && y - x > 2.0;
        worse(format!("final {:.1}s → {:.1}s", b.final_s, a.final_s), slower(b.final_s, a.final_s));
        worse(format!("diarize {:.1}s → {:.1}s", b.diarize_s, a.diarize_s), slower(b.diarize_s, a.diarize_s));
        println!("{:<28} {}", a.name, notes.join("  "));
    }
    println!("{}", if ok { "no regressions" } else { "regressions: see ✗" });
    ok
}

#[cfg(test)]
mod tests {
    #[test]
    fn word_errors() {
        assert_eq!(super::word_error_rate("a b c d", "a b c d"), 0.0);
        assert_eq!(super::word_error_rate("a b c d", "a x c"), 0.5);
    }

    #[test]
    fn rttm() {
        let t = super::parse_rttm("SPEAKER f 1 1.500 2.000 <NA> <NA> A <NA> <NA>\n");
        assert_eq!((t[0].start, t[0].end, t[0].speaker.as_str()), (1.5, 3.5, "A"));
    }

    #[test]
    fn overlap_seconds() {
        let s = |start, end, speaker| anlg_pyannote::SpeakerSegment { start, end, speaker };
        assert_eq!(super::overlap(&[s(0.0, 2.0, 0), s(1.5, 3.0, 1), s(4.0, 5.0, 0)]), 0.5);
    }
}
