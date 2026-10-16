use crate::health::{Health, Issue};
use crate::selector::Selector;
use recordem_ax::{AxNode, Rect};
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

/// Text taken from an element: its attribute `attr` ("text" = all the text in it and below), from the element
/// itself or, with `from`, the first element in it that matches; with `regex`, the capture group named `v` (or the
/// whole match). In a list, the first rule that yields something wins.
#[derive(Debug, Clone, Deserialize)]
pub struct Field {
    #[serde(default)]
    pub from: Option<Selector>,
    pub attr: String,
    #[serde(default, deserialize_with = "regex_opt")]
    pub regex: Option<Regex>,
}


fn regex_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Regex>, D::Error> {
    Vec::<String>::deserialize(d)?.iter().map(|s| Regex::new(s).map_err(serde::de::Error::custom)).collect()
}

/// The person on this Mac's own mic control, read to follow the call's mute: a label that says it is muted or open.
#[derive(Debug, Clone, Deserialize)]
pub struct SelfMic {
    /// Where it is: a button in the call's window (Meet), or an item of the app's menus (Zoom's "Meeting" menu).
    #[serde(rename = "in", default)]
    pub place: MicPlace,
    /// Its label while the mic is muted ("Turn on microphone"), and while it is open ("Turn off microphone"): a regex,
    /// or "@name", one of the profile's texts (the label starts with it, any language).
    pub muted: Label,
    pub open: Label,
}

#[derive(Debug, Clone)]
pub enum Label {
    Regex(Regex),
    Text(String, Vec<String>),
}

impl Label {
    pub fn is_match(&self, s: &str) -> bool {
        match self {
            Label::Regex(re) => re.is_match(s),
            Label::Text(_, values) => crate::selector::TextHow::Starts.test(s, values),
        }
    }
}

impl<'de> Deserialize<'de> for Label {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.strip_prefix('@') {
            Some(name) => Label::Text(name.to_string(), Vec::new()),
            None => Label::Regex(Regex::new(&s).map_err(serde::de::Error::custom)?),
        })
    }
}

/// A text a profile compares with, in every language: the app's own translations of `app_key` where the app has a
/// table of them (Zoom's Localizable.strings: "LN_Accessibility_Video_Render_194601" → "Video render", "Rendu
/// vidéo"…), plus `values` (a web app has no table to read: its translations are listed). Written as a plain list
/// when it has no key.
#[derive(Debug, Clone, Default)]
pub struct Text {
    pub app_key: Option<String>,
    pub values: Vec<String>,
}

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            List(Vec<String>),
            Keyed { app_key: Option<String>, #[serde(default)] values: Vec<String> },
        }
        Ok(match Raw::deserialize(d)? {
            Raw::List(values) => Text { app_key: None, values },
            Raw::Keyed { app_key, values } => Text { app_key, values },
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MicPlace {
    #[default]
    Window,
    Menu,
}

impl SelfMic {
    /// muted (true), open (false), or another control.
    pub fn state(&self, label: &str) -> Option<bool> {
        let l = label.trim();
        if self.muted.is_match(l) {
            Some(true)
        } else if self.open.is_match(l) {
            Some(false)
        } else {
            None
        }
    }
}

fn regex_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Regex>, D::Error> {
    Option::<String>::deserialize(d)?
        .map(|s| Regex::new(&s).map_err(serde::de::Error::custom))
        .transpose()
}

impl Field {
    fn read(&self, node: &AxNode) -> Option<String> {
        let el = match &self.from {
            Some(sel) => sel.find_first(node)?,
            None => node,
        };
        let raw = if self.attr == "text" { el.texts().join(" ") } else { el.attr(&self.attr)?.to_string() };
        let out = match &self.regex {
            Some(re) => {
                let caps = re.captures(&raw)?;
                caps.name("v").or_else(|| caps.get(0))?.as_str().to_string()
            }
            None => raw,
        };
        let out = out.trim().to_string();
        (!out.is_empty()).then_some(out)
    }
}

fn first(fields: &[Field], node: &AxNode) -> Option<String> {
    fields.iter().find_map(|f| f.read(node))
}

/// The app versions a profile is written for, both ends included ("6.5", "6.5.9.61929"). Open ends are fine.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Versions {
    #[serde(default)]
    pub min: Option<String>,
    #[serde(default)]
    pub max: Option<String>,
}

/// Dotted versions compared number by number; a missing part is 0 ("6.5" == "6.5.0").
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |s: &str| s.split(['.', ' ', '(']).map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let (a, b) = (parts(a), parts(b));
    (0..a.len().max(b.len())).map(|i| a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0))).find(|o| o.is_ne()).unwrap_or(std::cmp::Ordering::Equal)
}

