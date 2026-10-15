// asr-bench (cargo run --release --features bench --bin asr-bench -- …): the same audio through several engines, the way the recorder runs them (Silero VAD turns, then the
// engine per turn), to pick one with numbers instead of impressions.
//
//   asr-bench call.wav whisper parakeet parakeet:ultra --lang pt,en --out dir
//
// For each engine: load time, transcription time and speed (audio seconds per second), the text in <dir>/<engine>.txt,
// and how much of its text every other engine agrees with (word level).
use std::time::Instant;

pub fn main(args: Vec<String>) -> anyhow::Result<()> {
    let mut files = vec![];
    let mut engines = vec![];
    let mut langs = "pt,en".to_string();
    let mut out = std::path::PathBuf::from("asr-bench-out");
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--lang" => langs = it.next().unwrap_or(langs),
            "--out" => out = it.next().map(Into::into).unwrap_or(out),
            f if f.ends_with(".wav") => files.push(f.to_string()),
            e => engines.push(e.to_string()),
        }
    }
    anyhow::ensure!(files.len() == 1 && !engines.is_empty(), "usage: asr-bench <16 kHz mono .wav> <engine>... [--lang pt,en] [--out dir]");
    std::fs::create_dir_all(&out)?;
    let audio = crate::capture::read(std::path::Path::new(&files[0]))?;
    let secs = audio.len() as f64 / crate::capture::RATE as f64;
    let turns = crate::transcribe::speech_in(&mut crate::transcribe::vad()?, &audio)?;
    let speech: f64 = turns.iter().map(|(a, b)| (b - a) as f64).sum::<f64>() / crate::capture::RATE as f64;
    println!("{}: {secs:.0} s, {} turns, {speech:.0} s of speech", files[0], turns.len());
    let langs = crate::config::languages(&langs);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    let mut texts: Vec<(String, String)> = vec![];
    for setting in &engines {
        let t0 = Instant::now();
        let mut engine = crate::asr::open(setting, threads)?;
        let load = t0.elapsed().as_secs_f64();
        let t1 = Instant::now();
        let mut lines = vec![];
        let mut prev: Option<String> = None;
        for &(a, b) in &turns {
            let lang = engine.language(&audio[a..b], &langs, prev.as_deref())?;
            for seg in crate::transcribe::turn(engine.as_mut(), &audio[a..b], a, &lang, "", "mix", None)? {
                lines.push(format!("[{:>6.1}s {lang}] {}", seg.start_ms as f64 / 1000.0, seg.text));
            }
            prev = Some(lang);
        }
        let took = t1.elapsed().as_secs_f64();
        let file = out.join(format!("{}.txt", setting.replace(':', "-")));
        std::fs::write(&file, lines.join("\n"))?;
        println!("{:<16} load {load:>5.1} s   transcribe {took:>6.1} s   {:>5.0}x real time   → {}", engine.name(), secs / took, file.display());
        let text = lines.iter().map(|l| l.split_once("] ").map_or(l.as_str(), |x| x.1)).collect::<Vec<_>>().join(" ");
        texts.push((setting.clone(), text));
    }
    for i in 0..texts.len() {
        for j in i + 1..texts.len() {
            println!("agreement {} / {}: {:.0}%", texts[i].0, texts[j].0, 100.0 * agreement(&texts[i].1, &texts[j].1));
        }
    }
    Ok(())
}

fn words(s: &str) -> Vec<String> {
    s.split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'').to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Words in common, in order (longest common subsequence), over the average length: 1.0 is the same text.
fn agreement(a: &str, b: &str) -> f64 {
    let (a, b) = (words(a), words(b));
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let mut row = vec![0u32; b.len() + 1];
    for x in &a {
        let mut diag = 0;
        for (j, y) in b.iter().enumerate() {
            let up = row[j + 1];
            row[j + 1] = if x == y { diag + 1 } else { up.max(row[j]) };
            diag = up;
        }
    }
    2.0 * row[b.len()] as f64 / (a.len() + b.len()) as f64
}

#[cfg(test)]
mod tests {
    #[test]
    fn agreement_is_word_overlap() {
        assert_eq!(super::agreement("Olá, tudo bem?", "olá tudo bem"), 1.0);
        assert_eq!(super::agreement("a b c d", "a x c d"), 0.75);
    }
}
