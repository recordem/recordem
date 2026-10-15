// Naming the voices. The call app says who is speaking (scrape samples, a few per second); diarization says when
// each voice spoke ("system:1" from 12.0 s to 15.5 s). A voice is named after whoever the app showed speaking while
// it spoke, if that person clearly dominates and nobody else claims them. What doesn't settle stays unnamed: a
// wrong name is worse than "system:3".
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One scrape: at `t_ms` (recording time), these people were shown speaking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub t_ms: u64,
    pub speaking: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interval {
    pub name: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// A diarization turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub label: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Assignment {
    pub label: String,
    pub name: String,
    /// Time this voice spoke while the app showed `name` speaking.
    pub overlap_ms: u64,
    /// That overlap over all the overlap this voice has with anyone: how clear the evidence is.
    pub share: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Samples further apart than this are separate intervals (the scrape stalled or the person paused).
    pub max_gap_ms: u64,
    /// The app shows a speaker after they start: intervals are moved this much earlier.
    pub lag_ms: u64,
    /// Below this share, or this much overlap, a voice stays unnamed.
    pub min_share: f64,
    pub min_overlap_ms: u64,
}

impl Default for Options {
    fn default() -> Self {
        Options { max_gap_ms: 1_500, lag_ms: 300, min_share: 0.6, min_overlap_ms: 2_000 }
    }
}

/// Speaking intervals per person from the samples, in time order.
pub fn intervals(samples: &[Sample], opts: &Options) -> Vec<Interval> {
    let mut open: HashMap<&str, Interval> = HashMap::new();
    let mut out = Vec::new();
    for s in samples {
        let start = s.t_ms.saturating_sub(opts.lag_ms);
        for name in &s.speaking {
            match open.get_mut(name.as_str()) {
                Some(iv) if start <= iv.end_ms + opts.max_gap_ms => iv.end_ms = iv.end_ms.max(start),
                _ => {
                    if let Some(done) = open.insert(name, Interval { name: name.clone(), start_ms: start, end_ms: start }) {
                        out.push(done);
                    }
                }
            }
        }
    }
    out.extend(open.into_values());
    // A single sample is a point; it still stands for one polling period of speech.
    for iv in &mut out {
        if iv.end_ms == iv.start_ms {
            iv.end_ms += opts.max_gap_ms / 3;
        }
    }
    out.sort_by_key(|iv| (iv.start_ms, iv.name.clone()));
    out
}

fn overlap(a: (u64, u64), b: (u64, u64)) -> u64 {
    a.1.min(b.1).saturating_sub(a.0.max(b.0))
}

/// One name per voice at most, and one voice per name: the strongest pairs are taken first.
pub fn assign(turns: &[Turn], intervals: &[Interval], opts: &Options) -> Vec<Assignment> {
    let mut by_pair: HashMap<(&str, &str), u64> = HashMap::new();
    let mut by_label: HashMap<&str, u64> = HashMap::new();
    for t in turns {
        for iv in intervals {
            let o = overlap((t.start_ms, t.end_ms), (iv.start_ms, iv.end_ms));
            if o > 0 {
                *by_pair.entry((t.label.as_str(), iv.name.as_str())).or_default() += o;
                *by_label.entry(t.label.as_str()).or_default() += o;
            }
        }
    }
    let mut pairs: Vec<((&str, &str), u64)> = by_pair.into_iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let (mut labels, mut names) = (std::collections::HashSet::new(), std::collections::HashSet::new());
    let mut out = Vec::new();
    for ((label, name), o) in pairs {
        if labels.contains(label) || names.contains(name) {
            continue;
        }
        let share = o as f64 / by_label[label] as f64;
        if o < opts.min_overlap_ms || share < opts.min_share {
            continue;
        }
        labels.insert(label);
        names.insert(name);
        out.push(Assignment { label: label.to_string(), name: name.to_string(), overlap_ms: o, share });
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(t: u64, names: &[&str]) -> Sample {
        Sample { t_ms: t, speaking: names.iter().map(|s| s.to_string()).collect() }
    }

    fn turn(label: &str, s: u64, e: u64) -> Turn {
        Turn { label: label.into(), start_ms: s, end_ms: e }
    }

    #[test]
    fn samples_become_intervals() {
        let opts = Options { lag_ms: 0, ..Default::default() };
        let s: Vec<Sample> = (0..10).map(|i| sample(i * 500, &["Ana"])).chain([sample(10_000, &["Ana"])]).collect();
        let iv = intervals(&s, &opts);
        assert_eq!(iv.len(), 2, "a 5.5 s gap splits Ana's speech: {iv:?}");
        assert_eq!((iv[0].start_ms, iv[0].end_ms), (0, 4_500));
    }

    #[test]
    fn voices_get_the_dominant_name() {
        let opts = Options { lag_ms: 0, ..Default::default() };
        let mut s = Vec::new();
        for t in (0..10_000).step_by(500) {
            s.push(sample(t, &["Ana"]));
        }
        for t in (10_000..20_000).step_by(500) {
            s.push(sample(t, &["Bruno"]));
        }
        // Crosstalk: Bruno shown for a moment during Ana's turn.
        s.push(sample(3_000, &["Bruno"]));
        let iv = intervals(&s, &opts);
        let turns = [turn("system:1", 0, 9_800), turn("system:2", 10_100, 19_900), turn("system:3", 30_000, 31_000)];
        let a = assign(&turns, &iv, &opts);
        assert_eq!(a.iter().map(|a| (a.label.as_str(), a.name.as_str())).collect::<Vec<_>>(), [
            ("system:1", "Ana"),
            ("system:2", "Bruno")
        ]);
        assert!(a[0].share > 0.9);
    }

    #[test]
    fn unclear_voices_stay_unnamed() {
        let opts = Options { lag_ms: 0, ..Default::default() };
        // Ana and Bruno shown together the whole time: no evidence for either.
        let s: Vec<Sample> = (0..20).map(|i| sample(i * 500, &["Ana", "Bruno"])).collect();
        let a = assign(&[turn("system:1", 0, 9_500)], &intervals(&s, &opts), &opts);
        assert!(a.is_empty(), "{a:?}");
    }
}
