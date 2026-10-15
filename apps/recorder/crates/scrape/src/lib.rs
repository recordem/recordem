// From an app's Accessibility tree to who is in the call and who is speaking. What to look for in each app is a
// profile (profiles/*.json): data, so a redesign of Meet or Zoom is fixed by editing JSON, and a server can ship the
// fix to recorders without a release. This crate only interprets profiles; it never touches macOS.
mod health;
mod profile;
mod selector;

pub use health::{Event, Health, Issue, Monitor, MonitorOptions, Status};
pub use profile::{Field, Participant, Profile, Roster, RosterEntry, Scrape, Versions, compare_versions};
pub use selector::Selector;

use serde::Deserialize;

/// The profiles a recorder uses: the built-in ones, replaced by newer ones (same id, higher revision) a server sent.
pub struct Profiles(Vec<Profile>);

/// What a server sends: `{"profiles": [ … ]}`.
#[derive(Deserialize)]
struct Document {
    profiles: Vec<Profile>,
}

impl Profiles {
    pub fn builtin() -> Profiles {
        let all = [include_str!("../profiles/zoom.json"), include_str!("../profiles/meet-chrome.json")];
        Profiles(all.iter().map(|j| Profile::from_json(j).expect("built-in profile")).collect())
    }

    /// Adds the profiles of a server document; a profile replaces another with its id unless that one is newer.
    /// Profiles with a new id are added (an app version the built-in ones don't cover).
    pub fn merge(&mut self, json: &str) -> anyhow::Result<usize> {
        let doc: Document = serde_json::from_str(json)?;
        let mut changed = 0;
        for p in doc.profiles {
            match self.0.iter_mut().find(|q| q.id == p.id && same_range(q, &p)) {
                Some(q) if q.revision >= p.revision => {}
                Some(q) => {
                    *q = p;
                    changed += 1;
                }
                None => {
                    self.0.push(p);
                    changed += 1;
                }
            }
        }
        Ok(changed)
    }

    /// The profiles for this app and version, most specific first (a version range beats an open one).
    pub fn for_app(&self, bundle_id: &str, version: Option<&str>) -> Vec<&Profile> {
        let mut out: Vec<&Profile> = self.0.iter().filter(|p| p.reads(bundle_id, version)).collect();
        out.sort_by_key(|p| std::cmp::Reverse((p.app_versions.min.is_some() as u8 + p.app_versions.max.is_some() as u8, p.revision)));
        out
    }

    pub fn all(&self) -> &[Profile] {
        &self.0
    }
}

fn same_range(a: &Profile, b: &Profile) -> bool {
    a.app_versions.min == b.app_versions.min && a.app_versions.max == b.app_versions.max
}

/// The built-in profiles, as a list.
pub fn builtin() -> Vec<Profile> {
    Profiles::builtin().0
}

/// Scrapes with each profile for the app and keeps the healthiest result (then the one with the most people).
pub fn scrape(profiles: &[&Profile], tree: &recordem_ax::AxNode) -> Option<Scrape> {
    profiles
        .iter()
        .filter_map(|p| p.scrape(tree))
        .min_by_key(|s| (s.health.status, std::cmp::Reverse(s.participants.len())))
}
