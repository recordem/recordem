// Naming the voices. The call app says who is speaking (scrape samples, a few per second); diarization says when
// each voice spoke ("system:1" from 12.0 s to 15.5 s). A voice is named by the vote of its lines, each for whoever
// the app showed speaking during it (`attribute`). A voice that got a name keeps it on every line: the app's mark
// lingers a second or two on whoever spoke last, so a short reply right after them would take their name (Meet,
// Oct 2026). Lines of a voice with no clear name (a cluster of everyone's "yeah", one too small) take the person
// the app showed during each of them.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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
    /// Below this share, or this much overlap, a voice stays unnamed — unless its name leads the next one by `lead`
    /// with at least `min_lead_share` (in a lively call everyone's tile flickers, so nobody holds 60%, Oct 2026).
    pub min_share: f64,
    pub lead: f64,
    pub min_lead_share: f64,
    pub min_overlap_ms: u64,
    /// A speaking indicator on longer than this without a break has gone stale: while someone else's is fresh, only
    /// the fresh one counts. Meet lights a tile for any sound (an open mic in a noisy room stays lit for the whole
    /// call, Oct 2026), and that tile would otherwise claim every voice.
    pub stale_ms: u64,
    /// A line looks this far before and after itself for who spoke it (the app's mark lags and lingers); speech
    /// there counts `pad_weight` of speech inside the line.
    pub pad_before_ms: u64,
    pub pad_after_ms: u64,
    pub pad_weight: f64,
    /// A voice's name needs this share of its lines' named time and this share of all its time.
    pub voice_coverage: f64,
    /// A voice with less speech than this, in a call where another has more, is diarization noise (a cluster of
    /// everyone's "yeah"): its lines get the person the app showed or nobody.
    pub small_voice_ms: u64,
}

impl Default for Options {
    fn default() -> Self {
        Options { max_gap_ms: 1_500, lag_ms: 300, min_share: 0.6, lead: 1.5, min_lead_share: 0.45, min_overlap_ms: 2_000, stale_ms: 8_000, pad_before_ms: 500, pad_after_ms: 300, pad_weight: 0.25, voice_coverage: 0.25, small_voice_ms: 10_000 }
    }
}

/// Samples taken only when something changed (and as a heartbeat) as a steady series: each one holds until the next,
/// every `step_ms`, for at most `hold_ms` (a longer gap is a time nothing could be read).
pub fn held(samples: &[Sample], step_ms: u64, hold_ms: u64) -> Vec<Sample> {
    let mut out = Vec::with_capacity(samples.len());
    for (i, s) in samples.iter().enumerate() {
        let until = samples.get(i + 1).map_or(s.t_ms + step_ms, |n| n.t_ms.min(s.t_ms + hold_ms));
        let mut t = s.t_ms;
        while t < until {
            out.push(Sample { t_ms: t, speaking: s.speaking.clone() });
            t += step_ms;
        }
    }
    out
}

/// Each sample with its stale indicators dropped while a fresh one is on (`Options::stale_ms`).
fn fresh(samples: &[Sample], opts: &Options) -> Vec<Sample> {
    let mut since: HashMap<String, u64> = HashMap::new();
    let mut last = None::<u64>;
    samples
        .iter()
        .map(|s| {
            if last.is_some_and(|l| s.t_ms > l + opts.max_gap_ms) {
                since.clear();
            }
            last = Some(s.t_ms);
            since.retain(|n, _| s.speaking.contains(n));
            for n in &s.speaking {
                since.entry(n.clone()).or_insert(s.t_ms);
            }
            let new: Vec<String> = s.speaking.iter().filter(|n| s.t_ms - since[*n] < opts.stale_ms).cloned().collect();
            Sample { t_ms: s.t_ms, speaking: if new.is_empty() { s.speaking.clone() } else { new } }
        })
        .collect()
}

