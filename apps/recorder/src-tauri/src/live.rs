// The live transcript, while a call is being recorded: each channel's audio is cut where a turn ends (Silero VAD) and
// goes through the engine (asr.rs) right away, so the text reaches the API (`POST /v1/recordings/{id}/live`) seconds after it is spoken.
// It is a draft: the voices on the other side aren't told apart (diarization needs the whole call), and the pass after
// the call (transcribe::run) replaces it. Best effort: nothing here can stop or spoil the recording.
use crate::api::{self, LiveBatch, Participants, Segment};
use crate::config::Config;
use crate::participants::{Latest, LiveVoices};
use std::sync::mpsc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const RATE: usize = crate::capture::RATE as usize;
// A turn's end as the VAD reports it already carries ~300 ms of quiet (its own pad and speech()'s), so it is over
// ~750 ms after the voice stops. 600 ms cut sentences at a pause to think (a real call, Oct 2026); a pause for breath
// (~200–400 ms) never does. Silero over the whole
// buffer (at most MAX_WAIT) takes a few ms, so it can run this often.
const STEP: usize = RATE * 3 / 10;    // look for a finished turn every 300 ms of new audio
const SILENCE: usize = RATE * 45 / 100; // quiet needed after the turn's (padded) end
const MAX_WAIT: usize = 20 * RATE;    // someone talking for this long gets transcribed mid-turn
const IDLE_KEEP: usize = RATE / 2;    // with no speech, the buffer keeps only its tail (a turn may be starting)
const SEND_EVERY: Duration = Duration::from_secs(2);

/// 100 ms of each channel, as the capture produces it.
pub struct Frame {
    pub mic: Vec<f32>,
    pub system: Vec<f32>,
}

/// One channel's audio not transcribed yet, from sample `at` of the recording.
struct Channel {
    name: &'static str,
    speaker: Option<String>,
    buf: Vec<f32>,
    at: usize,
    engine: Box<dyn crate::asr::Engine>,
    lang: Option<String>,
}

impl Channel {
    /// Transcribes the turns that are over (or everything, after MAX_WAIT) and drops their audio.
    fn step(&mut self, vad: &mut whisper_rs::WhisperVadContext, langs: &[String], vocabulary: &str) -> anyhow::Result<Vec<Segment>> {
        let turns = crate::transcribe::speech_in(vad, &self.buf)?;
        if turns.is_empty() {
            let cut = self.buf.len().saturating_sub(IDLE_KEEP);
            self.drop_to(cut);
            return Ok(vec![]);
        }
        let len = self.buf.len();
        let over: Vec<(usize, usize)> = if len >= MAX_WAIT { turns } else { turns.into_iter().filter(|t| t.1 + SILENCE <= len).collect() };
        let Some(&(_, cut)) = over.last() else { return Ok(vec![]) };
        let mut out = vec![];
        for (a, b) in over {
            if b - a < RATE / 2 {
                continue;
            }
            let audio = &self.buf[a..b];
            let lang = self.engine.language(audio, langs, self.lang.as_deref())?;
            out.extend(crate::transcribe::turn(self.engine.as_mut(), audio, self.at + a, &lang, vocabulary, self.name, self.speaker.as_deref())?);
            self.lang = Some(lang);
        }
        self.drop_to(cut);
        Ok(out)
    }

    fn drop_to(&mut self, cut: usize) {
        self.buf.drain(..cut);
        self.at += cut;
    }
}

