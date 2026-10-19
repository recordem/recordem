// Naming the voices. The call app says who is speaking (scrape samples, a few per second); diarization says when
// each voice spoke ("system:1" from 12.0 s to 15.5 s). A voice is named by the vote of its lines, each for whoever
// the app showed speaking during it (`attribute`). A voice that got a name keeps it on every line: the app's mark
// lingers a second or two on whoever spoke last, so a short reply right after them would take their name (Meet,
// Oct 2026). Lines of a voice with no clear name (a cluster of everyone's "yeah", one too small) take the person
// the app showed during each of them. When the app showed nobody (another tab in front, a reader that broke), who
// was in the call and who was muted can still leave a single person: the other one in a 1:1, the one not muted, the
// last one without a voice.
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
    /// A line is someone's when they had this share of the time anyone was shown speaking around it: with several
    /// shown at once (a broken reader, crosstalk), it is nobody's.
    pub line_share: f64,
    /// A voice's name needs this share of its lines' named time and this share of all its time.
    pub voice_coverage: f64,
    /// A voice with less speech than this, in a call where another has more, is diarization noise (a cluster of
    /// everyone's "yeah"): its lines get the person the app showed or nobody.
    pub small_voice_ms: u64,
    /// A roll (who is listed, who is muted) holds this long without a newer one.
    pub roll_hold_ms: u64,
}

impl Default for Options {
    fn default() -> Self {
        Options { max_gap_ms: 1_500, lag_ms: 300, min_share: 0.6, lead: 1.5, min_lead_share: 0.45, min_overlap_ms: 2_000, stale_ms: 8_000, pad_before_ms: 500, pad_after_ms: 300, pad_weight: 0.25, line_share: 0.6, voice_coverage: 0.25, small_voice_ms: 10_000, roll_hold_ms: 6_000 }
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

/// Who the app listed at a moment besides the person on this Mac, and which of them it showed muted. Each holds until
/// the next one, for at most `Options::roll_hold_ms`, like the samples.
#[derive(Debug, Clone, Default)]
pub struct Roll {
    pub t_ms: u64,
    pub present: Vec<String>,
    pub muted: Vec<String>,
}

/// How a line got its person.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum By {
    /// The name of its voice: the vote of the voice's lines.
    Voice,
    /// The only person its voice could be (`eliminate`).
    Elimination,
    /// The person the app showed speaking during the line.
    Shown,
    /// The only person who could have spoken it (`only_one_at`).
    Alone,
}

/// Who said a line.
#[derive(Debug, Clone, PartialEq)]
pub enum Who {
    /// `share`: how clear the evidence was.
    Person { name: String, share: f64, by: By },
    /// A voice nobody could be named for.
    Voice(String),
    Nobody,
}

/// What a guess by elimination is worth: nobody was shown speaking, only nobody else could have been.
const ELIMINATED: f64 = 0.5;

impl Turn {
    fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }

    fn middle_ms(&self) -> u64 {
        self.start_ms + self.duration_ms() / 2
    }
}

/// The person the app showed speaking during a turn (and a little around it), with their share of that time, when
/// it is clearly them (`Options::line_share`).
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
    let share = w / total;
    (share >= opts.line_share).then(|| (name.to_string(), share))
}

/// Who said each turn, from the clearest evidence there is:
/// 1. the name of its voice, by the vote of the voice's lines for who was shown speaking during each;
/// 2. or the only person its voice could be;
/// 3. or who was shown speaking during the line itself;
/// 4. or the only person who could have spoken it;
/// 5. or its voice, unnamed, unless the voice is too small to be one.
///
/// A named voice keeps its name on every line: the app's mark lingers a second or two on whoever spoke last, so a
/// short reply right after them would otherwise take their name (Meet, Oct 2026). Several voices can get one name;
/// a voice never gets two.
pub fn attribute(turns: &[Turn], intervals: &[Interval], rolls: &[Roll], opts: &Options) -> Vec<Who> {
    let shown: Vec<Option<(String, f64)>> = turns.iter().map(|t| line_name(t, intervals, opts)).collect();
    let voices = Voices::of(turns, opts);
    let mut named = voices.voted(turns, &shown, opts);
    voices.eliminate(&mut named, turns, &shown, rolls, opts);
    turns
        .iter()
        .zip(shown)
        .map(|(t, shown)| {
            if let Some((name, share, by)) = named.get(t.label.as_str()) {
                return Who::Person { name: name.clone(), share: *share, by: *by };
            }
            if let Some((name, share)) = shown {
                return Who::Person { name, share, by: By::Shown };
            }
            if let Some(name) = only_one_at(rolls, t.middle_ms(), opts) {
                return Who::Person { name, share: ELIMINATED, by: By::Alone };
            }
            if voices.real(&t.label) { Who::Voice(t.label.clone()) } else { Who::Nobody }
        })
        .collect()
}

