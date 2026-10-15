// Real Google Meet trees (Chrome, pt-BR, October 2026), names and meeting code replaced. Taken with `probe dump`
// during a scripted call: silence, Ana speaks, Bruno speaks.
use recordem_ax::AxNode;

fn scrape(fixture: &str) -> Vec<(String, bool)> {
    let path = format!("{}/fixtures/meet-chrome/{fixture}.json", env!("CARGO_MANIFEST_DIR"));
    let tree: AxNode = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let meet = recordem_scrape::builtin().into_iter().find(|p| p.id == "meet-chrome").unwrap();
    let s = meet.scrape(&tree).expect("a call");
    assert_eq!(s.title.as_deref(), Some("abc-defg-hij"));
    assert!(s.participants.iter().all(|p| p.frame.is_some()), "every tile has a crop");
    s.participants.into_iter().map(|p| (p.name, p.speaking)).collect()
}

fn owned(v: &[(&str, bool)]) -> Vec<(String, bool)> {
    v.iter().map(|(n, s)| (n.to_string(), *s)).collect()
}

#[test]
fn silent() {
    assert_eq!(scrape("silent"), owned(&[("Ana Souza", false), ("Bruno Lima", false)]));
}

#[test]
fn ana_speaking() {
    assert_eq!(scrape("ana-speaking"), owned(&[("Ana Souza", true), ("Bruno Lima", false)]));
}

#[test]
fn bruno_speaking() {
    assert_eq!(scrape("bruno-speaking"), owned(&[("Ana Souza", false), ("Bruno Lima", true)]));
}

