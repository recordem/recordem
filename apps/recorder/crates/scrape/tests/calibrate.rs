// The calibrator on a recorded call: `RECORDEM_AX_DUMPS=<dir of probe dumps> cargo test --release -- --ignored
// finds_meets_mark --nocapture`. Who could be heard is taken from the profile's own reading (the dumps carry no
// audio); among every class in the tiles, the calibrator has to come back with the one that reading uses.
use recordem_ax::AxNode;
use recordem_scrape::calibrate::{Calibrator, Moment};

#[test]
#[ignore]
fn finds_meets_mark() {
    let dir = std::env::var("RECORDEM_AX_DUMPS").expect("RECORDEM_AX_DUMPS");
    let meet = recordem_scrape::builtin().into_iter().find(|p| p.id == "meet-chrome").unwrap();
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    files.sort();
    // A scripted minute: fewer quiet moments than a real call has.
    let mut c = Calibrator::with_enough(20);
    let (mut heard, mut quiet, mut tiles_seen) = (0, 0, 0);
    for f in files {
        let tree: AxNode = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        let Some(s) = meet.scrape(&tree) else { continue };
        let tiles: Vec<(bool, Vec<String>)> = s.participants.iter().filter(|p| p.frame.is_some()).map(|p| (p.is_self, p.classes.clone())).collect();
        tiles_seen += tiles.len();
        if s.participants.iter().any(|p| p.speaking && !p.is_self) { heard += 1 } else { quiet += 1 }
        c.observe(&Moment {
            tiles: tiles.iter().map(|(me, cl)| (*me, cl.as_slice())).collect(),
            others_audible: s.participants.iter().any(|p| p.speaking && !p.is_self),
            me_audible: s.participants.iter().any(|p| p.speaking && p.is_self),
        });
    }
    eprintln!("heard {heard} quiet {quiet} tiles {tiles_seen}");
    let best = c.best(0.0);
    eprintln!("{:?}", &best[..best.len().min(6)]);
    assert_eq!(best.first().map(|b| b.class.as_str()), Some("kssMZb"));
}
