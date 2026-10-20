// The call's link, read from the browser's tabs (AppleScript; macOS asks once to let the recorder "control" the
// browser). The window title gives the Meet code at best; the tab gives the whole link — and Zoom or Teams on the web.
// Only the first tab whose URL is a meeting is used; nothing else from the browser is kept.

/// (url, tab title) of the first meeting tab in the browser `bundle_id`, if any.
pub fn find(bundle_id: &str) -> Option<(String, String)> {
    let (url, title) = if bundle_id == "com.apple.Safari" { ("URL", "name") } else { ("URL", "title") };
    let script = format!(
        r#"if application id "{bundle_id}" is running then
  tell application id "{bundle_id}"
    set out to ""
    repeat with w in windows
      repeat with t in tabs of w
        set out to out & ({url} of t) & tab & ({title} of t) & linefeed
      end repeat
    end repeat
    return out
  end tell
end if"#
    );
    let o = std::process::Command::new("/usr/bin/osascript").args(["-e", &script]).output().ok()?;
    if !o.status.success() {
        eprintln!("call url: {}", String::from_utf8_lossy(&o.stderr).trim());
        return None;
    }
    String::from_utf8_lossy(&o.stdout).lines().find_map(|l| {
        let (u, t) = l.split_once('\t').unwrap_or((l, ""));
        is_meeting(bundle_id, u).then(|| (u.to_string(), t.to_string()))
    })
}

/// A call's link, by the reader profiles of this browser: the platform it is on ("google_meet", "zoom"…).
pub fn platform(bundle_id: &str, u: &str) -> Option<String> {
    let all = crate::profiles::current();
    all.for_app(bundle_id, None).into_iter().find(|p| p.links.iter().any(|l| l.is_match(u))).map(|p| p.platform.clone().unwrap_or_default())
}

fn is_meeting(bundle_id: &str, u: &str) -> bool {
    platform(bundle_id, u).is_some()
}
