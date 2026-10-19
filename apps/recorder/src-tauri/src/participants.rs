// Who is in the call and who is speaking, while it is recorded: the call app's Accessibility tree, read twice a second
// through the scrape profiles (crates/scrape), goes to speakers.jsonl in the recording's folder. After the call the
// voices diarization found ("system:1", …) are named after whoever the app showed speaking while each one spoke
// (crates/timeline). Best effort, like mute.rs: without the Accessibility permission, or for an app no profile reads,
// nothing is written and the transcript keeps "system:N". Each line is named on its own (crates/timeline), so a
// voice diarization merged or split still gets the right people.
use crate::api::{Participant, Participants, Segment, SpeakerName};
use recordem_ax::{AxNode, Limits, Scope};
use recordem_scrape::{Monitor, Profile, Scrape};
use recordem_timeline::{Options, Roll, Sample, Turn, Who};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const POLL: Duration = Duration::from_millis(500);
/// A line is written when something changed, and at least this often (a gap in the file means "not read").
const HEARTBEAT: Duration = Duration::from_secs(5);

/// One line of speakers.jsonl.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    /// From the start of the recording.
    pub t_ms: u64,
    /// The call could be read (false: another tab, a dialog in front…; nobody is known to speak then).
    pub visible: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speaking: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub people: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub muted: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_name: Option<String>,
}

impl Line {
    /// As the live transcript sends it.
    pub fn participants(&self) -> Participants {
        let people = self
            .people
            .iter()
            .map(|n| Participant {
                name: n.clone(),
                speaking: self.speaking.contains(n),
                muted: self.muted.contains(n).then_some(true),
                is_self: self.self_name.as_ref() == Some(n),
            })
            .collect();
        Participants { t_ms: self.t_ms, visible: self.visible, people }
    }
}

/// The last line read, for the live transcript to send along.
pub type Latest = Arc<Mutex<Option<Line>>>;

/// When the other side was last audible (unix ms; the capture sets it): tells the Monitor a call with nobody shown
/// speaking is a broken profile, not a quiet call.
pub static REMOTE_AUDIBLE_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// When the person on this Mac was last audible (their mic, echo removed, not muted): calibrates their own tile.
pub static ME_AUDIBLE_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The profile's own reading of "speaking", scored like a class (calibrate.rs) to compare with the classes.
const PROFILE_MARK: &str = "@profile";
/// A class replaces the profile's reading when it scores at least this, and clearly better than the profile.
const TAKE_OVER: f64 = 0.4;
const MARGIN: f64 = 0.3;

fn report(t_ms: u64, s: &Scrape, version: Option<String>, event: serde_json::Value) {
    let mut r = REPORTS.lock().unwrap();
    if r.len() < 100 {
        r.push(crate::api::ReaderReport { t_ms, profile: s.profile.clone(), revision: s.revision, app_version: version, event });
    }
}

/// What the Monitor found wrong with the profile, for the live transcript to report (the server logs and keeps it,
/// so a change in Meet or Zoom is seen before anyone complains).
pub static REPORTS: Mutex<Vec<crate::api::ReaderReport>> = Mutex::new(Vec::new());

fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn snapshot(pid: i32, profiles: &[&Profile]) -> Option<AxNode> {
    let window = |w: &AxNode| profiles.iter().any(|p| p.wants_window(w));
    let prune = |n: &AxNode| profiles.iter().all(|p| p.prunes(n));
    let prune_elsewhere = |n: &AxNode| profiles.iter().all(|p| p.prunes_elsewhere(n));
    recordem_ax::snapshot(pid, Limits::default(), &Scope { window: &window, prune: &prune, prune_elsewhere: &prune_elsewhere })
}

