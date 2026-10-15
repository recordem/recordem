// Who is in the call and who is speaking, while it is recorded: the call app's Accessibility tree, read twice a second
// through the scrape profiles (crates/scrape), goes to speakers.jsonl in the recording's folder. After the call the
// voices diarization found ("system:1", …) are named after whoever the app showed speaking while each one spoke
// (crates/timeline). Best effort, like mute.rs: without the Accessibility permission, or for an app no profile reads,
// nothing is written and the transcript keeps "system:N".
use crate::api::{Participant, Participants, Segment, SpeakerName};
use recordem_ax::{AxNode, Limits, Scope};
use recordem_scrape::{Monitor, Profile, Profiles, Scrape};
use recordem_timeline::{Assignment, Options, Sample, Turn};
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
    let all = Profiles::builtin();
    if all.for_app(&bundle, None).is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let Ok(mut file) = std::fs::File::create(dir.join("speakers.jsonl")) else { return };
        let mut monitor = Monitor::default();
        let (mut last, mut written) = (None::<Line>, Instant::now() - HEARTBEAT);
        let mut pid: Option<(i32, Option<String>)> = None;
        while !stop.is_cancelled() {
            let tick = Instant::now();
            // The app may restart during the call (Teams does): look it up again whenever it is gone.
            if pid.is_none() {
                pid = recordem_ax::pid_of(&bundle).map(|p| (p, recordem_ax::app_version(p)));
            }
            let read = pid.as_ref().and_then(|(p, version)| {
                let profiles = all.for_app(&bundle, version.as_deref());
                snapshot(*p, &profiles).and_then(|tree| recordem_scrape::scrape(&profiles, &tree))
            });
            if read.is_none() {
                pid = None;
            }
            let t_ms = started.elapsed().as_millis() as u64;
            if let Some(s) = read {
                for event in monitor.observe(t_ms, &s, None) {
                    // Reported to the server once that route exists; for now the log says when a profile broke.
                    eprintln!("participants: {}", serde_json::to_string(&event).unwrap_or_default());
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

/// speakers.jsonl as timeline samples: who the app showed speaking, minus the person on this Mac (their voice is the
/// mic channel, never a "system" voice).
fn samples(lines: &[Line]) -> Vec<Sample> {
    let me = lines.iter().rev().find_map(|l| l.self_name.clone());
    lines
        .iter()
        .filter(|l| l.visible)
        .map(|l| Sample { t_ms: l.t_ms, speaking: l.speaking.iter().filter(|n| Some(*n) != me.as_ref()).cloned().collect() })
        .collect()
}

pub fn read(dir: &Path) -> Vec<Line> {
    std::fs::read_to_string(dir.join("speakers.jsonl"))
        .map(|s| s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
        .unwrap_or_default()
}

/// Names for the voices of the other side, from the segments the pass after the call made: "system:1" → "Ana Souza".
/// Only clear ones (crates/timeline): a voice nobody clearly matches stays unnamed.
pub fn name_voices(lines: &[Line], segments: &[Segment]) -> Vec<Assignment> {
    let turns: Vec<Turn> = segments
        .iter()
        .filter(|s| s.channel == "system")
        .filter_map(|s| Some(Turn { label: s.speaker.clone()?, start_ms: s.start_ms, end_ms: s.end_ms }))
        .collect();
    let opts = Options::default();
    recordem_timeline::assign(&turns, &recordem_timeline::intervals(&samples(lines), &opts), &opts)
}

/// The names for the transcript; how clear the overlap was is the confidence.
pub fn names(assignments: &[Assignment]) -> Vec<SpeakerName> {
    assignments
        .iter()
        .map(|a| SpeakerName { label: a.label.clone(), name: a.name.clone(), source: "platform".into(), confidence: Some((a.share * 100.0).round() / 100.0) })
        .collect()
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
        let names: std::collections::HashMap<_, _> = names(&name_voices(&lines, &[seg("system:1", 300, 9_800), seg("system:2", 10_300, 19_800)])).into_iter().map(|n| (n.label, n.name)).collect();
        assert_eq!(names.get("system:1").map(String::as_str), Some("Ana Souza"));
        assert_eq!(names.get("system:2").map(String::as_str), Some("Carla Dias"));
    }

    #[test]
    fn unreadable_times_are_no_evidence() {
        let lines: Vec<Line> = (0..20).map(|i| Line { t_ms: i * 500, visible: false, speaking: vec![], people: vec![], muted: vec![], self_name: None }).collect();
        assert!(name_voices(&lines, &[seg("system:1", 0, 9_000)]).is_empty());
    }
}