/// Speaking intervals per person from the samples, in time order.
pub fn intervals(samples: &[Sample], opts: &Options) -> Vec<Interval> {
    let samples = fresh(samples, opts);
    let mut open: HashMap<&str, Interval> = HashMap::new();
    let mut out = Vec::new();
    for s in &samples {
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
    // Each voice's runner-up, for the lead.
    let mut second: HashMap<&str, u64> = HashMap::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for ((label, _), o) in &pairs {
        if !seen.insert(label) {
            second.entry(label).or_insert(*o);
        }
    }
    let (mut labels, mut names) = (HashSet::new(), HashSet::new());
    let mut out = Vec::new();
    for ((label, name), o) in pairs {
        if labels.contains(label) || names.contains(name) {
            continue;
        }
        let share = o as f64 / by_label[label] as f64;
        let leads = share >= opts.min_lead_share && o as f64 >= opts.lead * second.get(label).copied().unwrap_or(0) as f64;
        if o < opts.min_overlap_ms || share < opts.min_share && !leads {
            continue;
        }
        labels.insert(label);
        names.insert(name);
        out.push(Assignment { label: label.to_string(), name: name.to_string(), overlap_ms: o, share });
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// Who said a line.
#[derive(Debug, Clone, PartialEq)]
pub enum Who {
    /// A person: the name of the line's voice, or shown speaking during the line (`by_line`). `share`: how clear.
    Person { name: String, share: f64, by_line: bool },
    /// A voice nobody could be named for.
    Voice(String),
    Nobody,
}

/// The person the app showed speaking most during a turn (and a little around it), with their share of it.
pub fn line_name(turn: &Turn, intervals: &[Interval], opts: &Options) -> Option<(String, f64)> {
    let (s, e) = (turn.start_ms, turn.end_ms);
    let (ps, pe) = (s.saturating_sub(opts.pad_before_ms), e + opts.pad_after_ms);
    let mut by: HashMap<&str, f64> = HashMap::new();
    for iv in intervals {
        let inside = overlap((s, e), (iv.start_ms, iv.end_ms)) as f64;
        let around = overlap((ps, pe), (iv.start_ms, iv.end_ms)) as f64 - inside;
        let w = inside + around * opts.pad_weight;
        if w > 0.0 {
            *by.entry(iv.name.as_str()).or_default() += w;
        }
    }
    let total: f64 = by.values().sum();
    let (name, w) = by.into_iter().max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(a.0)))?;
    Some((name.to_string(), w / total))
}

/// Who said each turn: its voice's name (the vote of its lines, weighed by duration: `min_share` of the named time,
/// `voice_coverage` of all of it, `min_overlap_ms` at least), else the person shown during it, else the voice,
/// unless it is too small to be one. Several voices can get one name; a voice never gets two.
pub fn attribute(turns: &[Turn], intervals: &[Interval], opts: &Options) -> Vec<Who> {
    let lines: Vec<Option<(String, f64)>> = turns.iter().map(|t| line_name(t, intervals, opts)).collect();
    let mut length: HashMap<&str, u64> = HashMap::new();
    let mut votes: HashMap<&str, HashMap<&str, u64>> = HashMap::new();
    for (t, l) in turns.iter().zip(&lines) {
        if t.label.is_empty() {
            continue;
        }
        let d = t.end_ms.saturating_sub(t.start_ms);
        *length.entry(&t.label).or_default() += d;
        if let Some((name, _)) = l {
            *votes.entry(&t.label).or_default().entry(name.as_str()).or_default() += d;
        }
    }
    let mut voice: HashMap<&str, (String, f64)> = HashMap::new();
    for (label, v) in &votes {
        let named: u64 = v.values().sum();
        let Some((name, &d)) = v.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))) else { continue };
        let share = d as f64 / named as f64;
        if d >= opts.min_overlap_ms && share >= opts.min_share && d as f64 >= opts.voice_coverage * length[label] as f64 {
            voice.insert(label, (name.to_string(), share));
        }
    }
    let longest = length.values().copied().max().unwrap_or(0);
    turns
        .iter()
        .zip(lines)
        .map(|(t, l)| match (voice.get(t.label.as_str()), l) {
            (Some((name, share)), _) => Who::Person { name: name.clone(), share: *share, by_line: false },
            (None, Some((name, share))) => Who::Person { name, share, by_line: true },
            (None, None) if t.label.is_empty() => Who::Nobody,
            (None, None) if length[t.label.as_str()] < opts.small_voice_ms && longest >= opts.small_voice_ms => Who::Nobody,
            (None, None) => Who::Voice(t.label.clone()),
        })
        .collect()
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
    fn held_fills_between_changes() {
        let s = held(&[sample(0, &["Ana"]), sample(2_000, &[]), sample(20_000, &["Bia"])], 500, 6_000);
        assert_eq!(s.iter().filter(|x| x.speaking == ["Ana"]).count(), 4, "0, 0.5, 1, 1.5 s");
        assert!(s.iter().all(|x| x.t_ms < 8_000 || x.t_ms >= 20_000), "nothing held past 6 s: {s:?}");
    }

    #[test]
    fn a_stale_indicator_gives_way_to_a_fresh_one() {
        // Ana's tile is lit the whole time; Bia talks from 20 to 30 s, Caio from 40 to 50 s.
        let s: Vec<Sample> = (0..120)
            .map(|i| {
                let t = i * 500;
                let mut on = vec!["Ana"];
                if (20_000..30_000).contains(&t) { on.push("Bia") }
                if (40_000..50_000).contains(&t) { on.push("Caio") }
                sample(t, &on)
            })
            .collect();
        let opts = Options { lag_ms: 0, ..Default::default() };
        let named = assign(&[turn("system:1", 0, 8_000), turn("system:2", 20_500, 29_500), turn("system:3", 40_500, 49_500)], &intervals(&s, &opts), &opts);
        let got: Vec<(&str, &str)> = named.iter().map(|a| (a.label.as_str(), a.name.as_str())).collect();
        assert_eq!(got, [("system:1", "Ana"), ("system:2", "Bia"), ("system:3", "Caio")]);
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
        s.sort_by_key(|x| x.t_ms);
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

    fn people(w: &[Who]) -> Vec<Option<&str>> {
        w.iter().map(|w| match w { Who::Person { name, .. } => Some(name.as_str()), _ => None }).collect()
    }

    #[test]
    fn short_lines_get_whoever_was_shown() {
        let opts = Options { lag_ms: 0, ..Default::default() };
        // Ana shown until 5 s, Bia after.
        let s: Vec<Sample> = (0..40).map(|i| sample(i * 250, &[if i < 20 { "Ana" } else { "Bia" }])).collect();
        // Bia's "Yeah." (0.4 s), a line across the change that is mostly Ana's, and one mostly Bia's.
        let turns = [turn("", 6_000, 6_400), turn("", 3_500, 5_300), turn("", 4_800, 7_000)];
        assert_eq!(people(&attribute(&turns, &intervals(&s, &opts), &opts)), [Some("Bia"), Some("Ana"), Some("Bia")]);
    }

    #[test]
    fn a_voice_of_everyones_yeah_is_named_line_by_line() {
        let opts = Options { lag_ms: 0, ..Default::default() };
        let s: Vec<Sample> = (0..160).map(|i| sample(i * 250, &[if i < 80 { "Ana" } else { "Bia" }])).collect();
        let turns = [turn("system:1", 0, 15_000), turn("system:2", 21_000, 39_000), turn("system:3", 5_000, 5_500), turn("system:3", 30_000, 30_500)];
        let w = attribute(&turns, &intervals(&s, &opts), &opts);
        assert_eq!(people(&w), [Some("Ana"), Some("Bia"), Some("Ana"), Some("Bia")]);
    }

    #[test]
    fn lines_nobody_was_shown_for_take_their_voice() {
        let opts = Options { lag_ms: 0, ..Default::default() };
        // Ana shown until 20 s; then the call can't be read (another tab).
        let s: Vec<Sample> = (0..80).map(|i| sample(i * 250, &["Ana"])).collect();
        let turns = [turn("system:1", 0, 9_000), turn("system:1", 9_000, 19_000), turn("system:1", 30_000, 34_000), turn("system:2", 40_000, 41_000), turn("system:4", 45_000, 70_000)];
        let w = attribute(&turns, &intervals(&s, &opts), &opts);
        assert_eq!(w[2], Who::Person { name: "Ana".into(), share: 1.0, by_line: false });
        assert_eq!(w[3], Who::Nobody, "a 1 s voice is noise when another spoke for long");
        assert_eq!(w[4], Who::Voice("system:4".into()));
    }

    #[test]
    fn a_named_voice_keeps_its_name_on_a_reply_under_a_lingering_mark() {
        let opts = Options { lag_ms: 0, ..Default::default() };
        // Ana talks until 10 s; her mark stays on until 12 s while Bia answers "Okay, great." at 10.2 s.
        let s: Vec<Sample> = (0..48).map(|i| sample(i * 250, &["Ana"])).chain((48..80).map(|i| sample(i * 250, &["Bia"]))).collect();
        let turns = [turn("system:1", 0, 10_000), turn("system:2", 10_200, 11_400), turn("system:2", 12_500, 19_500)];
        assert_eq!(people(&attribute(&turns, &intervals(&s, &opts), &opts)), [Some("Ana"), Some("Bia"), Some("Bia")]);
    }

    #[test]
    fn a_voice_split_in_two_gets_one_name_twice() {
        let opts = Options { lag_ms: 0, ..Default::default() };
        let s: Vec<Sample> = (0..80).map(|i| sample(i * 250, &["Ana"])).collect();
        let turns = [turn("system:1", 0, 9_000), turn("system:2", 10_000, 19_000)];
        assert_eq!(people(&attribute(&turns, &intervals(&s, &opts), &opts)), [Some("Ana"), Some("Ana")]);
    }
}
