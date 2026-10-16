// The call app's icon (Chrome, Zoom, Teams…) for the call prompt, as a PNG data URL: the .icns its bundle names
// (CFBundleIconFile), through sips at 64 px, kept in the app's folder after the first time. None for an app whose icon
// lives only in an asset catalog, or that isn't installed where macOS can find it: the prompt shows its dot instead.
use base64::Engine;
use std::process::Command;

pub fn of(bundle_id: &str) -> Option<String> {
    // It goes into an AppleScript string and a file name.
    if bundle_id.is_empty() || !bundle_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return None;
    }
    let dir = crate::config::root().join("icons");
    let png = dir.join(format!("{bundle_id}.png"));
    if !png.exists() {
        let app = app_path(bundle_id)?;
        let app = app.to_string_lossy().to_string();
        let out = Command::new("defaults").args(["read", &format!("{app}/Contents/Info"), "CFBundleIconFile"]).output().ok()?;
        let name = String::from_utf8(out.stdout).ok()?.trim().to_string();
        if !out.status.success() || name.is_empty() {
            return None;
        }
        let icns = format!("{app}/Contents/Resources/{}", if name.ends_with(".icns") { name } else { format!("{name}.icns") });
        std::fs::create_dir_all(&dir).ok()?;
        let ok = Command::new("sips").args(["-s", "format", "png", "-Z", "64", &icns, "--out"]).arg(&png).output().ok()?.status.success();
        if !ok {
            let _ = std::fs::remove_file(&png);
            return None;
        }
    }
    let bytes = std::fs::read(&png).ok()?;
    Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// Where the app `bundle_id` is installed (Launch Services, through AppleScript; nothing is launched).
pub fn app_path(bundle_id: &str) -> Option<std::path::PathBuf> {
    if bundle_id.is_empty() || !bundle_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return None;
    }
    let script = format!("POSIX path of (path to application id \"{bundle_id}\")");
    let out = Command::new("osascript").args(["-e", &script]).output().ok()?;
    let app = String::from_utf8(out.stdout).ok()?.trim().trim_end_matches('/').to_string();
    (out.status.success() && !app.is_empty()).then(|| app.into())
}
