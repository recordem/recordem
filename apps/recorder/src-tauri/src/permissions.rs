// What macOS must allow before the recorder can do its job, checked and asked for in one place. The onboarding step
// after signing in asks for the required ones; Settings → Permissions lists them all:
//   microphone      your voice                                   required
//   system_audio    the call's audio (Core Audio tap)            required
//   accessibility   the meeting window's title (the call code)   recommended: links recordings to meetings by link
//   automation      the call's URL from the browser tab          recommended: the full link, not just the code
// (Notifications aren't listed: on macOS the notification plugin can't tell whether they're allowed.)
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

/// Accessibility has no "denied": not trusted is all macOS says. Once its prompt was shown (this run), the next step is
/// System Settings — also when the switch there is already on for an older build of the app, which no longer counts.
pub fn accessibility() -> Status {
    if macos_accessibility_client::accessibility::application_is_trusted() {
        Status::Granted
    } else if ASKED_AX.load(std::sync::atomic::Ordering::Relaxed) {
        Status::Denied
    } else {
        Status::NotAsked
    }
}

static ASKED_AX: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The browsers whose tabs the recorder reads for the call's link (AppleScript): those running now.
pub const BROWSERS: &[(&str, &str)] = &[
    ("com.google.Chrome", "Google Chrome"),
    ("company.thebrowser.Browser", "Arc"),
    ("com.microsoft.edgemac", "Microsoft Edge"),
    ("com.brave.Browser", "Brave Browser"),
    ("com.apple.Safari", "Safari"),
];

#[repr(C)]
struct AEDesc {
    descriptor_type: u32,
    data_handle: *mut std::ffi::c_void,
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn AECreateDesc(type_code: u32, data: *const std::ffi::c_void, size: isize, result: *mut AEDesc) -> i16;
    fn AEDisposeDesc(desc: *mut AEDesc) -> i16;
    fn AEDeterminePermissionToAutomateTarget(target: *const AEDesc, event_class: u32, event_id: u32, ask: u8) -> i32;
}

const TYPE_BUNDLE_ID: u32 = u32::from_be_bytes(*b"bund");
const WILDCARD: u32 = u32::from_be_bytes(*b"****");

/// May this app send Apple Events to `bundle_id`? With `ask`, macOS shows its prompt when it never asked before
/// (blocks until answered). Not running → Unknown: macOS can only tell for a running app.
fn automate(bundle_id: &str, ask: bool) -> Status {
    let mut desc = AEDesc { descriptor_type: 0, data_handle: std::ptr::null_mut() };
    let err = unsafe { AECreateDesc(TYPE_BUNDLE_ID, bundle_id.as_ptr().cast(), bundle_id.len() as isize, &mut desc) };
    if err != 0 {
        return Status::Unknown;
    }
    let r = unsafe { AEDeterminePermissionToAutomateTarget(&desc, WILDCARD, WILDCARD, ask as u8) };
    unsafe { AEDisposeDesc(&mut desc) };
    match r {
        0 => Status::Granted,
        -1743 => Status::Denied,  // errAEEventNotPermitted
        -1744 => Status::NotAsked, // errAEEventWouldRequireUserConsent
        _ => Status::Unknown,      // procNotFound (-600): not running
    }
}

/// Automation, for the browsers that are running: granted if any of them allows it (the one with the call is what
/// matters, and people use one), else the first one's answer.
pub fn automation() -> Status {
    let all: Vec<Status> = BROWSERS.iter().map(|(id, _)| automate(id, false)).filter(|s| *s != Status::Unknown).collect();
    if all.contains(&Status::Granted) { Status::Granted } else { all.first().copied().unwrap_or(Status::Unknown) }
}

/// The required ones: without them a recording is silence.
pub fn ready() -> bool {
    microphone() == Status::Granted && system_audio() == Status::Granted
}

/// All of them, required first.
pub fn all() -> Vec<Item> {
    vec![
        Item { id: "microphone", status: microphone(), required: true },
        Item { id: "system_audio", status: system_audio(), required: true },
        Item { id: "accessibility", status: accessibility(), required: false },
        Item { id: "automation", status: automation(), required: false },
    ]
}

/// Shows the system prompt; when macOS won't ask again (refused before), opens the right pane of System Settings.
pub fn request(id: &str) {
    let before = all().into_iter().find(|i| i.id == id).map(|i| i.status);
    if before == Some(Status::Denied) {
        open_settings(id);
        return;
    }
    match id {
        "microphone" => request_microphone(),
        "system_audio" => request_system_audio(),
        "accessibility" => {
            ASKED_AX.store(true, std::sync::atomic::Ordering::Relaxed);
            macos_accessibility_client::accessibility::application_is_trusted_with_prompt();
        }
        "automation" => {
            // The prompt for the first running browser that hasn't been answered yet.
            for (b, _) in BROWSERS {
                if automate(b, false) == Status::NotAsked {
                    automate(b, true);
                    break;
                }
            }
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
    let url = format!("x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?{anchor}");
    let _ = std::process::Command::new("/usr/bin/open").arg(url).status();
}