/// Runs on its own thread until the capture drops its end of `frames`; sends each turn's segments to `out`.
/// Whatever is still in the buffers then is left to the pass after the call.
/// `engine`: the setting (asr::open) for the draft.
pub fn transcribe(frames: mpsc::Receiver<Frame>, out: tokio::sync::mpsc::UnboundedSender<Vec<Segment>>, languages: &str, vocabulary: &str, engine: &str) -> anyhow::Result<()> {
    let langs = crate::config::languages(languages);
    // Half the cores: the call itself (video, the browser) runs on this Mac too.
    let threads = std::thread::available_parallelism().map(|n| n.get() / 2).unwrap_or(4).clamp(2, 4);
    let mut vad = crate::transcribe::vad()?;
    let mut mic = Channel { name: "mic", speaker: Some("mic".into()), buf: vec![], at: 0, engine: crate::asr::open(engine, threads)?, lang: None };
    let mut system = Channel { name: "system", speaker: None, buf: vec![], at: 0, engine: crate::asr::open(engine, threads)?, lang: None };
    let mut fresh = 0usize;
    while let Ok(f) = frames.recv() {
        fresh += f.mic.len();
        mic.buf.extend_from_slice(&f.mic);
        system.buf.extend_from_slice(&f.system);
        // Frames that queued up while Whisper ran are taken all at once.
        while let Ok(f) = frames.try_recv() {
            fresh += f.mic.len();
            mic.buf.extend_from_slice(&f.mic);
            system.buf.extend_from_slice(&f.system);
        }
        if fresh < STEP {
            continue;
        }
        fresh = 0;
        let mut segs = mic.step(&mut vad, &langs, vocabulary)?;
        segs.extend(system.step(&mut vad, &langs, vocabulary)?);
        if !segs.is_empty() {
            segs.sort_by_key(|s| s.start_ms);
            if out.send(segs).is_err() {
                break;
            }
        }
    }
    Ok(())
}

/// Sends the segments as they come, in batches. The API answers how many it holds, so a batch that failed (offline,
/// the API restarting) is sent again from there and nothing is duplicated. The first batch goes out empty, right away.
/// An answer with `stop` ("Stop recording" in the portal) stops the recording. When `segments` closes (the recording
/// ended), what is left goes in a last batch marked `ended`, tried once. The other side's lines go out named after who
/// the call app showed speaking (speakers.jsonl in `dir`, participants.rs), with the names when a new one appears.
pub async fn send(cfg: Config, id: String, mut segments: tokio::sync::mpsc::UnboundedReceiver<Vec<Segment>>, stop: CancellationToken, people: Latest, dir: std::path::PathBuf) {
    let mut all: Vec<Segment> = vec![];
    let mut voices = LiveVoices::default();
    let mut named = 0usize; // lines given a name (or found to have none) so far
    let mut new_names = false; // a name the API hasn't taken yet
    let mut reports: Vec<crate::api::ReaderReport> = vec![]; // reader issues the API hasn't taken yet
    let mut sent: Option<Participants> = None; // who the API was last told is in the call
    let mut held = 0usize; // how many the API has
    let mut open = true;
    let mut first = true;
    loop {
        if open && !first {
            match tokio::time::timeout(SEND_EVERY, segments.recv()).await {
                Ok(Some(s)) => {
                    all.extend(s);
                    while let Ok(s) = segments.try_recv() {
                        all.extend(s);
                    }
                }
                Ok(None) => open = false,
                // Nothing new: an empty batch anyway, to hear about "Stop".
                Err(_) => {}
            }
        }
        first = false;
        // Who is in the call goes along only when it changed (when it was read is no change).
        let now = people.lock().unwrap().as_ref().map(|l| l.participants());
        let changed = now.as_ref().filter(|n| sent.as_ref().is_none_or(|s| Participants { t_ms: n.t_ms, ..s.clone() } != **n));
        if named < all.len() {
            new_names |= voices.label(&crate::participants::read(&dir), &mut all[named..]);
            named = all.len();
        }
        let speakers = new_names.then(|| voices.speakers());
        reports.extend(std::mem::take(&mut *crate::participants::REPORTS.lock().unwrap()));
        let batch = LiveBatch { from: held, segments: &all[held..], participants: changed, reader: &reports, speakers: speakers.as_deref(), ended: !open };
        match api::live(&cfg, &id, &batch).await {
            Ok(ack) => {
                held = ack.n.min(all.len());
                if changed.is_some() {
                    sent = now.clone();
                }
                new_names = false;
                reports.clear();
                if ack.stop && open {
                    stop.cancel();
                }
            }
            Err(e) => eprintln!("live transcript not sent: {e}"),
        }
        if !open {
            break;
        }
    }
}