fn line(t_ms: u64, s: &Scrape) -> Line {
    Line {
        t_ms,
        visible: s.visible,
        speaking: s.participants.iter().filter(|p| p.speaking).map(|p| p.name.clone()).collect(),
        people: s.participants.iter().map(|p| p.name.clone()).collect(),
        muted: s.participants.iter().filter(|p| p.muted == Some(true)).map(|p| p.name.clone()).collect(),
        self_name: s.self_name.clone(),
    }
}

/// Watches `bundle` (the call's app) on a thread of its own until `stop`; `started` is the recording's sample 0. Each
/// line read also goes to `latest`.
pub fn watch(bundle: String, dir: PathBuf, started: Instant, stop: CancellationToken, latest: Latest) {
    if !recordem_ax::is_trusted() {
        return;
    }
    let all = crate::profiles::current();
    if all.for_app(&bundle, None).is_empty() {
        return;
    }
    REPORTS.lock().unwrap().clear();
    std::thread::spawn(move || {
        let Ok(mut file) = std::fs::File::create(dir.join("speakers.jsonl")) else { return };
        let mut monitor = Monitor::default();
        let (mut last, mut written) = (None::<Line>, Instant::now() - HEARTBEAT);
        let mut pid: Option<(i32, Option<String>)> = None;
        let mut localized: Option<(Option<String>, Vec<Profile>)> = None;
        // When the profile's "speaking" stops reading, the class that does (found during the call; see calibrate.rs).
        let mut calibrator = recordem_scrape::calibrate::Calibrator::default();
        let mut speaking_class: Option<String> = None;
        let mut judged = Instant::now();
        let mut last_good: Option<(u64, recordem_ax::AxNode)> = None;
        while !stop.is_cancelled() {
            let tick = Instant::now();
            // The app may restart during the call (Teams does): look it up again whenever it is gone.
            if pid.is_none() {
                pid = recordem_ax::pid_of(&bundle).map(|p| (p, recordem_ax::app_version(p)));
            }
            // The profiles for this app version, with the app's own translations of their texts.
            if pid.is_some() && localized.as_ref().map(|(v, _)| v) != pid.as_ref().map(|(_, v)| v) {
                let version = pid.as_ref().and_then(|(_, v)| v.clone());
                let found = all.for_app(&bundle, version.as_deref());
                let keys: Vec<&str> = found.iter().flat_map(|p| p.app_keys()).collect();
                let table = crate::apptexts::table(&bundle, version.as_deref(), &keys);
                let mine: Vec<Profile> = found.iter().map(|p| p.localized(&|k| table.get(k).cloned().unwrap_or_default())).collect();
                localized = Some((version, mine));
            }
            let read = pid.as_ref().zip(localized.as_ref()).and_then(|((p, _), (_, mine))| {
                let profiles: Vec<&Profile> = mine.iter().collect();
                snapshot(*p, &profiles).and_then(|tree| recordem_scrape::scrape(&profiles, &tree).map(|s| (tree, s)))
            });
            if read.is_none() {
                pid = None;
            }
            let t_ms = started.elapsed().as_millis() as u64;
            if let Some((tree, mut s)) = read {
                let profile = localized.as_ref().and_then(|(_, mine)| mine.iter().find(|p| p.id == s.profile));
                let now = unix_ms();
                let audible = now.saturating_sub(REMOTE_AUDIBLE_AT.load(std::sync::atomic::Ordering::Relaxed)) < 1_500;
                let me_audible = now.saturating_sub(ME_AUDIBLE_AT.load(std::sync::atomic::Ordering::Relaxed)) < 1_500;
                let version = pid.as_ref().and_then(|(_, v)| v.clone());
                // The last good look at the call, and next to it the one where reading broke (dumps.rs).
                if s.health.status == recordem_scrape::Status::Healthy && last_good.as_ref().is_none_or(|(t, _)| t_ms >= t + 10_000) {
                    last_good = profile.and_then(|p| p.excerpt(&tree)).map(|e| (t_ms, e));
                }
                let mut broke = false;
                for event in monitor.observe(t_ms, &s, Some(audible)) {
                    eprintln!("participants: {}", serde_json::to_string(&event).unwrap_or_default());
                    broke |= matches!(event, recordem_scrape::Event::Report { .. });
                    report(t_ms, &s, version.clone(), serde_json::to_value(&event).unwrap_or_default());
                }
                // Calibration: what each on-screen tile showed, with the profile's own reading as one more mark.
                if s.visible && s.participants.iter().any(|p| !p.classes.is_empty()) {
                    let marked: Vec<(bool, Vec<String>)> = s
                        .participants
                        .iter()
                        .filter(|p| p.frame.is_some())
                        .map(|p| (p.is_self, p.classes.iter().cloned().chain(p.speaking.then(|| PROFILE_MARK.to_string())).collect()))
                        .collect();
                    calibrator.observe(&recordem_scrape::calibrate::Moment {
                        tiles: marked.iter().map(|(me, c)| (*me, c.as_slice())).collect(),
                        others_audible: audible,
                        me_audible,
                    });
                    if calibrator.ready() && judged.elapsed() >= Duration::from_secs(10) {
                        judged = Instant::now();
                        let profile = calibrator.score(PROFILE_MARK).unwrap_or(0.0);
                        let best = calibrator.best(TAKE_OVER).into_iter().find(|c| c.class != PROFILE_MARK);
                        let pick = best.filter(|b| b.score >= profile + MARGIN).map(|b| (b.class, b.score));
                        if pick.as_ref().map(|(c, _)| c) != speaking_class.as_ref() {
                            if let Some((class, score)) = &pick {
                                eprintln!("participants: speaking read from class {class} ({score:.2}; the profile's {profile:.2})");
                                report(t_ms, &s, version.clone(), serde_json::json!({ "event": "calibrated", "speaking_class": class, "score": score, "profile_score": profile }));
                            }
                            broke |= pick.is_some();
                            speaking_class = pick.map(|(c, _)| c);
                        }
                    }
                }
                if broke && let Some(bad) = profile.and_then(|p| p.excerpt(&tree)) {
                    crate::dumps::keep(&dir, t_ms, last_good.as_ref().map(|(_, g)| g), &bad);
                }
                if let Some(class) = &speaking_class {
                    for p in s.participants.iter_mut().filter(|p| !p.classes.is_empty()) {
                        p.speaking = p.classes.iter().any(|c| c == class);
                    }
                }
                let l = line(t_ms, &s);
                *latest.lock().unwrap() = Some(l.clone());
                let same = last.as_ref().is_some_and(|p| Line { t_ms, ..p.clone() } == l);
                if !same || written.elapsed() >= HEARTBEAT {
                    if writeln!(file, "{}", serde_json::to_string(&l).unwrap_or_default()).is_err() {
                        return;
                    }
                    written = Instant::now();
                    last = Some(l);
                }
            }
            std::thread::sleep(POLL.saturating_sub(tick.elapsed()));
        }
    });
}