impl Versions {
    pub fn contains(&self, version: &str) -> bool {
        use std::cmp::Ordering::*;
        self.min.as_deref().is_none_or(|m| compare_versions(version, m) != Less)
            && self.max.as_deref().is_none_or(|m| compare_versions(version, m) != Greater)
    }
}

/// The app's own list of people (Meet's "People" panel, Zoom's participants window): names as the app knows them,
/// including people whose tile is not on screen. Read only when it is open; the recorder never opens it.
#[derive(Debug, Clone, Deserialize)]
pub struct Roster {
    /// The list itself.
    pub list: Selector,
    /// One element per person, inside the list.
    pub item: Selector,
    pub name: Vec<Field>,
    /// The item of the person on this Mac ("(You)").
    #[serde(default)]
    pub is_self: Option<Selector>,
    #[serde(default)]
    pub muted: Option<Selector>,
}

/// Where the call is when it is not on screen: a browser tab in the background. Only its tab is readable (the
/// browser exposes nothing of a background page), which says the call goes on, and its title.
#[derive(Debug, Clone, Deserialize)]
pub struct Elsewhere {
    /// The call's tab, in any window's tab strip.
    pub item: Selector,
    #[serde(default)]
    pub title: Vec<Field>,
    /// What is skipped in windows that are not the call (the pages, toolbars): only the tab strip is wanted there.
    pub prune: Selector,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    /// "meet-chrome", "zoom", …
    pub id: String,
    /// Bumped on every change: between two profiles with the same id, the higher revision wins.
    #[serde(default)]
    pub revision: u32,
    /// Bundle ids of the apps this profile reads.
    pub apps: Vec<String>,
    #[serde(default)]
    pub app_versions: Versions,
    /// Which window is the call (by default every window of the app).
    #[serde(default)]
    pub window: Option<Selector>,
    /// Elements whose contents are not read (a browser's tabs and toolbar): only their own attributes count here.
    #[serde(default)]
    pub prune: Option<Selector>,
    /// Where the tiles are, inside the window (Meet: the page's "main" landmark), so a list elsewhere that looks
    /// like tiles (the roster panel) is never read as tiles. Default: the whole window.
    #[serde(default)]
    pub stage: Option<Selector>,
    /// One element per participant. Alternatives in order of trust: the first that finds named tiles is used, and
    /// using any but the first is reported (the app changed; the profile needs an update).
    #[serde(default)]
    pub tiles: Vec<Selector>,
    #[serde(default = "deep")]
    pub tile_depth: usize,
    /// Empty in a profile that only knows the call by its links (a web app nobody reads yet).
    #[serde(default)]
    pub name: Vec<Field>,
    /// The participant is speaking when this matches the tile or an element in it.
    #[serde(default)]
    pub speaking: Option<Selector>,
    #[serde(default)]
    pub muted: Option<Selector>,
    /// Names of whoever is speaking, read outside the tiles (Zoom's "Talking: Ana, Bia" banner), comma separated.
    #[serde(default)]
    pub speakers: Vec<Field>,
    #[serde(default)]
    pub roster: Option<Roster>,
    #[serde(default)]
    pub self_name: Vec<Field>,
    #[serde(default)]
    pub title: Vec<Field>,
    #[serde(default)]
    pub elsewhere: Option<Elsewhere>,
    /// Something covers the call (a modal dialog: Chrome then hides the rest of the page from Accessibility). With
    /// nobody found, that is reported as obstructed, not as a broken profile.
    #[serde(default)]
    pub obstructed: Option<Selector>,
    /// Tile names that are not people ("Presentation", "You are presenting"…).
    #[serde(default, deserialize_with = "regex_opt")]
    pub ignore_name: Option<Regex>,
    /// What the server calls the platform ("google_meet", "zoom"…).
    #[serde(default)]
    pub platform: Option<String>,
    /// The call's links (a browser tab whose URL matches is the call's).
    #[serde(default, deserialize_with = "regex_list")]
    pub links: Vec<Regex>,
    #[serde(default)]
    pub self_mic: Option<SelfMic>,
    /// The tile of the person on this Mac: the only one that matches (Meet's lacks the controls a tile of someone
    /// else has, like "You can't unmute someone else"). Tells them apart with no roster open and no account name.
    #[serde(default)]
    pub self_tile: Option<Selector>,
    /// The texts its selectors name ("@video_render"), in every language.
    #[serde(default)]
    pub texts: HashMap<String, Text>,
}

