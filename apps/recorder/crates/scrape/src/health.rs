// Is the profile still reading the app right? Apps change without notice, and a profile that silently reads
// nothing (or the wrong text) would leave voices unnamed, or misnamed, for every user. Each scrape says what looked
// wrong in it (Health); the Monitor watches scrapes over time and says when to restart the reader and what to
// report so the profile gets fixed.
use crate::profile::Scrape;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "issue", rename_all = "snake_case")]
pub enum Issue {
    /// A call window, but nobody in it: the tile rules no longer match.
    NoParticipants,
    /// Only a fallback tile rule matched (rule index): it still works, but the app changed.
    FallbackTiles { rule: usize },
    /// Tile names the app's own roster doesn't have: the name rule reads the wrong text.
    NotInRoster { names: Vec<String> },
    /// The snapshot hit its limits: people may be missing.
    Truncated,
    /// Nobody found, but something covers the call (a dialog): the app's state, not the profile's fault.
    Obstructed,
    /// The call goes on in a background tab: nothing of it can be read, nor recorded.
    NotVisible,
    /// Over time: the other side was heard for a while and nobody was ever shown speaking.
    SpeakingSignalLost { silent_ms: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Healthy,
    /// Works, but something changed: report it.
    Degraded,
    /// Doesn't work: its results must not be used.
    Broken,
}

impl Issue {
    pub fn status(&self) -> Status {
        match self {
            Issue::NoParticipants | Issue::SpeakingSignalLost { .. } => Status::Broken,
            Issue::FallbackTiles { .. } | Issue::NotInRoster { .. } | Issue::Truncated | Issue::Obstructed | Issue::NotVisible => Status::Degraded,
        }
    }

    /// The same issue from one scrape to the next, whatever its details.
    fn kind(&self) -> std::mem::Discriminant<Issue> {
        std::mem::discriminant(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Health {
    pub status: Status,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<Issue>,
}

impl Health {
    pub fn from_issues(issues: Vec<Issue>) -> Health {
        Health { status: issues.iter().map(Issue::status).max().unwrap_or(Status::Healthy), issues }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Broken for a while: snapshot afresh (Chromium sometimes stops updating its tree until asked again).
    Restart,
    /// Tell the server, once per issue per call: profile id and revision, app version, what went wrong.
    Report { profile: String, revision: u32, issue: Issue },
}

#[derive(Debug, Clone, Copy)]
pub struct MonitorOptions {
    /// Broken this long in a row → Restart.
    pub restart_after_ms: u64,
    /// Remote audio this long with nobody shown speaking → SpeakingSignalLost.
    pub speaking_lost_after_ms: u64,
    /// An issue this long in a row is reported (a single odd scrape, mid-redraw, is not).
    pub report_after_ms: u64,
}

impl Default for MonitorOptions {
    fn default() -> Self {
        MonitorOptions { restart_after_ms: 10_000, speaking_lost_after_ms: 30_000, report_after_ms: 5_000 }
    }
}

#[derive(Default)]
pub struct Monitor {
    pub options: MonitorOptions,
    broken_since: Option<u64>,
    /// When remote audio started without anyone shown speaking since.
    unattributed_since: Option<u64>,
    issue_since: Vec<(std::mem::Discriminant<Issue>, u64)>,
    reported: Vec<std::mem::Discriminant<Issue>>,
}

impl Monitor {
    pub fn new(options: MonitorOptions) -> Monitor {
        Monitor { options, ..Default::default() }
    }

    /// One scrape at `t_ms`; `remote_audio`: whether the other side was audible just now (the recorder knows the
    /// system channel's level), None if unknown.
    pub fn observe(&mut self, t_ms: u64, scrape: &Scrape, remote_audio: Option<bool>) -> Vec<Event> {
        let o = self.options;
        let mut issues = scrape.health.issues.clone();
        // The user's doing (a dialog, another tab): nothing is wrong with the profile, and no evidence either way.
        if issues.iter().any(|i| matches!(i, Issue::Obstructed | Issue::NotVisible)) {
            self.unattributed_since = None;
            self.broken_since = None;
            return Vec::new();
        }
        let anyone_speaking = scrape.participants.iter().any(|p| p.speaking);
        match (remote_audio, anyone_speaking) {
            (_, true) => self.unattributed_since = None,
            (Some(true), false) => {
                let since = *self.unattributed_since.get_or_insert(t_ms);
                if t_ms - since >= o.speaking_lost_after_ms {
                    issues.push(Issue::SpeakingSignalLost { silent_ms: t_ms - since });
                }
            }
            _ => {}
        }
        let mut events = Vec::new();
        if issues.iter().any(|i| i.status() == Status::Broken) {
            let since = *self.broken_since.get_or_insert(t_ms);
            if t_ms - since >= o.restart_after_ms {
                events.push(Event::Restart);
                self.broken_since = Some(t_ms);
            }
        } else {
            self.broken_since = None;
        }
        self.issue_since.retain(|(k, _)| issues.iter().any(|i| i.kind() == *k));
        for issue in issues {
            let since = match self.issue_since.iter().find(|(k, _)| *k == issue.kind()) {
                Some((_, s)) => *s,
                None => {
                    self.issue_since.push((issue.kind(), t_ms));
                    t_ms
                }
            };
            let long = t_ms - since >= o.report_after_ms || matches!(issue, Issue::SpeakingSignalLost { .. });
            if long && !self.reported.contains(&issue.kind()) {
                self.reported.push(issue.kind());
                events.push(Event::Report { profile: scrape.profile.clone(), revision: scrape.revision, issue });
            }
        }
        events
    }
}
