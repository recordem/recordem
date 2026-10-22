// Telling the voices of the other side apart while the call goes on. It is Anarlog's pyannote-local pipeline (MIT:
// segmentation, one WeSpeaker embedding per voice per 10 s window, centroid-linkage clustering, frame voting), split
// where it can be: the windows — nearly all of the work — are run as the audio arrives, and only the clustering, a
// few milliseconds of arithmetic over the embeddings, waits for the end of the call. The result is the segments
// `Diarizer::diarize` gives for the whole file (minus known voiceprints, which the recorder doesn't use), with one
// difference: one voice at a time. Where two voices overlap, the frame goes to the one most windows heard (as
// pyannote's exclusive diarization, and Granola, do): each turn is then transcribed once, never the same words twice.
//
// Only the audio one window still needs is kept (10 s), so an hour of call costs the activities and embeddings, not
// the samples. The windows are 2 s apart for the first hour, then 4 s (the batch pipeline caps the count the same
// way, so clustering stays bounded).
use anlg_pyannote::clustering::{self, SpeakerBounds};
use anlg_pyannote::segmentation::{FRAME_SIZE, LOCAL_SPEAKERS, SAMPLE_RATE, Segmenter, WINDOW_SAMPLES, WindowActivity, frame_start_sample, frames_for_samples};
use anlg_pyannote::{DiarizationConfig, SpeakerSegment};

struct Embedded {
    window: usize,
    local: usize,
    embedding: Vec<f32>,
    anchor: bool,
}

pub struct Streaming {
    segmenter: Segmenter,
    embedder: anlg_embedding::EmbeddingExtractor,
    config: DiarizationConfig,
    /// Audio from sample `kept_from` of the channel on; the next window starts at `next`.
    kept: Vec<f32>,
    kept_from: usize,
    next: usize,
    total: usize,
    starts: Vec<usize>,
    activities: Vec<WindowActivity>,
    embeddings: Vec<Embedded>,
    mask: Vec<f32>,
    /// One voice per frame (the default); off, overlapping voices each keep their frames, as the batch pipeline.
    pub exclusive: bool,
    /// How many voices there can be (e.g. no more than the people the call app lists); unbounded by default.
    pub bounds: SpeakerBounds,
}

impl Streaming {
    pub fn new() -> anyhow::Result<Self> {
        Self::with(DiarizationConfig::default())
    }

    /// With other settings than the default (eval.rs tries them).
    pub fn with(config: DiarizationConfig) -> anyhow::Result<Self> {
        Ok(Self {
            segmenter: Segmenter::new()?,
            embedder: anlg_embedding::EmbeddingExtractor::new()?,
            config,
            kept: Vec::with_capacity(2 * WINDOW_SAMPLES),
            kept_from: 0,
            next: 0,
            total: 0,
            starts: vec![],
            activities: vec![],
            embeddings: vec![],
            mask: vec![0.0; WINDOW_SAMPLES],
            exclusive: true,
            bounds: SpeakerBounds::default(),
        })
    }

    fn step(&self) -> usize {
        let seconds = if self.starts.len() < self.config.max_windows / 2 { self.config.step_seconds } else { self.config.step_seconds * 2.0 };
        seconds_to_frames(seconds).max(1) * FRAME_SIZE
    }

    /// The next samples of the channel (16 kHz mono); runs every window they complete.
    pub fn push(&mut self, samples: &[f32]) -> anyhow::Result<()> {
        self.kept.extend_from_slice(samples);
        self.total += samples.len();
        while self.next + WINDOW_SAMPLES <= self.total {
            self.window(WINDOW_SAMPLES)?;
        }
        Ok(())
    }

    /// The call is over: the windows left (the last ones run past the end, zero-padded, as in the batch pipeline),
    /// then who spoke when. Speakers are numbered from 0 by first appearance; empty when no one spoke.
    pub fn finish(mut self) -> anyhow::Result<Vec<SpeakerSegment>> {
        if self.total == 0 {
            return Ok(vec![]);
        }
        loop {
            let last_end = self.starts.last().map(|s| s + WINDOW_SAMPLES);
            if last_end.is_some_and(|e| e >= self.total) || self.next >= self.total && !self.starts.is_empty() {
                break;
            }
            self.window(self.total - self.next)?;
        }
        if self.embeddings.is_empty() {
            return Ok(vec![]);
        }
        let labels = self.cluster();
        let mut segments = self.reconstruct(&labels);
        renumber(&mut segments);
        Ok(segments)
    }