fn deep() -> usize {
    60
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Participant {
    pub name: String,
    pub speaking: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    /// The tile, as fractions of the call window: the crop for this person's video. None: in the call (the roster
    /// says so) but not on screen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<Rect>,
    /// The tile `self_tile` says is the person on this Mac.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_self: bool,
    /// The DOM classes in the tile (web apps), sorted: what calibrate.rs looks through when "speaking" stops reading.
    #[serde(skip)]
    pub classes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RosterEntry {
    pub name: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_self: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Scrape {
    pub profile: String,
    pub revision: u32,
    /// The call can be read (its window is on this Space and shows it). When false, nobody can be read.
    pub visible: bool,
    /// The call's window shows the call, readable or not: true on another Space or minimised (a recording of that
    /// window still shows the call), false when the call is a background tab (the window shows something else,
    /// which must not be recorded).
    pub window_shows_call: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    /// The call window on screen, in points: what to capture.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_frame: Option<Rect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_name: Option<String>,
    /// Everyone in the call: the tiles, then roster names without a tile.
    pub participants: Vec<Participant>,
    /// The app's own list, when it is open.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub roster: Vec<RosterEntry>,
    pub health: Health,
}

impl Profile {
    /// Fills in the texts its selectors name: each text's own values, plus `app(key)` for a text with an app key (the
    /// installed app's translations; empty when unknown). Names no text defines are returned (a profile error).
    pub fn bind(&mut self, app: &dyn Fn(&str) -> Vec<String>) -> Vec<String> {
        let texts: HashMap<String, Vec<String>> = self
            .texts
            .iter()
            .map(|(name, t)| {
                let mut v = t.values.clone();
                if let Some(k) = &t.app_key {
                    for x in app(k) {
                        if !v.contains(&x) {
                            v.push(x);
                        }
                    }
                }
                (name.clone(), v)
            })
            .collect();
        let mut missing = Vec::new();
        self.each_selector(&mut |s| {
            let mut names = Vec::new();
            s.text_names(&mut names);
            missing.extend(names.into_iter().filter(|n| !texts.contains_key(n)));
            s.bind(&texts);
        });
        if let Some(m) = &mut self.self_mic {
            for l in [&mut m.muted, &mut m.open] {
                if let Label::Text(name, values) = l {
                    match texts.get(name.as_str()) {
                        Some(v) => *values = v.clone(),
                        None => missing.push(name.clone()),
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        missing
    }

    /// This profile with the installed app's translations: `app` gives each key's text in every language the app has.
    pub fn localized(&self, app: &dyn Fn(&str) -> Vec<String>) -> Profile {
        let mut p = self.clone();
        p.bind(app);
        p
    }

    /// The app keys of its texts (what to read from the app's translation tables).
    pub fn app_keys(&self) -> Vec<&str> {
        self.texts.values().filter_map(|t| t.app_key.as_deref()).collect()
    }

    fn each_selector(&mut self, f: &mut dyn FnMut(&mut Selector)) {
        for s in [&mut self.window, &mut self.prune, &mut self.stage, &mut self.speaking, &mut self.muted, &mut self.obstructed, &mut self.self_tile]
            .into_iter()
            .flatten()
        {
            f(s);
        }
        self.tiles.iter_mut().for_each(&mut *f);
        let fields = |fl: &mut Vec<Field>, f: &mut dyn FnMut(&mut Selector)| fl.iter_mut().filter_map(|x| x.from.as_mut()).for_each(f);
        fields(&mut self.name, f);
        fields(&mut self.speakers, f);
        fields(&mut self.self_name, f);
        fields(&mut self.title, f);
        if let Some(r) = &mut self.roster {
            f(&mut r.list);
            f(&mut r.item);
            for s in [&mut r.is_self, &mut r.muted].into_iter().flatten() {
                f(s);
            }
            fields(&mut r.name, f);
        }
        if let Some(e) = &mut self.elsewhere {
            f(&mut e.item);
            f(&mut e.prune);
            fields(&mut e.title, f);
        }
    }

    /// A profile from its JSON, its texts bound to their own values (`localized` adds the app's translations).
    pub fn from_json(json: &str) -> anyhow::Result<Profile> {
        let mut p: Profile = serde_json::from_str(json)?;
        p.check()?;
        Ok(p)
    }

    /// Binds the texts to their own values; fails when a selector names a text the profile doesn't define.
    pub fn check(&mut self) -> anyhow::Result<()> {
        let missing = self.bind(&|_| Vec::new());
        anyhow::ensure!(missing.is_empty(), "profile {}: undefined texts {}", self.id, missing.join(", "));
        Ok(())
    }

    pub fn reads(&self, bundle_id: &str, version: Option<&str>) -> bool {
        self.apps.iter().any(|a| a == bundle_id) && version.is_none_or(|v| self.app_versions.contains(v))
    }

    /// For the snapshot: read only the call window, skip what the profile prunes.
    pub fn wants_window(&self, window: &AxNode) -> bool {
        self.window.as_ref().is_none_or(|w| w.matches(window))
    }

    pub fn prunes(&self, node: &AxNode) -> bool {
        self.prune.as_ref().is_some_and(|p| p.matches(node))
    }

    /// Outside the call window: everything if the profile has no use for it.
    pub fn prunes_elsewhere(&self, node: &AxNode) -> bool {
        match &self.elsewhere {
            Some(e) => e.prune.matches(node),
            None => node.role.as_deref() == Some("AXWindow"),
        }
    }

    /// A call window the window server knows of but Accessibility can't read.
    fn away(&self, app: &AxNode, window: &AxNode) -> Scrape {
        let wrapped = AxNode { children: vec![window.clone()], ..Default::default() };
        Scrape {
            profile: self.id.clone(),
            revision: self.revision,
            visible: false,
            window_shows_call: true,
            window_title: window.title.clone(),
            window_frame: window.frame,
            title: first(&self.title, &wrapped).or_else(|| first(&self.title, app)),
            self_name: None,
            participants: Vec::new(),
            roster: Vec::new(),
            health: Health::from_issues(vec![Issue::NotVisible]),
        }
    }

    fn not_visible(&self, app: &AxNode) -> Option<Scrape> {
        let e = self.elsewhere.as_ref()?;
        let tab = e.item.find_first(app)?;
        Some(Scrape {
            profile: self.id.clone(),
            revision: self.revision,
            visible: false,
            window_shows_call: false,
            window_title: None,
            window_frame: None,
            title: first(&e.title, tab),
            self_name: None,
            participants: Vec::new(),
            roster: Vec::new(),
            health: Health::from_issues(vec![Issue::NotVisible]),
        })
    }

    /// Reads the call from an app snapshot. None when no window of the app is a call.
    pub fn scrape(&self, app: &AxNode) -> Option<Scrape> {
        match &self.window {
            // Several windows can match (and macOS adds one, "Window", with a sharing indicator while the app is being
            // captured): the call is the one with the most people in it.
            // Meet while presenting: the tab and a floating window with the tiles. The best window gives the frames
            // (one window is recorded); the others add who they show speaking, and anyone missing.
            Some(sel) => {
                let is_window = |w: &&AxNode| w.role.as_deref() == Some("AXWindow") && sel.matches(w);
                let elsewhere = |w: &&AxNode| w.subrole.as_deref() == Some(recordem_ax::ELSEWHERE);
                let mut all: Vec<Scrape> = app
                    .children
                    .iter()
                    .filter(|w| is_window(w) && !elsewhere(w))
                    .map(|w| self.scrape_window(app, w))
                    .collect();
                let Some(best) = (0..all.len())
                    .max_by_key(|&i| (all[i].participants.iter().filter(|p| p.frame.is_some()).count(), all[i].participants.len()))
                else {
                    // The call's window is on another Space or minimised, or the call is a background tab.
                    let away = app.children.iter().find(|w| is_window(w) && elsewhere(w));
                    return self.not_visible(app).or_else(|| away.map(|w| self.away(app, w)));
                };
                let mut out = all.swap_remove(best);
                for other in all.iter().filter(|o| o.health.status != crate::health::Status::Broken) {
                    for p in &other.participants {
                        match out.participants.iter_mut().find(|q| q.name == p.name) {
                            Some(q) => q.speaking |= p.speaking,
                            None => out.participants.push(Participant { frame: None, ..p.clone() }),
                        }
                    }
                }
                Some(out)
            }
            None => Some(self.scrape_window(app, app)),
        }
    }

    fn read_tiles(&self, window: &AxNode, stage: &AxNode, rule: &Selector) -> Vec<Participant> {
        let window_frame = window.frame.filter(|_| self.window.is_some());
        let mut out: Vec<Participant> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for tile in rule.find_all(stage, self.tile_depth) {
            let Some(name) = first(&self.name, tile) else { continue };
            if self.ignore_name.as_ref().is_some_and(|re| re.is_match(&name)) {
                continue;
            }
            let speaking = self.speaking.as_ref().is_some_and(|s| s.find_first(tile).is_some());
            let muted = self.muted.as_ref().map(|s| s.find_first(tile).is_some());
            // The same person shows up twice in some layouts (Zoom's speaker view: the stage and the strip); keep the
            // first tile, but they are speaking if either says so.
            match index.get(&name) {
                Some(&i) => out[i].speaking |= speaking,
                None => {
                    index.insert(name.clone(), out.len());
                    let frame = match (tile.frame, window_frame) {
                        (Some(t), Some(w)) => t.relative_to(&w),
                        _ => None,
                    };
                    let is_self = self.self_tile.as_ref().is_some_and(|s| s.matches(tile));
                    let mut classes: Vec<String> =
                        std::iter::once(tile).chain(tile.descendants().map(|(_, n)| n)).flat_map(|n| n.dom_classes.iter().cloned()).collect();
                    classes.sort();
                    classes.dedup();
                    out.push(Participant { name, speaking, muted, frame, is_self, classes });
                }
            }
        }
        out
    }

    fn read_roster(&self, window: &AxNode) -> Vec<RosterEntry> {
        let Some(r) = &self.roster else { return Vec::new() };
        let Some(list) = r.list.find_first(window) else { return Vec::new() };
        let mut out: Vec<RosterEntry> = Vec::new();
        for item in r.item.find_all(list, 8) {
            let Some(name) = first(&r.name, item) else { continue };
            if out.iter().any(|e| e.name == name) {
                continue;
            }
            out.push(RosterEntry {
                name,
                is_self: r.is_self.as_ref().is_some_and(|s| s.find_first(item).is_some()),
                muted: r.muted.as_ref().map(|s| s.find_first(item).is_some()),
            });
        }
        out
    }

    fn scrape_window(&self, app: &AxNode, window: &AxNode) -> Scrape {
        let mut issues = Vec::new();
        let mut participants = Vec::new();
        let stage = match &self.stage {
            Some(sel) => sel.find_first(window),
            None => Some(window),
        };
        for (i, rule) in self.tiles.iter().enumerate() {
            let Some(stage) = stage else { break };
            participants = self.read_tiles(window, stage, rule);
            if !participants.is_empty() {
                if i > 0 {
                    issues.push(Issue::FallbackTiles { rule: i });
                }
                break;
            }
        }
        let mut index: HashMap<String, usize> = participants.iter().enumerate().map(|(i, p)| (p.name.clone(), i)).collect();
        for banner in self.speakers.iter().filter_map(|f| f.read(window)) {
            for name in banner.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                match index.get(name) {
                    Some(&i) => participants[i].speaking = true,
                    None => {
                        index.insert(name.to_string(), participants.len());
                        participants.push(Participant { name: name.to_string(), speaking: true, muted: None, frame: None, is_self: false, classes: Vec::new() });
                    }
                }
            }
        }
        let roster = self.read_roster(window);
        if !roster.is_empty() {
            let unknown: Vec<String> =
                participants.iter().filter(|p| !roster.iter().any(|r| r.name == p.name)).map(|p| p.name.clone()).collect();
            if !unknown.is_empty() {
                // A tile name the app's own list doesn't have: the name rule reads the wrong text.
                issues.push(Issue::NotInRoster { names: unknown });
            }
            for r in &roster {
                match index.get(&r.name) {
                    Some(&i) => {
                        if participants[i].muted.is_none() {
                            participants[i].muted = r.muted;
                        }
                    }
                    None => {
                        index.insert(r.name.clone(), participants.len());
                        participants.push(Participant { name: r.name.clone(), speaking: false, muted: r.muted, frame: None, is_self: false, classes: Vec::new() });
                    }
                }
            }
        }
        if participants.is_empty() {
            match &self.obstructed {
                Some(sel) if sel.find_first(window).is_some() => issues.push(Issue::Obstructed),
                _ => issues.push(Issue::NoParticipants),
            }
        }
        if window.is_truncated() {
            issues.push(Issue::Truncated);
        }
        // The person on this Mac: the app's list says so, or a rule of the profile, or the one tile that is theirs.
        let only_self = || {
            let mut mine = participants.iter().filter(|p| p.is_self);
            match (mine.next(), mine.next()) {
                (Some(p), None) => Some(p.name.clone()),
                _ => None,
            }
        };
        let self_name = roster.iter().find(|r| r.is_self).map(|r| r.name.clone()).or_else(|| first(&self.self_name, app)).or_else(only_self);
        Scrape {
            profile: self.id.clone(),
            revision: self.revision,
            visible: true,
            window_shows_call: true,
            window_title: window.title.clone().filter(|_| self.window.is_some()),
            window_frame: window.frame.filter(|_| self.window.is_some()),
            title: first(&self.title, app),
            self_name,
            participants,
            roster,
            health: Health::from_issues(issues),
        }
    }
}
