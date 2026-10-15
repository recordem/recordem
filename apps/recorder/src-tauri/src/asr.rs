// Speech recognition engines behind one interface, so the model is a setting: Whisper (whisper.cpp, Metal) and
// Parakeet (FluidAudio, CoreML on the Neural Engine, through the recordem-parakeet helper). The live draft and the pass
// after the call each pick one (config `asr_live`, `asr_final`); `asr-bench` runs any of them on the same audio.
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use whisper_rs::{get_lang_id, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

const RATE: usize = crate::capture::RATE as usize;

/// Text of a turn, with times in ms from the turn's start.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

pub trait Engine: Send {
    /// What the transcript says it was made with ("whisper large-v3-turbo q5_0", "parakeet v3").
    fn name(&self) -> String;
    /// Which of `langs` a turn is in; `prev` is the voice's last one (a short "ok" keeps it).
    fn language(&mut self, audio: &[f32], langs: &[String], prev: Option<&str>) -> anyhow::Result<String>;
    /// One turn, 16 kHz mono.
    fn transcribe(&mut self, audio: &[f32], lang: &str, vocabulary: &str) -> anyhow::Result<Vec<Piece>>;
}

/// An engine by its setting: "whisper", "parakeet" (v3) or "parakeet:<model>" (v3, ultra, redux). `threads`: what
/// Whisper may use (the live draft leaves half the Mac to the call).
pub fn open(setting: &str, threads: usize) -> anyhow::Result<Box<dyn Engine>> {
    match setting.split_once(':').unwrap_or((setting, "")) {
        ("whisper", _) | ("", _) => Ok(Box::new(Whisper::new(threads)?)),
        ("parakeet", model) => Ok(Box::new(Parakeet::new(if model.is_empty() { "v3" } else { model })?)),
        (other, _) => anyhow::bail!("unknown speech recognition engine {other:?}"),
    }
}

/// The engine's name for a setting, without loading it.
pub fn describe(setting: &str) -> String {
    match setting.split_once(':').unwrap_or((setting, "")) {
        ("parakeet", "") => "parakeet v3".into(),
        ("parakeet", model) => format!("parakeet {model}"),
        _ => "whisper large-v3-turbo q5_0".into(),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Whisper

static WHISPER: OnceLock<WhisperContext> = OnceLock::new();

pub(crate) fn whisper_ctx() -> anyhow::Result<&'static WhisperContext> {
    if let Some(c) = WHISPER.get() {
        return Ok(c);
    }
    let path = crate::models::path(&crate::models::WHISPER);
    let c = WhisperContext::new_with_params(path.to_str().unwrap(), WhisperContextParameters::default())
        .map_err(|e| anyhow::anyhow!("couldn't load the model: {e}"))?;
    Ok(WHISPER.get_or_init(|| c))
}

pub struct Whisper {
    state: WhisperState,
    threads: usize,
}

impl Whisper {
    pub fn new(threads: usize) -> anyhow::Result<Whisper> {
        let state = whisper_ctx()?.create_state().map_err(|e| anyhow::anyhow!("whisper: {e}"))?;
        Ok(Whisper { state, threads })
    }
}

impl Engine for Whisper {
    fn name(&self) -> String {
        "whisper large-v3-turbo q5_0".into()
    }

    /// One language: that one, no detection. Several: Whisper's language detection over the turn, restricted to
    /// `langs`; a short turn ("ok", "yeah") or an unsure one keeps the voice's previous language.
    fn language(&mut self, audio: &[f32], langs: &[String], prev: Option<&str>) -> anyhow::Result<String> {
        const MIN_DETECT: usize = 2 * RATE; // shorter than this, detection is a coin toss
        const SURE: f32 = 0.7;              // share of the allowed languages' probability the winner needs
        if langs.len() == 1 {
            return Ok(langs[0].clone());
        }
        if let Some(p) = prev.filter(|_| audio.len() < MIN_DETECT) {
            return Ok(p.into());
        }
        self.state.pcm_to_mel(audio, self.threads).map_err(|e| anyhow::anyhow!("whisper: {e}"))?;
        let (_, probs) = self.state.lang_detect(0, self.threads).map_err(|e| anyhow::anyhow!("whisper: {e}"))?;
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

    fn transcribe(&mut self, audio: &[f32], lang: &str, vocabulary: &str) -> anyhow::Result<Vec<Piece>> {
        let prompt = prompt(vocabulary);
        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        p.set_language(Some(lang));
        p.set_n_threads(self.threads as i32);
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
        self.state.full(p, audio).map_err(|e| anyhow::anyhow!("whisper: {e}"))?;
        let mut out = vec![];
        for seg in self.state.as_iter() {
            let text = seg.to_str_lossy().map(|s| s.trim().to_string()).unwrap_or_default();
            if text.is_empty() || seg.no_speech_probability() > 0.8 {
                continue;
            }
            // whisper timestamps are centiseconds from the start of this chunk
            out.push(Piece {
                start_ms: (seg.start_timestamp().max(0) as u64) * 10,
                end_ms: (seg.end_timestamp().max(0) as u64) * 10,
                text,
            });
        }
        Ok(out)
    }
}

/// Whisper's initial prompt: the person's own words (settings), nothing else. Whisper reads only the last ~224 tokens
/// of it; keep it short. Nothing about a meeting or a person goes in: Whisper sometimes echoes prompt words into the
/// text over silence.
fn prompt(vocabulary: &str) -> String {
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

// ---------------------------------------------------------------------------------------------------------------
// Parakeet

/// The helper process (apps/recorder/parakeet): the models load once (~480 MB, on the Neural Engine) and every engine
/// of the same model shares it, turn by turn.
struct Helper {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

#[derive(serde::Deserialize)]
struct Reply {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    words: Vec<Word>,
}

#[derive(serde::Deserialize, Debug, Clone)]
struct Word {
    text: String,
    start_ms: u64,
    end_ms: u64,
}

/// Where the helper is: $RECORDEM_PARAKEET, or next to the recorder's executable (bundled as a sidecar).
fn helper_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("RECORDEM_PARAKEET") {
        return p.into();
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.with_file_name("recordem-parakeet")
}

impl Helper {
    fn start(model: &str) -> anyhow::Result<Helper> {
        let dir = crate::config::models_dir().join("parakeet");
        std::fs::create_dir_all(&dir)?;
        let mut child = Command::new(helper_path())
            .args(["--model", model, "--models"])
            .arg(&dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("parakeet helper ({}): {e}", helper_path().display()))?;
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        // The first start downloads the models: no timeout here, the caller shows its own progress.
        let mut line = String::new();
        stdout.read_line(&mut line)?;
        let ready: serde_json::Value = serde_json::from_str(&line).map_err(|_| anyhow::anyhow!("parakeet helper: {line}"))?;
        anyhow::ensure!(ready["ready"] == true, "parakeet helper: {}", ready["error"]);
        Ok(Helper { _child: child, stdin, stdout })
    }

    fn turn(&mut self, audio: &[f32], lang: &str) -> anyhow::Result<Vec<Word>> {
        let header = serde_json::json!({"samples": audio.len(), "language": lang});
        let mut bytes = Vec::with_capacity(audio.len() * 4);
        for s in audio {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        writeln!(self.stdin, "{header}")?;
        self.stdin.write_all(&bytes)?;
        self.stdin.flush()?;
        let mut line = String::new();
        anyhow::ensure!(self.stdout.read_line(&mut line)? > 0, "parakeet helper exited");
        let reply: Reply = serde_json::from_str(&line)?;
        match reply.error {
            Some(e) => anyhow::bail!("parakeet: {e}"),
            None => Ok(reply.words),
        }
    }
}

static HELPERS: OnceLock<Mutex<Vec<(String, Arc<Mutex<Helper>>)>>> = OnceLock::new();

pub struct Parakeet {
    model: String,
    helper: Arc<Mutex<Helper>>,
}

impl Parakeet {
    pub fn new(model: &str) -> anyhow::Result<Parakeet> {
        let mut all = HELPERS.get_or_init(Default::default).lock().unwrap();
        let helper = match all.iter().find(|(m, _)| m == model) {
            Some((_, h)) => h.clone(),
            None => {
                let h = Arc::new(Mutex::new(Helper::start(model)?));
                all.push((model.to_string(), h.clone()));
                h
            }
        };
        Ok(Parakeet { model: model.to_string(), helper })
    }
}

impl Engine for Parakeet {
    fn name(&self) -> String {
        format!("parakeet {}", self.model)
    }

    /// Parakeet picks the language itself and can't be told one; the voice's last language (or the first allowed)
    /// is only a hint for the script.
    fn language(&mut self, _audio: &[f32], langs: &[String], prev: Option<&str>) -> anyhow::Result<String> {
        Ok(prev.map(String::from).unwrap_or_else(|| langs[0].clone()))
    }

    fn transcribe(&mut self, audio: &[f32], lang: &str, _vocabulary: &str) -> anyhow::Result<Vec<Piece>> {
        let words = self.helper.lock().unwrap().turn(audio, lang)?;
        Ok(sentences(&words))
    }
}

/// Words into lines: a line ends at a sentence's end, a pause of more than a second, or past 25 s.
fn sentences(words: &[Word]) -> Vec<Piece> {
    const PAUSE_MS: u64 = 1_000;
    const MAX_MS: u64 = 25_000;
    let mut out: Vec<Piece> = vec![];
    let mut open = false;
    for w in words {
        let text = w.text.trim();
        if text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(p) if open && w.start_ms <= p.end_ms + PAUSE_MS && w.end_ms - p.start_ms <= MAX_MS => {
                p.text.push(' ');
                p.text.push_str(text);
                p.end_ms = w.end_ms;
            }
            _ => out.push(Piece { start_ms: w.start_ms, end_ms: w.end_ms, text: text.to_string() }),
        }
        open = !text.ends_with(['.', '?', '!', '…']);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, start_ms: u64, end_ms: u64) -> Word {
        Word { text: text.into(), start_ms, end_ms }
    }

    #[test]
    fn words_become_sentences() {
        let words = [w("Olá,", 0, 300), w("tudo", 400, 600), w("bem?", 700, 900), w("Sim.", 1000, 1200), w("Depois", 4000, 4300), w("disso", 4400, 4600)];
        let got: Vec<_> = sentences(&words).into_iter().map(|p| (p.text, p.start_ms, p.end_ms)).collect();
        assert_eq!(got, [
            ("Olá, tudo bem?".to_string(), 0, 900),
            ("Sim.".to_string(), 1000, 1200),
            ("Depois disso".to_string(), 4000, 4600),
        ]);
    }

    #[test]
    fn engine_settings() {
        assert!(open("tts", 2).is_err());
    }
}
