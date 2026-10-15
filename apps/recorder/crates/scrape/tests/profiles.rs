use recordem_scrape::{Profiles, compare_versions};
use std::cmp::Ordering::*;

#[test]
fn versions_compare_by_number() {
    assert_eq!(compare_versions("6.5.9.61929", "6.5.10"), Less);
    assert_eq!(compare_versions("6.5", "6.5.0"), Equal);
    assert_eq!(compare_versions("7.0", "6.99"), Greater);
}

#[test]
fn a_server_profile_replaces_an_older_one_and_adds_versions() {
    let mut p = Profiles::builtin();
    let zoom_rev = p.for_app("us.zoom.xos", Some("6.5.9"))[0].revision;
    let doc = format!(
        r#"{{"profiles": [
            {{"id": "zoom", "revision": {next}, "apps": ["us.zoom.xos"], "tiles": [["eq", "role", "AXGroup"]], "name": [{{"attr": "title"}}]}},
            {{"id": "zoom", "revision": 1, "apps": ["us.zoom.xos"], "app_versions": {{"min": "7.0"}}, "tiles": [["eq", "role", "AXCell"]], "name": [{{"attr": "title"}}]}},
            {{"id": "meet-chrome", "revision": 0, "apps": ["com.google.Chrome"], "tiles": [], "name": []}}
        ]}}"#,
        next = zoom_rev + 1
    );
    assert_eq!(p.merge(&doc).unwrap(), 2, "newer zoom replaced, zoom 7 added, older meet ignored");
    assert_eq!(p.for_app("us.zoom.xos", Some("6.5.9")).len(), 1);
    assert_eq!(p.for_app("us.zoom.xos", Some("6.5.9"))[0].revision, zoom_rev + 1);
    let seven = p.for_app("us.zoom.xos", Some("7.1"));
    assert_eq!(seven.len(), 2);
    assert_eq!(seven[0].app_versions.min.as_deref(), Some("7.0"), "the version-specific profile comes first");
    assert!(p.for_app("com.google.Chrome", None)[0].revision >= 2, "built-in meet kept");
}