/// The diarized voices: the turns of each, and how long it spoke.
struct Voices<'a> {
    lines: HashMap<&'a str, Vec<usize>>,
    length_ms: HashMap<&'a str, u64>,
    /// Below this a voice is diarization noise (`Options::small_voice_ms`), when another spoke for longer.
    noise_below_ms: u64,
}

type Named<'a> = HashMap<&'a str, (String, f64, By)>;

impl<'a> Voices<'a> {
    fn of(turns: &'a [Turn], opts: &Options) -> Voices<'a> {
        let (mut lines, mut length_ms) = (HashMap::<&str, Vec<usize>>::new(), HashMap::<&str, u64>::new());
        for (i, t) in turns.iter().enumerate().filter(|(_, t)| !t.label.is_empty()) {
            lines.entry(&t.label).or_default().push(i);
            *length_ms.entry(&t.label).or_default() += t.duration_ms();
        }
        let longest = length_ms.values().copied().max().unwrap_or(0);
        Voices { lines, length_ms, noise_below_ms: if longest >= opts.small_voice_ms { opts.small_voice_ms } else { 0 } }
    }

    /// A voice, and not diarization noise.
    fn real(&self, label: &str) -> bool {
        self.length_ms.get(label).is_some_and(|&l| l >= self.noise_below_ms)
    }

    /// Each voice's name by the vote of its lines, weighed by duration: the winner needs `min_share` of the named
    /// time, `voice_coverage` of all of it, and `min_overlap_ms` at least.
    fn voted(&self, turns: &[Turn], shown: &[Option<(String, f64)>], opts: &Options) -> Named<'a> {
        let mut named = Named::new();
        for (&label, lines) in &self.lines {
            let mut votes: HashMap<&str, u64> = HashMap::new();
            for &i in lines {
                if let Some((name, _)) = &shown[i] {
                    *votes.entry(name).or_default() += turns[i].duration_ms();
                }
            }
            let total: u64 = votes.values().sum();
            let Some((name, ms)) = votes.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0))) else { continue };
            let share = ms as f64 / total as f64;
            if ms >= opts.min_overlap_ms && share >= opts.min_share && ms as f64 >= opts.voice_coverage * self.length_ms[label] as f64 {
                named.insert(label, (name.to_string(), share, By::Voice));
            }
        }
        named
    }

    /// Names the voices the app never showed when only one person can be them (`only_one`): in a call with one other
    /// person, that person; in a bigger one, the last one left. A name given here can leave another voice a single
    /// person, so it repeats until nothing changes. Noise, and voices with lines shown as someone (diarization's mix of
    /// everyone's "yeah", which the vote couldn't name), are left out.
    fn eliminate(&self, named: &mut Named<'a>, turns: &[Turn], shown: &[Option<(String, f64)>], rolls: &[Roll], opts: &Options) {
        loop {
            let taken: HashSet<&str> = named.values().map(|(name, ..)| name.as_str()).collect();
            let found: Vec<(&'a str, String)> = self
                .lines
                .iter()
                .filter(|(label, lines)| !named.contains_key(*label) && self.real(label) && lines.iter().all(|&i| shown[i].is_none()))
                .filter_map(|(&label, lines)| Some((label, only_one(lines, turns, rolls, &taken, opts)?)))
                .collect();
            if found.is_empty() {
                return;
            }
            for (label, name) in found {
                named.insert(label, (name, ELIMINATED, By::Elimination));
            }
        }
    }
}

/// The one person a voice could be, if there is exactly one: listed while it spoke, not muted during most of the time
/// it was read, and not the name of another voice.
fn only_one(lines: &[usize], turns: &[Turn], rolls: &[Roll], taken: &HashSet<&str>, opts: &Options) -> Option<String> {
    let mut present: HashSet<&str> = HashSet::new();
    let mut muted_ms: HashMap<&str, u64> = HashMap::new();
    let mut read_ms = 0;
    for &i in lines {
        let Some(roll) = roll_at(rolls, turns[i].middle_ms(), opts) else { continue };
        let d = turns[i].duration_ms();
        read_ms += d;
        present.extend(roll.present.iter().map(String::as_str));
        for m in &roll.muted {
            *muted_ms.entry(m).or_default() += d;
        }
    }
    let mostly_muted = |p: &str| 2 * muted_ms.get(p).copied().unwrap_or(0) > read_ms;
    single(present.into_iter().filter(|p| !taken.contains(p) && !mostly_muted(p)))
}