fn load(fixture: &str) -> AxNode {
    let path = format!("{}/fixtures/meet-chrome/{fixture}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn meet() -> recordem_scrape::Profile {
    recordem_scrape::builtin().into_iter().find(|p| p.id == "meet-chrome").unwrap()
}

/// Google renames its obfuscated classes: what the next Meet deploy looks like.
fn rename_classes(node: &mut AxNode) {
    for c in &mut node.dom_classes {
        *c = format!("{c}_renamed");
    }
    node.children.iter_mut().for_each(rename_classes);
}

#[test]
fn roster_names_everyone_and_who_is_self() {
    let s = meet().scrape(&load("roster-open")).unwrap();
    assert_eq!(s.self_name.as_deref(), Some("Bruno Lima"));
    let roster: Vec<_> = s.roster.iter().map(|r| (r.name.as_str(), r.is_self, r.muted)).collect();
    assert_eq!(roster, [("Bruno Lima", true, Some(true)), ("Ana Souza", false, Some(true))]);
    assert_eq!(s.health.status, recordem_scrape::Status::Healthy);
}

#[test]
fn renamed_classes_fall_back_to_structure_and_say_so() {
    let mut tree = load("roster-open");
    rename_classes(&mut tree);
    let s = meet().scrape(&tree).unwrap();
    let names: Vec<_> = s.participants.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Ana Souza", "Bruno Lima"], "tiles and names still found by structure");
    assert!(s.participants.iter().all(|p| p.frame.is_some()));
    assert_eq!(s.health.status, recordem_scrape::Status::Degraded);
    assert!(s.health.issues.contains(&recordem_scrape::Issue::FallbackTiles { rule: 1 }));
}

#[test]
fn lost_speaking_signal_is_reported_and_restarts() {
    use recordem_scrape::{Event, Issue, Monitor, MonitorOptions};
    let mut tree = load("ana-speaking");
    rename_classes(&mut tree);
    let s = meet().scrape(&tree).unwrap();
    assert!(s.participants.iter().all(|p| !p.speaking), "the speaking classes are gone");
    let mut m = Monitor::new(MonitorOptions::default());
    let mut events = Vec::new();
    // 45 s of the other side talking, scraped twice a second.
    for t in (0..45_000).step_by(500) {
        events.extend(m.observe(t, &s, Some(true)));
    }
    assert!(events.iter().any(|e| matches!(e, Event::Report { issue: Issue::SpeakingSignalLost { .. }, .. })), "{events:?}");
    assert!(events.contains(&Event::Restart));
    let reports = events.iter().filter(|e| matches!(e, Event::Report { issue: Issue::SpeakingSignalLost { .. }, .. })).count();
    assert_eq!(reports, 1, "reported once per call");
}

#[test]
fn healthy_calls_report_nothing() {
    use recordem_scrape::{Monitor, MonitorOptions};
    let (talk, quiet) = (meet().scrape(&load("ana-speaking")).unwrap(), meet().scrape(&load("silent")).unwrap());
    let mut m = Monitor::new(MonitorOptions::default());
    for t in (0..60_000u64).step_by(500) {
        let (s, audio) = if (t / 5_000) % 2 == 0 { (&talk, true) } else { (&quiet, false) };
        assert!(m.observe(t, s, Some(audio)).is_empty());
    }
}

// Taken during a live call while changing layouts and presenting (same anonymisation).

#[test]
fn layout_dialog_is_obstruction_not_breakage() {
    use recordem_scrape::{Issue, Monitor, MonitorOptions, Status};
    let s = meet().scrape(&load("dialog-open")).unwrap();
    assert!(s.participants.is_empty(), "the modal dialog hides the page");
    assert_eq!(s.health.status, Status::Degraded);
    assert_eq!(s.health.issues, [Issue::Obstructed]);
    let mut m = Monitor::new(MonitorOptions::default());
    for t in (0..20_000).step_by(500) {
        assert!(m.observe(t, &s, None).is_empty(), "nothing to restart or report");
    }
}

#[test]
fn presenting_merges_the_floating_window_and_skips_the_presentation_tile() {
    let s = meet().scrape(&load("presenting")).unwrap();
    let got: Vec<_> = s.participants.iter().map(|p| (p.name.as_str(), p.speaking, p.frame.is_some())).collect();
    // Bruno's own tile in the floating window carries a notice instead of his name; the tab still lists him.
    assert_eq!(got, [("Ana Souza", true, true), ("Bruno Lima", false, false)]);
}

#[test]
fn someone_elses_presentation_is_not_a_person() {
    let s = meet().scrape(&load("other-presenting")).unwrap();
    let names: Vec<_> = s.participants.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Ana Souza", "Bruno Lima"]);
}

// Switching tabs during a call: Chrome opens Meet's picture-in-picture window; closing it leaves only the tab.

#[test]
fn picture_in_picture_keeps_names_and_speakers() {
    let s = meet().scrape(&load("picture-in-picture")).unwrap();
    assert!(s.visible);
    assert_eq!(s.title.as_deref(), Some("abc-defg-hij"));
    assert_eq!(s.window_title.as_deref(), Some("Meet: abc-defg-hij - Bruno"), "read from the floating window");
    let names: Vec<_> = s.participants.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Ana Souza", "Bruno Lima"]);
}

#[test]
fn background_tab_is_a_call_not_on_screen() {
    use recordem_scrape::{Issue, Monitor, MonitorOptions};
    let s = meet().scrape(&load("background-tab")).expect("the call is still on");
    assert!(!s.visible);
    assert!(!s.window_shows_call, "the window shows another tab: not to be recorded");
    assert_eq!(s.title.as_deref(), Some("abc-defg-hij"));
    assert_eq!(s.health.issues, [Issue::NotVisible]);
    let mut m = Monitor::new(MonitorOptions::default());
    for t in (0..60_000).step_by(500) {
        assert!(m.observe(t, &s, Some(true)).is_empty(), "no evidence, nothing to report");
    }
}

#[test]
fn call_window_on_another_space() {
    // What the snapshot adds for a window Accessibility can't see: its title from the window server.
    let away = AxNode {
        role: Some("AXWindow".into()),
        subrole: Some(recordem_ax::ELSEWHERE.into()),
        title: Some("Meet: abc-defg-hij - Camera and microphone recording - Google Chrome - Bruno".into()),
        ..Default::default()
    };
    let other = AxNode { role: Some("AXWindow".into()), title: Some("Other window".into()), ..Default::default() };
    let app = AxNode { role: Some("AXApplication".into()), children: vec![other, away], ..Default::default() };
    let s = meet().scrape(&app).expect("the call is still on");
    assert!(!s.visible);
    assert!(s.window_shows_call, "its window still shows the call: keep recording it");
    assert_eq!(s.title.as_deref(), Some("abc-defg-hij"));
    assert_eq!(s.health.issues, [recordem_scrape::Issue::NotVisible]);
}