    /// The window at `next`, of which `valid` samples are audio (the rest zeros).
    fn window(&mut self, valid: usize) -> anyhow::Result<()> {
        let start = self.next;
        let at = start - self.kept_from;
        let mut window = vec![0.0f32; WINDOW_SAMPLES];
        let n = valid.min(self.kept.len() - at);
        window[..n].copy_from_slice(&self.kept[at..at + n]);
        let mut activity = self.segmenter.run_window(&window)?;
        activity.frames.truncate(frames_for_samples(valid).min(activity.frames.len()));
        let min_anchor = seconds_to_frames(self.config.min_embedding_seconds).max(1);
        let min_activity = seconds_to_frames(self.config.min_activity_seconds).max(1);
        let index = self.starts.len();
        for local in 0..LOCAL_SPEAKERS {
            if activity.active_frames(local) < min_activity {
                continue;
            }
            let anchor = activity.clean_frames(local) >= min_anchor;
            fill_mask(&activity, local, anchor, &mut self.mask);
            if let Some(embedding) = self.embedder.compute_with_mask_optional(&window, &self.mask)? {
                self.embeddings.push(Embedded { window: index, local, embedding, anchor });
            }
        }
        self.starts.push(start);
        self.activities.push(activity);
        self.next = start + self.step();
        // Nothing before the next window is needed again.
        let drop = (self.next - self.kept_from).min(self.kept.len());
        self.kept.drain(..drop);
        self.kept_from += drop;
        Ok(())
    }

    /// Clean-speech embeddings ("anchors") are clustered; every other embedding joins its nearest cluster.
    fn cluster(&self) -> Vec<usize> {
        let mut anchors: Vec<usize> = (0..self.embeddings.len()).filter(|i| self.embeddings[*i].anchor).collect();
        if anchors.is_empty() {
            anchors = (0..self.embeddings.len()).collect();
        }
        let vectors: Vec<Vec<f32>> = anchors.iter().map(|i| self.embeddings[*i].embedding.clone()).collect();
        let step = self.total.saturating_sub(WINDOW_SAMPLES).max(1) as f32 / self.starts.len().saturating_sub(1).max(1) as f32;
        let min_cluster = (self.config.min_cluster_seconds / (step / SAMPLE_RATE as f32).max(f32::EPSILON)).ceil() as usize;
        let anchor_labels = clustering::cluster(&vectors, self.config.clustering_threshold, min_cluster, self.bounds);
        let centroids = clustering::centroids(&vectors, &anchor_labels);
        let mut labels = vec![usize::MAX; self.embeddings.len()];
        for (i, label) in anchors.iter().zip(&anchor_labels) {
            labels[*i] = *label;
        }
        for (i, e) in self.embeddings.iter().enumerate() {
            if labels[i] == usize::MAX {
                labels[i] = clustering::nearest_centroid(&e.embedding, &centroids).unwrap_or(0);
            }
        }
        labels
    }

    /// Each frame goes to the voices that a majority of the windows covering it heard there.
    fn reconstruct(&self, labels: &[usize]) -> Vec<SpeakerSegment> {
        let speakers = labels.iter().max().map_or(0, |m| m + 1);
        let frames = frames_for_samples(self.total.max(WINDOW_SAMPLES));
        let mut votes = vec![vec![0u16; frames]; speakers];
        let mut coverage = vec![0u16; frames];
        for (w, activity) in self.activities.iter().enumerate() {
            let offset = self.starts[w] / FRAME_SIZE;
            for frame in 0..activity.frames.len() {
                if let Some(c) = coverage.get_mut(offset + frame) {
                    *c += 1;
                }
            }
        }
        for (e, &label) in self.embeddings.iter().zip(labels) {
            let offset = self.starts[e.window] / FRAME_SIZE;
            for (frame, active) in self.activities[e.window].frames.iter().enumerate() {
                if active[e.local]
                    && let Some(v) = votes[label].get_mut(offset + frame)
                {
                    *v += 1;
                }
            }
        }
        let end = self.total as f64 / SAMPLE_RATE as f64;
        let winners = self.exclusive.then(|| winners(&votes, &coverage));
        let mut segments = vec![];
        for (speaker, track) in votes.iter().enumerate() {
            let active: Vec<bool> = match &winners {
                Some(w) => w.iter().map(|w| *w == Some(speaker)).collect(),
                None => track.iter().zip(&coverage).map(|(v, c)| heard(*v, *c)).collect(),
            };
            segments.extend(
                runs(&active, speaker, self.config.min_duration_on, self.config.min_duration_off)
                    .into_iter()
                    .map(|mut s| {
                        s.end = s.end.min(end);
                        s
                    })
                    .filter(|s| s.end > s.start),
            );
        }
        segments.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.speaker.cmp(&b.speaker)));
        segments
    }
}

