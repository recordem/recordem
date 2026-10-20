// What to call a meeting from its window: a helper for hosts, which name recordings from `Window::title`.

/// A window or tab title worth showing as the meeting's name: not the app's own label nor just the call code.
/// Browser window titles carry more than the tab's: "Meet - Quarterly planning review - Camera and microphone
/// recording - Google Chrome - Rodrigo (Work)" → "Quarterly planning review".
pub fn meeting_title(t: &str) -> Option<String> {
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
    use super::meeting_title as t;

    #[test]
    fn titles() {
        assert_eq!(t("Meet - Quarterly planning review - Camera and microphone recording - Google Chrome - Rodrigo (Work)").as_deref(), Some("Quarterly planning review"));
        assert_eq!(t("Meet - abc-defg-hij - Camera and microphone recording - Google Chrome"), None);
        assert_eq!(t("Meet - abc-defg-hij"), None);
        assert_eq!(t("Zoom Meeting"), None);
        assert_eq!(t("Weekly sync").as_deref(), Some("Weekly sync"));
    }
}
