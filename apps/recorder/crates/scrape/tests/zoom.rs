// Zoom's tiles, as read from its Accessibility tree: elements whose role description is "Video render", with a
// description "Name, audio status[, active speaker]". Synthetic until real dumps land in fixtures/zoom/.
use recordem_ax::{AxNode, Rect};

fn tile(desc: &str, x: f64) -> AxNode {
    AxNode {
        role: Some("AXGroup".into()),
        role_description: Some("Video render".into()),
        description: Some(desc.into()),
        frame: Some(Rect { x, y: 100.0, w: 400.0, h: 300.0 }),
        ..Default::default()
    }
}

#[test]
fn zoom_tiles() {
    let window = AxNode {
        role: Some("AXWindow".into()),
        title: Some("Zoom Meeting".into()),
        frame: Some(Rect { x: 0.0, y: 100.0, w: 800.0, h: 600.0 }),
        children: vec![AxNode {
            role: Some("AXGroup".into()),
            children: vec![
                tile("Ana Souza, unmuted, active speaker", 0.0),
                tile("Silva, Bruno, muted", 400.0),
                tile("Ana Souza, unmuted", 0.0),
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    let app = AxNode { role: Some("AXApplication".into()), children: vec![window], ..Default::default() };
    let zoom = recordem_scrape::builtin().into_iter().find(|p| p.id == "zoom").unwrap();
    let s = zoom.scrape(&app).expect("a call");
    let got: Vec<_> = s.participants.iter().map(|p| (p.name.as_str(), p.speaking, p.muted)).collect();
    assert_eq!(got, [("Ana Souza", true, Some(false)), ("Silva, Bruno", false, Some(true))]);
    assert_eq!(s.participants[1].frame.unwrap().x, 0.5);
}

fn app(children: Vec<AxNode>) -> AxNode {
    let window = AxNode {
        role: Some("AXWindow".into()),
        title: Some("Zoom Meeting".into()),
        frame: Some(Rect { x: 0.0, y: 100.0, w: 800.0, h: 600.0 }),
        children,
        ..Default::default()
    };
    AxNode { role: Some("AXApplication".into()), children: vec![window], ..Default::default() }
}

fn zoom() -> recordem_scrape::Profile {
    recordem_scrape::builtin().into_iter().find(|p| p.id == "zoom").unwrap()
}

#[test]
fn zoom_in_french() {
    let mut t = tile("Ana Souza, unmuted, active speaker", 0.0);
    t.role_description = Some("Rendu vidéo".into());
    let s = zoom().scrape(&app(vec![t])).expect("a call");
    assert_eq!(s.participants[0].name, "Ana Souza");
    assert!(s.participants[0].speaking);
}

#[test]
fn zoom_immersive_view_has_no_video_render() {
    let unknown = |d: &str| AxNode { role: Some("AXUnknown".into()), description: Some(d.into()), ..Default::default() };
    let s = zoom().scrape(&app(vec![unknown("Ana Souza, unmuted, active speaker"), unknown("Tab 1")])).expect("a call");
    let got: Vec<_> = s.participants.iter().map(|p| (p.name.as_str(), p.speaking)).collect();
    assert_eq!(got, [("Ana Souza", true)]);
    assert!(s.health.issues.contains(&recordem_scrape::Issue::FallbackTiles { rule: 1 }));
}

#[test]
fn zoom_talking_banner_with_hidden_thumbnails() {
    let banner = AxNode { role: Some("AXStaticText".into()), value: Some("Talking: Ana Souza, Bruno Lima".into()), ..Default::default() };
    let s = zoom().scrape(&app(vec![banner])).expect("a call");
    let got: Vec<_> = s.participants.iter().map(|p| (p.name.as_str(), p.speaking)).collect();
    assert_eq!(got, [("Ana Souza", true), ("Bruno Lima", true)]);
}
