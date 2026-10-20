// Finding, during a call, the class that means "speaking" when the profile's no longer works. A web app marks the
// tile of whoever speaks with an obfuscated class (Meet's "kssMZb", Oct 2026) that a redesign renames. The recorder
// knows something the page doesn't say: when the other side is audible (the system channel) and when the person on
// this Mac is (the mic). The class that comes on in someone else's tile while the other side is heard, and stays off
// while it isn't — and in the person's own tile with their mic — is that mark. Nothing is guessed from a name. And it
// comes on for more than one person: a class only ever on one tile tells nobody apart (Meet's shared screen, named
// after the presenter, shows its toolbar while hovered, which went with the others being heard well enough to win,
// Oct 2026).
//
// It only scores what it is given: which classes each tile has at each moment, and the two audio facts. The profile's
// own reading is scored the same way, so the recorder can tell "the profile works" from "this class works better".
use std::collections::{BTreeSet, HashMap};

/// One moment: every tile (whether it is the person on this Mac, their name, the classes), and who could be heard.
pub struct Moment<'a> {
    pub tiles: Vec<(bool, &'a str, &'a [String])>,
    pub others_audible: bool,
    pub me_audible: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub class: String,
    /// 0..1: on while heard, off while not, on one tile of the others at a time.
    pub score: f64,
}

pub struct Calibrator {
    /// Moments of each kind (heard, quiet) needed before a score means anything.
    pub enough: u32,
    /// Per class: [heard & on, heard, quiet & on, quiet, on in >1 others' tiles at once] for the others' tiles, and
    /// [me heard & on, me heard, me quiet & on, me quiet] for the own tile.
    counts: HashMap<String, [u32; 9]>,
    /// Per class, the others it came on for while they were heard, and how often.
    people: HashMap<String, HashMap<String, u32>>,
    moments: [u32; 4], // others heard, others quiet, me heard, me quiet
}

/// Enough of each kind of moment (heard, quiet) before a score means anything: ~30 s each at two reads a second.
pub const ENOUGH: u32 = 60;
/// A mark tells people apart once it came on for two of them, each while heard for at least this share of `enough`
/// (a few seconds each).
const PERSON_SHARE: f64 = 0.1;

impl Default for Calibrator {
    fn default() -> Self {
        Calibrator { enough: ENOUGH, counts: HashMap::new(), people: HashMap::new(), moments: [0; 4] }
    }
}

impl Calibrator {
    /// One that judges after `enough` moments of each kind (a short recorded call; a real one uses the default).
    pub fn with_enough(enough: u32) -> Calibrator {
        Calibrator { enough, ..Default::default() }
    }

    pub fn observe(&mut self, m: &Moment) {
        let others: Vec<(&str, &[String])> = m.tiles.iter().filter(|(me, ..)| !me).map(|(_, n, c)| (*n, *c)).collect();
        let mine: Option<&[String]> = m.tiles.iter().find(|(me, ..)| *me).map(|(.., c)| *c);
        let all: BTreeSet<&String> = m.tiles.iter().flat_map(|(.., c)| c.iter()).collect();
        self.moments[if m.others_audible { 0 } else { 1 }] += 1;
        if mine.is_some() {
            self.moments[if m.me_audible { 2 } else { 3 }] += 1;
        }
        for class in all {
            let on_others = others.iter().filter(|(_, c)| c.contains(class)).count();
            if m.others_audible {
                let people = self.people.entry(class.clone()).or_default();
                for (name, _) in others.iter().filter(|(_, c)| c.contains(class)) {
                    *people.entry(name.to_string()).or_default() += 1;
                }
            }
            let c = self.counts.entry(class.clone()).or_default();
            if m.others_audible {
                c[0] += (on_others > 0) as u32;
                c[1] += 1;
            } else {
                c[2] += (on_others > 0) as u32;
                c[3] += 1;
            }
            c[4] += (on_others > 1) as u32;
            if let Some(mine) = mine {
                let on = mine.contains(class) as u32;
                if m.me_audible {
                    c[5] += on;
                    c[6] += 1;
                } else {
                    c[7] += on;
                    c[8] += 1;
                }
            }
        }
    }

    /// Whether there were enough moments of the other side heard and quiet to judge.
    pub fn ready(&self) -> bool {
        self.moments[0] >= self.enough && self.moments[1] >= self.enough
    }

    /// The score of a class (see Candidate), or None before `ready`.
    pub fn score(&self, class: &str) -> Option<f64> {
        if !self.ready() {
            return None;
        }
        let enough_for = |n: &u32| *n as f64 >= PERSON_SHARE * self.enough as f64;
        if self.people.get(class).is_none_or(|p| p.values().filter(|n| enough_for(n)).count() < 2) {
            return Some(0.0);
        }
        let c = self.counts.get(class).copied().unwrap_or_default();
        let rate = |on: u32, n: u32| if n == 0 { 0.0 } else { on as f64 / n as f64 };
        // Missing moments count as "off": a class that only exists while someone speaks is the point.
        let heard = rate(c[0], self.moments[0]);
        let quiet = rate(c[2], self.moments[1]);
        let crowded = rate(c[4], self.moments[0] + self.moments[1]);
        // Two people do talk at once now and then: being on two tiles together costs half.
        let mut s = (heard - quiet - crowded / 2.0).max(0.0);
        // The own tile, when there is one and enough of it: the same mark must follow the person's mic.
        if self.moments[2] >= self.enough / 2 && self.moments[3] >= self.enough / 2 {
            let me = (rate(c[5], self.moments[2]) - rate(c[7], self.moments[3])).max(0.0);
            s = (2.0 * s + me) / 3.0;
        }
        Some(s)
    }

