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
        is_meeting(u).then(|| (u.to_string(), t.to_string()))
    })
}

fn is_meeting(u: &str) -> bool {
    let meet = u.contains("meet.google.com/") && u.split('/').nth(3).map(|p| p.len() == 12 && p.chars().filter(|c| *c == '-').count() == 2).unwrap_or(false);
    meet || u.contains("zoom.us/j/") || u.contains("zoom.us/wc/") || u.contains("teams.microsoft.com/") || u.contains("teams.live.com/")
}

/// A window or tab title worth showing as the meeting's name: not the app's own label nor just the call code.
/// Browser window titles carry more than the tab's: "Meet - Quarterly planning review - Camera and microphone
/// recording - Google Chrome - Rodrigo (Work)" → "Quarterly planning review".
pub fn meaningful_title(t: &str) -> Option<String> {
    let mut t = t.trim();
    // The browser's own suffix, and the profile after it.
    for b in [" - Google Chrome", " - Microsoft Edge", " - Brave", " - Arc", " — Safari", " - Safari"] {
        if let Some(i) = t.find(b) {
            t = &t[..i];
        }
    }
    // Chrome's recording indicators at the end of a tab's title.
    for ind in [" - Camera and microphone recording", " - Microphone recording", " - Camera recording", " - Audio playing", " - Screen sharing"] {
        if let Some(rest) = t.strip_suffix(ind) {
            t = rest;
        }
    }
    let t = t.strip_prefix("Meet - ").or_else(|| t.strip_prefix("Meet – ")).unwrap_or(t).trim();
    let t = t.strip_suffix(" - Google Meet").unwrap_or(t).trim();
    let code = t.len() == 12 && t.chars().filter(|c| *c == '-').count() == 2;
    let generic = ["", "Meet", "Google Meet", "Zoom", "Zoom Meeting", "Zoom Workplace", "Microsoft Teams"].contains(&t);
    if code || generic { None } else { Some(t.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::meaningful_title as t;

    #[test]
    fn titles() {
        assert_eq!(t("Meet - Quarterly planning review - Camera and microphone recording - Google Chrome - Rodrigo (Work)").as_deref(), Some("Quarterly planning review"));
        assert_eq!(t("Meet - abc-defg-hij - Camera and microphone recording - Google Chrome"), None);
        assert_eq!(t("Meet - abc-defg-hij"), None);
        assert_eq!(t("Zoom Meeting"), None);
        assert_eq!(t("Weekly sync").as_deref(), Some("Weekly sync"));
    }
}