/// A voice speaks in a frame when at least half the windows covering it heard it there.
fn heard(votes: u16, coverage: u16) -> bool {
    coverage > 0 && u32::from(votes) * 2 >= u32::from(coverage)
}

/// The one voice of each frame: of those heard there, the one with the most votes; a tie stays with the voice of the
/// frame before (a turn isn't cut by a coin toss), else the first. None where no one is heard.
fn winners(votes: &[Vec<u16>], coverage: &[u16]) -> Vec<Option<usize>> {
    let mut out: Vec<Option<usize>> = Vec::with_capacity(coverage.len());
    for (frame, &c) in coverage.iter().enumerate() {
        let prev = out.last().copied().flatten();
        let mut best: Option<(u16, usize)> = None;
        for (speaker, track) in votes.iter().enumerate() {
            let v = track[frame];
            if !heard(v, c) {
                continue;
            }
            if best.is_none_or(|(b, k)| v > b || v == b && prev == Some(speaker) && prev != Some(k)) {
                best = Some((v, speaker));
            }
        }
        out.push(best.map(|(_, s)| s));
    }
    out
}

/// Runs while a recording does: the system channel from `audio` until the capture drops its end, then voices.json
/// in `dir`. Nothing is written when it fails: the pass after the call then diarizes the whole file itself.
pub fn during(audio: std::sync::mpsc::Receiver<std::sync::Arc<[f32]>>, dir: &std::path::Path) {
    let result = (|| -> anyhow::Result<Vec<SpeakerSegment>> {
        let mut d = Streaming::new()?;
        for chunk in audio {
            d.push(&chunk)?;
        }
        d.finish()
    })();
    match result {
        Ok(segs) => {
            let rows: Vec<(f64, f64, usize)> = segs.iter().map(|s| (s.start, s.end, s.speaker)).collect();
            if let Err(e) = std::fs::write(dir.join("voices.json"), serde_json::to_vec(&rows).unwrap_or_default()) {
                eprintln!("voices not saved: {e}");
            }
        }
        Err(e) => eprintln!("diarization during the call failed: {e}"),
    }
}

/// What `during` found for the recording in `dir`, if it got to the end.
pub fn found(dir: &std::path::Path) -> Option<Vec<SpeakerSegment>> {
    let rows: Vec<(f64, f64, usize)> = serde_json::from_slice(&std::fs::read(dir.join("voices.json")).ok()?).ok()?;
    Some(rows.into_iter().map(|(start, end, speaker)| SpeakerSegment { start, end, speaker }).collect())
}

fn seconds_to_frames(seconds: f32) -> usize {
    ((seconds.max(0.0) * SAMPLE_RATE as f32) / FRAME_SIZE as f32).round() as usize
}

fn frame_time(frame: usize) -> f64 {
    frame_start_sample(frame) as f64 / SAMPLE_RATE as f64
}

/// 1 where `speaker` speaks in the window (and, for an anchor, no one else does).
fn fill_mask(activity: &WindowActivity, speaker: usize, clean_only: bool, mask: &mut [f32]) {
    mask.fill(0.0);
    for (frame, active) in activity.frames.iter().enumerate() {
        if !active[speaker] || clean_only && active.iter().filter(|v| **v).count() != 1 {
            continue;
        }
        let start = if frame == 0 { 0 } else { frame_start_sample(frame) };
        let end = frame_start_sample(frame + 1).min(mask.len());
        if start < end {
            mask[start..end].fill(1.0);
        }
    }
}

/// Runs of active frames as segments, gaps shorter than `min_off` bridged, turns shorter than `min_on` dropped.
fn runs(active: &[bool], speaker: usize, min_on: f32, min_off: f32) -> Vec<SpeakerSegment> {
    let mut out: Vec<SpeakerSegment> = vec![];
    let mut from = None;
    for (frame, on) in active.iter().copied().chain(std::iter::once(false)).enumerate() {
        match (from, on) {
            (None, true) => from = Some(frame),
            (Some(start), false) => {
                let s = SpeakerSegment { start: frame_time(start), end: frame_time(frame), speaker };
                match out.last_mut() {
                    Some(prev) if s.start - prev.end < f64::from(min_off) => prev.end = s.end,
                    _ => out.push(s),
                }
                from = None;
            }
            _ => {}
        }
    }
    out.retain(|s| s.end - s.start >= f64::from(min_on));
    out
}

