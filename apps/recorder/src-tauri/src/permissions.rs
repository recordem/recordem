// What macOS must allow before the recorder can do its job, checked and asked for in one place (the onboarding step
// after signing in, and Settings → Permissions):
//   microphone      your voice                                   required
//   system_audio    the call's audio (Core Audio tap)            required
//   accessibility   the meeting window's title (the call code)   recommended: links recordings to meetings by link
//   automation      the call's URL from the browser tab          recommended: the full link, not just the code
//   notifications   "Recording", "Transcript sent"               optional
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Granted,
    Denied,
    /// Never asked yet: asking shows the system prompt.
    NotAsked,
    /// Can't tell now (automation: the browser isn't running).
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub id: &'static str,
    pub status: Status,
    pub required: bool,
}

pub fn microphone() -> Status {
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
    let s = unsafe { AVCaptureDevice::authorizationStatusForMediaType(AVMediaTypeAudio.expect("AVMediaTypeAudio")) };
    match s {
        AVAuthorizationStatus::Authorized => Status::Granted,
        AVAuthorizationStatus::NotDetermined => Status::NotAsked,
        _ => Status::Denied,
    }
}

fn request_microphone() {
    use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    let done = block2::RcBlock::new(move |granted: objc2::runtime::Bool| {
        let _ = tx.send(granted.as_bool());
    });
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(AVMediaTypeAudio.expect("AVMediaTypeAudio"), &done) };
    let _ = rx.recv_timeout(std::time::Duration::from_secs(120));
}

pub fn system_audio() -> Status {
    match anlg_tcc::audio_capture_permission_status() {
        anlg_tcc::GRANTED => Status::Granted,
        anlg_tcc::NEVER_ASKED => Status::NotAsked,
        anlg_tcc::DENIED => Status::Denied,
        _ => Status::Unknown,
    }
}

/// There is no prompt API for the audio tap: opening one is what makes macOS ask.
fn request_system_audio() {
    use futures_util::StreamExt;
    if let Ok(mut s) = anlg_audio_actual::AudioInput::from_speaker_capture(16_000, 1600) {
        let _ = tauri::async_runtime::block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(2), s.next()).await
        });
    }
}

pub fn accessibility() -> Status {
    if macos_accessibility_client::accessibility::application_is_trusted() { Status::Granted } else { Status::NotAsked }
}

/// The browsers whose tabs the recorder reads for the call's link (AppleScript): those running now.
pub const BROWSERS: &[(&str, &str)] = &[
    ("com.google.Chrome", "Google Chrome"),
    ("company.thebrowser.Browser", "Arc"),
    ("com.microsoft.edgemac", "Microsoft Edge"),
    ("com.brave.Browser", "Brave Browser"),
    ("com.apple.Safari", "Safari"),
];

fn running(bundle_id: &str) -> bool {
    std::process::Command::new("/usr/bin/pgrep").args(["-f", &format!("{}.app/Contents/MacOS", app_name(bundle_id).unwrap_or(bundle_id))]).output().map(|o| o.status.success()).unwrap_or(false)
}

fn app_name(bundle_id: &str) -> Option<&'static str> {
    BROWSERS.iter().find(|(id, _)| *id == bundle_id).map(|(_, n)| *n)
}

/// Automation of the first running browser: asking it something harmless shows the prompt the first time, and an
/// error -1743 means it was refused.
fn automation_probe() -> Status {
    let Some((id, _)) = BROWSERS.iter().find(|(id, _)| running(id)) else { return Status::Unknown };
    match std::process::Command::new("/usr/bin/osascript").args(["-e", &format!("tell application id \"{id}\" to count windows")]).output() {
        Ok(o) if o.status.success() => Status::Granted,
        Ok(o) if String::from_utf8_lossy(&o.stderr).contains("-1743") => Status::Denied,
        _ => Status::Unknown,
    }
}

pub fn notifications(app: &tauri::AppHandle) -> Status {
    use tauri_plugin_notification::{NotificationExt, PermissionState};
    match app.notification().permission_state() {
        Ok(PermissionState::Granted) => Status::Granted,
        Ok(PermissionState::Denied) => Status::Denied,
        _ => Status::NotAsked,
    }
}

/// The required ones: without them a recording is silence.
pub fn ready() -> bool {
    microphone() == Status::Granted && system_audio() == Status::Granted
}

/// Automation's status is only known by asking the browser, which itself prompts the first time: remembered here
/// between checks so the list doesn't prompt on every refresh.
static AUTOMATION: std::sync::Mutex<Status> = std::sync::Mutex::new(Status::Unknown);

pub fn all(app: &tauri::AppHandle) -> Vec<Item> {
    vec![
        Item { id: "microphone", status: microphone(), required: true },
        Item { id: "system_audio", status: system_audio(), required: true },
        Item { id: "accessibility", status: accessibility(), required: false },
        Item { id: "automation", status: *AUTOMATION.lock().unwrap(), required: false },
        Item { id: "notifications", status: notifications(app), required: false },
    ]
}

/// Shows the system prompt; when macOS won't ask again (refused before), opens the right pane of System Settings.
pub fn request(app: &tauri::AppHandle, id: &str) {
    let before = all(app).into_iter().find(|i| i.id == id).map(|i| i.status);
    if before == Some(Status::Denied) {
        open_settings(id);
        return;
    }
    match id {
        "microphone" => request_microphone(),
        "system_audio" => request_system_audio(),
        "accessibility" => {
            macos_accessibility_client::accessibility::application_is_trusted_with_prompt();
        }
        "automation" => {
            let s = automation_probe();
            *AUTOMATION.lock().unwrap() = s;
            if s == Status::Denied {
                open_settings(id);
            }
        }
        "notifications" => {
            use tauri_plugin_notification::NotificationExt;
            let _ = app.notification().request_permission();
        }
        _ => {}
    }
}

pub fn open_settings(id: &str) {
    let anchor = match id {
        "microphone" => "Privacy_Microphone",
        "system_audio" => "Privacy_AudioCapture",
        "accessibility" => "Privacy_Accessibility",
        "automation" => "Privacy_Automation",
        _ => "",
    };
    let url = if id == "notifications" {
        "x-apple.systempreferences:com.apple.Notifications-Settings.extension".to_string()
    } else {
        format!("x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?{anchor}")
    };
    let _ = std::process::Command::new("/usr/bin/open").arg(url).status();
}