/// The person on this Mac: who the app says ("You" in Meet's participant list, only while it is open), or else the
/// name they signed in to Recordem with. Their voice is the mic channel, never a "system" voice.
fn me(lines: &[Line]) -> Option<String> {
    lines.iter().rev().find_map(|l| l.self_name.clone()).or_else(|| {
        let account = crate::config::load().account;
        account.split(" (").next().map(str::trim).filter(|n| !n.is_empty()).map(String::from)
    })
}

/// speakers.jsonl as timeline samples: who the app showed speaking, minus the person on this Mac. Lines are written
/// on a change and as a heartbeat, so each holds until the next; a time the call couldn't be read holds nobody.
fn samples(lines: &[Line], me: Option<&str>) -> Vec<Sample> {
    let s: Vec<Sample> = lines
        .iter()
        .map(|l| Sample {
            t_ms: l.t_ms,
            speaking: if l.visible { l.speaking.iter().filter(|n| Some(n.as_str()) != me).cloned().collect() } else { vec![] },
        })
        .collect();
    recordem_timeline::held(&s, 250, HEARTBEAT.as_millis() as u64 + 1_000)
}

/// speakers.jsonl as who could be speaking: everyone listed so far but the person on this Mac (a tile the layout
/// hides is still in the call, and counting someone who left only makes a guess rarer), and who was shown muted. A
/// time the call couldn't be read lists nobody.
fn rolls(lines: &[Line], me: Option<&str>) -> Vec<Roll> {
    let mut seen: Vec<String> = vec![];
    lines
        .iter()
        .map(|l| {
            for p in &l.people {
                if Some(p.as_str()) != me && !seen.contains(p) {
                    seen.push(p.clone());
                }
            }
            if !l.visible {
                return Roll { t_ms: l.t_ms, ..Default::default() };
            }
            Roll { t_ms: l.t_ms, present: seen.clone(), muted: l.muted.clone() }
        })
        .collect()
}