/// The only person who could have spoken at `t`: the one other person listed, or the one of them not muted.
fn only_one_at(rolls: &[Roll], t: u64, opts: &Options) -> Option<String> {
    let roll = roll_at(rolls, t, opts)?;
    single(roll.present.iter().map(String::as_str).filter(|p| !roll.muted.iter().any(|m| m == p)))
}

fn single<'n>(mut names: impl Iterator<Item = &'n str>) -> Option<String> {
    let one = names.next()?;
    names.next().is_none().then(|| one.to_string())
}

/// The roll in effect at `t`: the last one before it, if it still holds.
fn roll_at<'r>(rolls: &'r [Roll], t: u64, opts: &Options) -> Option<&'r Roll> {
    let i = rolls.partition_point(|r| r.t_ms <= t).checked_sub(1)?;
    (t - rolls[i].t_ms <= opts.roll_hold_ms).then(|| &rolls[i])
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
        assert_eq!(people(&attribute(&turns, &intervals(&s, &opts), &[], &opts)), [Some("Bia"), Some("Ana"), Some("Bia")]);
    }

    #[test]
    fn a_voice_of_everyones_yeah_is_named_line_by_line() {
        let opts = Options { lag_ms: 0, ..Default::default() };
        let s: Vec<Sample> = (0..160).map(|i| sample(i * 250, &[if i < 80 { "Ana" } else { "Bia" }])).collect();
        let turns = [turn("system:1", 0, 15_000), turn("system:2", 21_000, 39_000), turn("system:3", 5_000, 5_500), turn("system:3", 30_000, 30_500)];
        let w = attribute(&turns, &intervals(&s, &opts), &[], &opts);
        assert_eq!(people(&w), [Some("Ana"), Some("Bia"), Some("Ana"), Some("Bia")]);
    }

    #[test]
    fn lines_nobody_was_shown_for_take_their_voice() {
        let opts = Options { lag_ms: 0, ..Default::default() };
        // Ana shown until 20 s; then the call can't be read (another tab).
        let s: Vec<Sample> = (0..80).map(|i| sample(i * 250, &["Ana"])).collect();
        let turns = [turn("system:1", 0, 9_000), turn("system:1", 9_000, 19_000), turn("system:1", 30_000, 34_000), turn("system:2", 40_000, 41_000), turn("system:4", 45_000, 70_000)];
        let w = attribute(&turns, &intervals(&s, &opts), &[], &opts);
        assert_eq!(w[2], Who::Person { name: "Ana".into(), share: 1.0, by: By::Voice });
        assert_eq!(w[3], Who::Nobody, "a 1 s voice is noise when another spoke for long");
        assert_eq!(w[4], Who::Voice("system:4".into()));
    }

    #[test]
    fn a_line_with_several_shown_at_once_is_nobodys() {
        let opts = Options { lag_ms: 0, ..Default::default() };
        let s: Vec<Sample> = (0..40).map(|i| sample(i * 250, &["Ana", "Bia", "Caio"])).collect();
        let turns = [turn("system:1", 1_000, 9_000)];
        assert_eq!(attribute(&turns, &intervals(&s, &opts), &[], &opts), [Who::Voice("system:1".into())]);
    }

    #[test]
    fn a_named_voice_keeps_its_name_on_a_reply_under_a_lingering_mark() {
        let opts = Options { lag_ms: 0, ..Default::default() };
        // Ana talks until 10 s; her mark stays on until 12 s while Bia answers "Okay, great." at 10.2 s.
        let s: Vec<Sample> = (0..48).map(|i| sample(i * 250, &["Ana"])).chain((48..80).map(|i| sample(i * 250, &["Bia"]))).collect();
        let turns = [turn("system:1", 0, 10_000), turn("system:2", 10_200, 11_400), turn("system:2", 12_500, 19_500)];
        assert_eq!(people(&attribute(&turns, &intervals(&s, &opts), &[], &opts)), [Some("Ana"), Some("Bia"), Some("Bia")]);
    }

    #[test]
    fn a_voice_split_in_two_gets_one_name_twice() {
        let opts = Options { lag_ms: 0, ..Default::default() };
        let s: Vec<Sample> = (0..80).map(|i| sample(i * 250, &["Ana"])).collect();
        let turns = [turn("system:1", 0, 9_000), turn("system:2", 10_000, 19_000)];
        assert_eq!(people(&attribute(&turns, &intervals(&s, &opts), &[], &opts)), [Some("Ana"), Some("Ana")]);
    }

    fn roll(t: u64, present: &[&str], muted: &[&str]) -> Roll {
        Roll { t_ms: t, present: present.iter().map(|s| s.to_string()).collect(), muted: muted.iter().map(|s| s.to_string()).collect() }
    }

    /// Every 5 s until `end_ms`, the same people and mutes.
    fn rolls(end_ms: u64, present: &[&str], muted: &[&str]) -> Vec<Roll> {
        (0..end_ms).step_by(5_000).map(|t| roll(t, present, muted)).collect()
    }

    fn by(w: &[Who]) -> Vec<Option<(&str, By)>> {
        w.iter().map(|w| match w { Who::Person { name, by, .. } => Some((name.as_str(), *by)), _ => None }).collect()
    }

    #[test]
    fn in_a_one_on_one_the_other_voice_is_the_other_person() {
        let opts = Options::default();
        // Nobody was ever shown speaking (another tab in front the whole call).
        let turns = [turn("system:1", 0, 12_000), turn("system:1", 20_000, 26_000)];
        let w = attribute(&turns, &[], &rolls(30_000, &["Ana"], &[]), &opts);
        assert_eq!(by(&w), [Some(("Ana", By::Elimination)); 2]);
    }

    #[test]
    fn the_one_not_muted_is_who_speaks() {
        let opts = Options::default();
        let turns = [turn("system:1", 0, 12_000)];
        let w = attribute(&turns, &[], &rolls(15_000, &["Ana", "Bia", "Caio"], &["Ana", "Caio"]), &opts);
        assert_eq!(by(&w), [Some(("Bia", By::Elimination))]);
    }

    #[test]
    fn the_last_voice_left_is_the_last_person_left() {
        let opts = Options { lag_ms: 0, ..Default::default() };
        // Ana and Bia shown while they spoke; then the reader broke, and a third voice spoke for 15 s.
        let s: Vec<Sample> = (0..80).map(|i| sample(i * 250, &[if i < 40 { "Ana" } else { "Bia" }])).collect();
        let turns = [turn("system:1", 0, 9_500), turn("system:2", 10_500, 19_500), turn("system:3", 30_000, 45_000)];
        let w = attribute(&turns, &intervals(&s, &opts), &rolls(50_000, &["Ana", "Bia", "Caio"], &[]), &opts);
        assert_eq!(by(&w), [Some(("Ana", By::Voice)), Some(("Bia", By::Voice)), Some(("Caio", By::Elimination))]);
    }

    #[test]
    fn a_mix_of_everyones_yeah_is_not_the_last_person_left() {
        let opts = Options { lag_ms: 0, ..Default::default() };
        // Ana and Bia talk; a voice of short replies is shown as one or the other; Caio never speaks.
        let s: Vec<Sample> = (0..160).map(|i| sample(i * 250, &[if i < 80 { "Ana" } else { "Bia" }])).collect();
        let mut turns = vec![turn("system:1", 0, 19_000), turn("system:2", 20_500, 39_500)];
        turns.extend((0..12).map(|k| turn("system:3", 1_000 + k * 3_200, 1_900 + k * 3_200)));
        let w = attribute(&turns, &intervals(&s, &opts), &rolls(40_000, &["Ana", "Bia", "Caio"], &[]), &opts);
        assert!(by(&w).iter().all(|p| !matches!(p, Some(("Caio", _)))), "{w:?}");
    }

    #[test]
    fn a_line_nobody_else_could_have_said() {
        let opts = Options::default();
        // Live: no voices. Two others in the call, one muted.
        let turns = [turn("", 1_000, 3_000), turn("", 7_000, 8_000)];
        let r = [roll(0, &["Ana", "Bia"], &["Bia"]), roll(5_000, &["Ana", "Bia"], &[])];
        assert_eq!(by(&attribute(&turns, &[], &r, &opts)), [Some(("Ana", By::Alone)), None]);
    }

    #[test]
    fn nobody_is_guessed_without_a_roll() {
        let opts = Options::default();
        let turns = [turn("system:1", 10_000, 24_000), turn("", 20_000, 21_000)];
        assert_eq!(by(&attribute(&turns, &[], &[roll(0, &["Ana"], &[])], &opts)), [None, None], "a roll holds 6 s, not the call");
    }
}