fn renumber(segments: &mut [SpeakerSegment]) {
    let mut order: Vec<usize> = vec![];
    for s in segments.iter() {
        if !order.contains(&s.speaker) {
            order.push(s.speaker);
        }
    }
    for s in segments.iter_mut() {
        s.speaker = order.iter().position(|x| *x == s.speaker).unwrap_or(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fed in uneven pieces, the windows land where the batch pipeline puts them, and the audio kept stays bounded.
    #[test]
    fn windows_follow_the_audio() {
        let mut d = Streaming::new().unwrap();
        let tone: Vec<f32> = (0..SAMPLE_RATE as usize * 25).map(|i| (i as f32 * 0.05).sin() * 0.1).collect();
        for piece in tone.chunks(1_600 + 37) {
            d.push(piece).unwrap();
            assert!(d.kept.len() <= WINDOW_SAMPLES + 2_000);
        }
        let step = d.step();
        assert!(d.starts.iter().enumerate().all(|(i, s)| *s == i * step));
        assert!(d.starts.last().unwrap() + WINDOW_SAMPLES <= tone.len());
        let _ = d.finish().unwrap();
    }

    /// The same voices as the batch pipeline over the whole file: `RECORDEM_DIARIZE_WAV=call.wav cargo test --release
    /// -- --ignored same_as_batch` (16 kHz mono, under an hour; `RECORDEM_DIARIZE_SECONDS` cuts it).
    #[test]
    #[ignore]
    fn same_as_batch() {
        let path = std::env::var("RECORDEM_DIARIZE_WAV").expect("RECORDEM_DIARIZE_WAV");
        let mut audio = crate::capture::read(std::path::Path::new(&path)).unwrap();
        if let Ok(s) = std::env::var("RECORDEM_DIARIZE_SECONDS") {
            audio.truncate(s.parse::<usize>().unwrap() * SAMPLE_RATE as usize);
        }
        let t = std::time::Instant::now();
        let batch = anlg_pyannote::Diarizer::new(DiarizationConfig::default())
            .unwrap()
            .diarize(&mut audio.as_slice(), &anlg_pyannote::DiarizeRequest::default())
            .unwrap()
            .segments;
        let batch_time = t.elapsed();
        let mut d = Streaming::new().unwrap();
        d.exclusive = false; // the batch pipeline lets voices overlap
        let mut slowest = std::time::Duration::ZERO;
        for chunk in audio.chunks(1_600) {
            let t = std::time::Instant::now();
            d.push(chunk).unwrap();
            slowest = slowest.max(t.elapsed());
        }
        let t = std::time::Instant::now();
        let streamed = d.finish().unwrap();
        eprintln!("batch {batch_time:?}; streaming: slowest 100 ms chunk {slowest:?}, finish {:?}; {} vs {} segments", t.elapsed(), batch.len(), streamed.len());
        assert_eq!(batch.len(), streamed.len());
        for (a, b) in batch.iter().zip(&streamed) {
            assert_eq!(a.speaker, b.speaker);
            assert!((a.start - b.start).abs() < 1e-6 && (a.end - b.end).abs() < 1e-6, "{a:?} vs {b:?}");
        }
    }

    /// Two voices heard over the same frames: the frame goes to the one heard more, and a tie to whoever was speaking.
    #[test]
    fn one_voice_per_frame() {
        let votes = vec![vec![2, 2, 2, 1, 0], vec![0, 2, 1, 2, 2]];
        let coverage = vec![2, 2, 2, 2, 2];
        assert_eq!(winners(&votes, &coverage), vec![Some(0), Some(0), Some(0), Some(1), Some(1)]);
        assert_eq!(winners(&[vec![0, 0]], &[0, 3]), vec![None, None]);
    }

    #[test]
    fn silence_has_no_voices() {
        let mut d = Streaming::new().unwrap();
        d.push(&vec![0.0; SAMPLE_RATE as usize * 15]).unwrap();
        assert!(d.finish().unwrap().is_empty());
    }
}