    /// The best classes, best first, scoring at least `min`.
    pub fn best(&self, min: f64) -> Vec<Candidate> {
        let mut out: Vec<Candidate> = self
            .counts
            .keys()
            .filter_map(|class| Some(Candidate { class: class.clone(), score: self.score(class)? }))
            .filter(|c| c.score >= min)
            .collect();
        out.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.class.cmp(&b.class)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cls(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Three tiles; "spk" marks whoever talks, "tile" is on every tile, "hover" flickers with nothing. The others
    /// talk in turns, the person on this Mac now and then.
    #[test]
    fn finds_the_speaking_mark() {
        let mut c = Calibrator::default();
        for t in 0..400u32 {
            let others = t % 40 < 20; // others heard half the time
            let who = (t / 40) % 2; // which of the two
            let me = !others && t % 7 == 0;
            let hover = if t % 5 == 0 { "hover" } else { "x" };
            let a = if others && who == 0 { cls(&["tile", "spk", hover]) } else { cls(&["tile", hover]) };
            let b = if others && who == 1 { cls(&["tile", "spk"]) } else { cls(&["tile"]) };
            let m = if me { cls(&["tile", "spk"]) } else { cls(&["tile"]) };
            c.observe(&Moment { tiles: vec![(false, "A", &a), (false, "B", &b), (true, "Me", &m)], others_audible: others, me_audible: me });
        }
        let best = c.best(0.5);
        assert_eq!(best.first().map(|b| b.class.as_str()), Some("spk"), "{best:?}");
        assert!(c.score("tile").unwrap() < 0.1, "on everywhere, always: no information");
        assert!(c.score("hover").unwrap() < 0.1);
    }

    /// A class lit on every remote tile while anyone talks (a "call has audio" mark) is not who speaks.
    #[test]
    fn a_mark_on_all_tiles_is_not_the_speaker() {
        let mut c = Calibrator::default();
        for t in 0..300u32 {
            let others = t % 30 < 15;
            let who = (t / 30) % 2;
            let a = if others && who == 0 { cls(&["audio", "spk"]) } else if others { cls(&["audio"]) } else { cls(&[]) };
            let b = if others && who == 1 { cls(&["audio", "spk"]) } else if others { cls(&["audio"]) } else { cls(&[]) };
            c.observe(&Moment { tiles: vec![(false, "A", &a), (false, "B", &b)], others_audible: others, me_audible: false });
        }
        assert!(c.score("spk").unwrap() > c.score("audio").unwrap());
    }

    #[test]
    fn not_before_enough_moments() {
        let mut c = Calibrator::default();
        let a = cls(&["spk"]);
        for _ in 0..10 {
            c.observe(&Moment { tiles: vec![(false, "A", &a)], others_audible: true, me_audible: false });
        }
        assert!(!c.ready() && c.best(0.0).is_empty());
    }

    /// Meet's shared screen is a tile named after the presenter, with a toolbar that shows while hovered; hovering it
    /// went with the others being heard. The ring moves between the people who talk; the toolbar never leaves the
    /// presenter (Oct 2026).
    #[test]
    fn a_mark_that_never_leaves_one_tile_is_not_the_speaker() {
        let mut c = Calibrator::default();
        for t in 0..400u32 {
            let others = t % 40 < 20;
            let who = (t / 40) % 3;
            let hovered = others && t % 40 < 18;
            let screen = if hovered { cls(&["toolbar"]) } else { cls(&[]) };
            let ring = |i| if others && who == i { cls(&["ring"]) } else { cls(&[]) };
            let (a, b, d) = (ring(0), ring(1), ring(2));
            let hadley: Vec<String> = a.iter().chain(&screen).cloned().collect();
            c.observe(&Moment { tiles: vec![(false, "Hadley", &hadley), (false, "Gabe", &b), (false, "Jefferson", &d)], others_audible: others, me_audible: false });
        }
        assert_eq!(c.score("toolbar"), Some(0.0));
        assert_eq!(c.best(0.4).first().map(|b| b.class.as_str()), Some("ring"));
    }

    /// In a call with one other person no mark can tell people apart, so none takes over from the profile.
    #[test]
    fn nothing_is_calibrated_with_one_other_person() {
        let mut c = Calibrator::default();
        for t in 0..300u32 {
            let others = t % 30 < 15;
            let a = if others { cls(&["spk"]) } else { cls(&[]) };
            c.observe(&Moment { tiles: vec![(false, "A", &a)], others_audible: others, me_audible: false });
        }
        assert!(c.best(0.01).is_empty());
    }
}