/// The evidence speakers.jsonl gives `crates/timeline`: who was shown speaking, and who could have been.
fn evidence(lines: &[Line], opts: &Options) -> (Vec<recordem_timeline::Interval>, Vec<Roll>) {
    let me = me(lines);
    (recordem_timeline::intervals(&samples(lines, me.as_deref()), opts), rolls(lines, me.as_deref()))
}

pub fn read(dir: &Path) -> Vec<Line> {
    std::fs::read_to_string(dir.join("speakers.jsonl"))
        .map(|s| s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
        .unwrap_or_default()
}

/// Names the other side's lines after the pass after the call (`crates/timeline::attribute`): each line gets its
/// voice's name, else the person shown speaking during it, else the only person it could be. Lines are relabelled one label per person, then per
/// voice nobody was named for ("system:1", "system:2", … by first appearance); a line of a voice too small to be one
/// gets no label. The names, with how clear the evidence was on average (by time) as the confidence, and the person on
/// this Mac's for the mic lines.
pub fn name_lines(lines: &[Line], segments: &mut [Segment]) -> Vec<SpeakerName> {
    let opts = Options::default();
    let (intervals, rolls) = evidence(lines, &opts);
    let system: Vec<usize> = (0..segments.len()).filter(|&i| segments[i].channel == "system").collect();
    let turns: Vec<Turn> = system
        .iter()
        .map(|&i| Turn { label: segments[i].speaker.clone().unwrap_or_default(), start_ms: segments[i].start_ms, end_ms: segments[i].end_ms })
        .collect();
    let who = recordem_timeline::attribute(&turns, &intervals, &rolls, &opts);
    let mut keys: Vec<(bool, String)> = vec![]; // (a person, their name or the voice's label), by first appearance
    let mut clarity: std::collections::HashMap<String, (f64, u64)> = Default::default();
    for (&i, w) in system.iter().zip(who) {
        let key = match w {
            Who::Person { name, share, .. } => {
                let d = segments[i].end_ms.saturating_sub(segments[i].start_ms).max(1);
                let c = clarity.entry(name.clone()).or_default();
                c.0 += share * d as f64;
                c.1 += d;
                (true, name)
            }
            Who::Voice(label) => (false, label),
            Who::Nobody => {
                segments[i].speaker = None;
                continue;
            }
        };
        let k = keys.iter().position(|x| *x == key).unwrap_or_else(|| {
            keys.push(key);
            keys.len() - 1
        });
        segments[i].speaker = Some(format!("system:{}", k + 1));
    }
    let others = keys.iter().enumerate().filter(|(_, (person, _))| *person).map(|(k, (_, name))| {
        let (w, d) = clarity[name];
        SpeakerName { label: format!("system:{}", k + 1), name: name.clone(), source: "platform".into(), confidence: Some((w / d as f64 * 100.0).round() / 100.0) }
    });
    let mine = me(lines)
        .filter(|_| segments.iter().any(|s| s.channel == "mic"))
        .map(|name| SpeakerName { label: "mic".into(), name, source: "platform".into(), confidence: None });
    others.chain(mine).collect()
}

/// While the call goes on, each line of the other side is named after whoever the call app showed speaking during it
/// (`name_lines`' rule, without voices), or after the only person who could have said it: "Ana Souza" becomes
/// "system:1", the next name "system:2", by first appearance. Other lines stay unnamed. The pass after the call
/// replaces all of it.
#[derive(Default)]
pub struct LiveVoices {
    names: Vec<String>,
}

impl LiveVoices {
    /// Names the system lines of `segs` that have no speaker yet. True when a name appeared that wasn't there before.
    pub fn label(&mut self, lines: &[Line], segs: &mut [Segment]) -> bool {
        let opts = Options::default();
        let (intervals, rolls) = evidence(lines, &opts);
        let before = self.names.len();
        for s in segs.iter_mut().filter(|s| s.channel == "system" && s.speaker.is_none()) {
            let turn = Turn { label: String::new(), start_ms: s.start_ms, end_ms: s.end_ms };
            let Some(Who::Person { name, .. }) = recordem_timeline::attribute(&[turn], &intervals, &rolls, &opts).pop() else { continue };
            let k = self.names.iter().position(|n| *n == name).unwrap_or_else(|| {
                self.names.push(name);
                self.names.len() - 1
            });
            s.speaker = Some(format!("system:{}", k + 1));
        }
        self.names.len() > before
    }

    pub fn speakers(&self) -> Vec<SpeakerName> {
        self.names
            .iter()
            .enumerate()
            .map(|(k, n)| SpeakerName { label: format!("system:{}", k + 1), name: n.clone(), source: "platform".into(), confidence: None })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(label: &str, s: u64, e: u64) -> Segment {
        Segment { start_ms: s, end_ms: e, channel: "system".into(), text: "…".into(), language: None, speaker: Some(label.into()) }
    }

    #[test]
    fn voices_are_named_and_self_is_left_out() {
        let mut lines = vec![];
        for t in (0..10_000).step_by(500) {
            // The person on this Mac talks over Ana now and then: never a system voice.
            let speaking = if t % 2000 == 0 { vec!["Ana Souza".into(), "Bruno Lima".into()] } else { vec!["Ana Souza".into()] };
            lines.push(Line { t_ms: t, visible: true, speaking, people: vec![], muted: vec![], self_name: Some("Bruno Lima".into()) });
        }
        for t in (10_000..20_000).step_by(500) {
            lines.push(Line { t_ms: t, visible: true, speaking: vec!["Carla Dias".into()], people: vec![], muted: vec![], self_name: None });
        }
        let mic = Segment { channel: "mic".into(), ..seg("mic", 2_000, 3_000) };
        let mut segs = [seg("system:1", 300, 9_800), seg("system:2", 10_300, 19_800), seg("system:3", 4_000, 4_400), mic];
        let names: std::collections::HashMap<_, _> = name_lines(&lines, &mut segs).into_iter().map(|n| (n.label, n.name)).collect();
        assert_eq!(names.get("system:1").map(String::as_str), Some("Ana Souza"));
        assert_eq!(names.get("system:2").map(String::as_str), Some("Carla Dias"));
        assert_eq!(names.get("mic").map(String::as_str), Some("Bruno Lima"), "the person on this Mac");
        assert_eq!(segs[2].speaker.as_deref(), Some("system:1"), "a voice of one short line still gets who was shown");
    }

    #[test]
    fn a_one_on_one_is_named_without_anyone_shown_speaking() {
        // Another tab in front: the call is listed, nobody is shown speaking.
        let lines: Vec<Line> = (0..30)
            .map(|i| Line { t_ms: i * 1_000, visible: true, speaking: vec![], people: vec!["Ana Souza".into(), "Bruno Lima".into()], muted: vec![], self_name: Some("Bruno Lima".into()) })
            .collect();
        let mut segs = [seg("system:1", 1_000, 13_000), seg("system:1", 15_000, 25_000)];
        let names = name_lines(&lines, &mut segs);
        assert_eq!(names.iter().map(|n| (n.label.as_str(), n.name.as_str())).collect::<Vec<_>>(), [("system:1", "Ana Souza")], "no mic lines, no mic name");
        assert_eq!(names[0].confidence, Some(0.5), "a guess, not something the app showed");
    }

    #[test]
    fn live_lines_are_named_by_first_appearance() {
        let mut lines = vec![];
        for t in (0..20_000).step_by(500) {
            let who = if t < 10_000 { "Carla Dias" } else { "Ana Souza" };
            lines.push(Line { t_ms: t, visible: t < 15_000, speaking: vec![who.into()], people: vec![], muted: vec![], self_name: None });
        }
        let unnamed = |s, e| Segment { speaker: None, ..seg("", s, e) };
        let mut segs = vec![unnamed(1_000, 4_000), unnamed(10_500, 13_000), unnamed(5_000, 8_000), unnamed(16_000, 19_000)];
        let mut v = LiveVoices::default();
        assert!(v.label(&lines, &mut segs));
        let got: Vec<_> = segs.iter().map(|s| s.speaker.as_deref()).collect();
        // The call couldn't be read after 15 s: that line stays unnamed.
        assert_eq!(got, [Some("system:1"), Some("system:2"), Some("system:1"), None]);
        assert_eq!(v.speakers().iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["Carla Dias", "Ana Souza"]);
        assert!(!v.label(&lines, &mut segs));
    }

    /// A recorded call's names, again: `RECORDEM_CALL_DIR=<recording folder> cargo test --release -- --ignored
    /// names_of_a_call --nocapture` (speakers.jsonl and transcript.json from the recorder's folder). The transcript
    /// with the new names goes to transcript.named.json there, to send in place of the old one.
    #[test]
    #[ignore]
    fn names_of_a_call() {
        let dir = std::path::PathBuf::from(std::env::var("RECORDEM_CALL_DIR").expect("RECORDEM_CALL_DIR"));
        let t: crate::api::Transcript = serde_json::from_slice(&std::fs::read(dir.join("transcript.json")).unwrap()).unwrap();
        let lines = read(&dir);
        let opts = Options::default();
        let (intervals, _) = evidence(&lines, &opts);
        let mut by: std::collections::BTreeMap<(String, String), u64> = Default::default();
        for s in t.segments.iter().filter(|s| s.channel == "system") {
            for iv in &intervals {
                let o = s.end_ms.min(iv.end_ms).saturating_sub(s.start_ms.max(iv.start_ms));
                *by.entry((s.speaker.clone().unwrap_or_default(), iv.name.clone())).or_default() += o;
            }
        }
        eprintln!("overlap: {by:?}");
        let mut segments = t.segments.clone();
        let speakers = name_lines(&lines, &mut segments);
        for n in &speakers {
            let lines = segments.iter().filter(|s| s.speaker.as_deref() == Some(n.label.as_str())).count();
            eprintln!("{} {} confidence {:?}, {lines} lines", n.label, n.name, n.confidence);
        }
        let named = crate::api::Transcript { segments, speakers, ..t };
        std::fs::write(dir.join("transcript.named.json"), serde_json::to_vec(&named).unwrap()).unwrap();
    }

    #[test]
    fn unreadable_times_are_no_evidence() {
        let lines: Vec<Line> = (0..20).map(|i| Line { t_ms: i * 500, visible: false, speaking: vec![], people: vec![], muted: vec![], self_name: None }).collect();
        let mut segs = [seg("system:1", 0, 9_000)];
        assert!(name_lines(&lines, &mut segs).is_empty());
        assert_eq!(segs[0].speaker.as_deref(), Some("system:1"), "the voice stays");
    }
}
