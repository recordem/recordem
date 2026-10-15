// Mute: while it is on, the mic channel is recorded as silence, so nothing said then is transcribed (live or after
// the call) and the two channels keep the same length. The other side of the call is still recorded.
//
// It follows the call's own mute button where that can be read through the Accessibility tree (that permission):
// Google Meet in the browser (the mic button is "Ativar microfone" while muted, "Desativar microfone" while open) and
// the Zoom app (its menu: "Unmute Audio" / "Mute Audio"). Only a change of the call's button moves it, so the
// recorder's own toggle holds until the next change in the call. When the button can't be read (another tab in front,
// a language not listed here, no permission, Meet renamed it) nothing changes and the recording goes on as it was.
//
// Plain Core Foundation / AX calls with their status codes: cidre's AX wrappers abort on some valid error codes
// (Anarlog's note in meeting_ax.rs), and nothing here may take the app, and the recording, down with it.
use std::ffi::{c_char, c_void};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

type Ref = *const c_void;
const UTF8: u32 = 0x0800_0100;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> Ref;
    fn AXUIElementCopyAttributeValue(element: Ref, attribute: Ref, value: *mut Ref) -> i32;
    fn AXUIElementSetAttributeValue(element: Ref, attribute: Ref, value: Ref) -> i32;
    fn AXUIElementSetMessagingTimeout(element: Ref, seconds: f32) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFBooleanTrue: Ref;
    fn CFRetain(cf: Ref) -> Ref;
    fn CFRelease(cf: Ref);
    fn CFGetTypeID(cf: Ref) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFURLGetTypeID() -> usize;
    fn CFArrayGetCount(array: Ref) -> isize;
    fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
    fn CFURLGetString(url: Ref) -> Ref;
    fn CFStringCreateWithBytes(alloc: Ref, bytes: *const u8, len: isize, encoding: u32, external: u8) -> Ref;
    fn CFStringGetLength(s: Ref) -> isize;
    fn CFStringGetMaximumSizeForEncoding(len: isize, encoding: u32) -> isize;
    fn CFStringGetCString(s: Ref, buf: *mut c_char, size: isize, encoding: u32) -> u8;
}

/// An owned Core Foundation object (an AX element, a string, an array).
struct Cf(Ref);

impl Drop for Cf {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}

impl Cf {
    fn owned(r: Ref) -> Option<Cf> {
        (!r.is_null()).then_some(Cf(r))
    }

    fn str(s: &str) -> Option<Cf> {
        Cf::owned(unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, 0) })
    }

    fn attr(&self, name: &str) -> Option<Cf> {
        let name = Cf::str(name)?;
        let mut out: Ref = std::ptr::null();
        let status = unsafe { AXUIElementCopyAttributeValue(self.0, name.0, &mut out) };
        if status != 0 {
            if !out.is_null() {
                unsafe { CFRelease(out) };
            }
            return None;
        }
        Cf::owned(out)
    }

    fn text(&self, name: &str) -> Option<String> {
        self.attr(name).and_then(|v| string(v.0))
    }

    fn elements(&self, name: &str) -> Vec<Cf> {
        let Some(a) = self.attr(name) else { return vec![] };
        if unsafe { CFGetTypeID(a.0) } != unsafe { CFArrayGetTypeID() } {
            return vec![];
        }
        (0..unsafe { CFArrayGetCount(a.0) })
            .filter_map(|i| {
                let r = unsafe { CFArrayGetValueAtIndex(a.0, i) };
                (!r.is_null()).then(|| Cf(unsafe { CFRetain(r) }))
            })
            .collect()
    }
}

/// A CFString (or a CFURL's string) as text.
fn string(r: Ref) -> Option<String> {
    unsafe {
        let t = CFGetTypeID(r);
        if t == CFURLGetTypeID() {
            let s = CFURLGetString(r);
            return if s.is_null() { None } else { string(s) };
        }
        if t != CFStringGetTypeID() {
            return None;
        }
        let size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(r), UTF8) + 1;
        let mut buf = vec![0u8; size.max(1) as usize];
        if CFStringGetCString(r, buf.as_mut_ptr() as *mut c_char, size, UTF8) == 0 {
            return None;
        }
        buf.truncate(buf.iter().position(|&b| b == 0).unwrap_or(buf.len()));
        String::from_utf8(buf).ok()
    }
}

/// Meet's mic button, by its label's start (the shortcut may follow: "Ativar microfone (⌘ + d)").
const MEET_MUTED: &[&str] = &["turn on microphone", "ativar microfone", "activar micrófono"];
const MEET_OPEN: &[&str] = &["turn off microphone", "desativar microfone", "desactivar micrófono"];
/// Zoom's menu item, whole.
const ZOOM_MUTED: &[&str] = &["unmute audio", "ativar áudio", "ativar som"];
const ZOOM_OPEN: &[&str] = &["mute audio", "desativar áudio", "desativar som"];

/// muted (true), open (false), or not this button.
fn meet_label(label: &str) -> Option<bool> {
    let l = label.trim().to_lowercase();
    if MEET_MUTED.iter().any(|p| l.starts_with(p)) {
        Some(true)
    } else if MEET_OPEN.iter().any(|p| l.starts_with(p)) {
        Some(false)
    } else {
        None
    }
}

fn zoom_label(label: &str) -> Option<bool> {
    let l = label.trim().to_lowercase();
    if ZOOM_MUTED.contains(&l.as_str()) {
        Some(true)
    } else if ZOOM_OPEN.contains(&l.as_str()) {
        Some(false)
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Source {
    Meet,
    Zoom,
}

const POLL: Duration = Duration::from_secs(1);
/// A full search of the browser's tree is the costly part: at most this often, while the button isn't at hand.
const SEARCH_EVERY: Duration = Duration::from_secs(5);
const MAX_NODES: usize = 5000;
const MAX_DEPTH: usize = 80;

struct Watch {
    source: Source,
    bundle: String,
    app: Option<(i32, Cf)>,
    button: Option<Cf>,
    searched: Option<Instant>,
    manual_ax: bool,
}

impl Watch {
    /// The call app's AX element, found again if the app restarted.
    fn app(&mut self) -> Option<&Cf> {
        let pid = pid(&self.bundle)?;
        if self.app.as_ref().map(|(p, _)| *p) != Some(pid) {
            let el = Cf::owned(unsafe { AXUIElementCreateApplication(pid) })?;
            unsafe { AXUIElementSetMessagingTimeout(el.0, 0.5) };
            self.app = Some((pid, el));
            self.button = None;
            self.manual_ax = false;
        }
        self.app.as_ref().map(|(_, el)| el)
    }

    fn poll(&mut self) -> Option<bool> {
        match self.source {
            Source::Zoom => zoom(self.app()?),
            Source::Meet => self.meet(),
        }
    }

    fn meet(&mut self) -> Option<bool> {
        // The button found before: one call to read its label.
        if let Some(b) = &self.button {
            let label = b.text("AXDescription").or_else(|| b.text("AXTitle"));
            if let Some(m) = label.as_deref().and_then(meet_label) {
                return Some(m);
            }
            self.button = None;
        }
        if self.searched.is_some_and(|t| t.elapsed() < SEARCH_EVERY) {
            return None;
        }
        self.searched = Some(Instant::now());
        let app = self.app()?;
        let mut visited = 0;
        let mut meet_page = false;
        let mut found = None;
        for w in app.elements("AXWindows") {
            if let Some(b) = find_meet_button(&w, 0, &mut visited, &mut meet_page, false) {
                found = Some(b);
                break;
            }
        }
        match found {
            Some((b, m)) => {
                self.button = Some(b);
                Some(m)
            }
            None => {
                // A Meet page with no button in its tree: Chromium builds a page's tree only for assistive apps that
                // ask for it. Asked once per browser run; the next search sees the page.
                if meet_page && !self.manual_ax {
                    self.manual_ax = true;
                    if let (Some(app), Some(name)) = (self.app.as_ref().map(|(_, a)| a), Cf::str("AXManualAccessibility")) {
                        unsafe { AXUIElementSetAttributeValue(app.0, name.0, kCFBooleanTrue) };
                    }
                    self.searched = None;
                }
                None
            }
        }
    }
}

/// Depth-first through a browser window; only Meet pages (by URL) are searched inside.
/// `meet_page` notes that one was seen; `inside`, that `el` is in one.
fn find_meet_button(el: &Cf, depth: usize, visited: &mut usize, meet_page: &mut bool, inside: bool) -> Option<(Cf, bool)> {
    if depth > MAX_DEPTH || *visited >= MAX_NODES {
        return None;
    }
    *visited += 1;
    let role = el.text("AXRole").unwrap_or_default();
    let mut inside = inside;
    if role == "AXWebArea" && !inside {
        if !el.text("AXURL").unwrap_or_default().contains("meet.google.com") {
            return None;
        }
        *meet_page = true;
        inside = true;
    }
    if role == "AXButton" || role == "AXCheckBox" {
        if inside {
            let label = el.text("AXDescription").or_else(|| el.text("AXTitle"));
            if let Some(m) = label.as_deref().and_then(meet_label) {
                return Some((Cf(unsafe { CFRetain(el.0) }), m));
            }
        }
        return None;
    }
    for c in el.elements("AXChildren") {
        if let Some(hit) = find_meet_button(&c, depth + 1, visited, meet_page, inside) {
            return Some(hit);
        }
    }
    None
}

/// Zoom's menu bar: some menu has "Mute Audio" or "Unmute Audio" while in a meeting (titles only; nothing is opened).
fn zoom(app: &Cf) -> Option<bool> {
    let bar = app.attr("AXMenuBar")?;
    for item in bar.elements("AXChildren") {
        for menu in item.elements("AXChildren") {
            for entry in menu.elements("AXChildren") {
                if let Some(m) = entry.text("AXTitle").as_deref().and_then(zoom_label) {
                    return Some(m);
                }
            }
        }
    }
    None
}

fn pid(bundle: &str) -> Option<i32> {
    let apps = cidre::ns::RunningApp::with_bundle_id(&cidre::ns::String::with_str(bundle));
    apps.get(0).ok().map(|a| a.pid())
}

/// Follows the call app's mute button until `stop`; `changed` gets each new state (the first one seen included).
pub fn watch(source: Source, bundle: String, stop: CancellationToken, changed: impl Fn(bool) + Send + 'static) {
    let spawned = std::thread::Builder::new().name("mute-watch".into()).spawn(move || {
        let mut w = Watch { source, bundle, app: None, button: None, searched: None, manual_ax: false };
        let mut last = None;
        while !stop.is_cancelled() {
            std::thread::sleep(POLL);
            if stop.is_cancelled() || !unsafe { AXIsProcessTrusted() } {
                continue;
            }
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.poll())) {
                Ok(Some(m)) if last != Some(m) => {
                    last = Some(m);
                    changed(m);
                }
                Ok(_) => {}
                Err(_) => {
                    eprintln!("mute watch stopped");
                    return;
                }
            }
        }
    });
    if let Err(e) = spawned {
        eprintln!("mute watch: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(meet_label("Ativar microfone (⌘ + d)"), Some(true));
        assert_eq!(meet_label("Desativar microfone (⌘ + d)"), Some(false));
        assert_eq!(meet_label("Turn on microphone"), Some(true));
        assert_eq!(meet_label("Turn off microphone (⌘ + d)"), Some(false));
        assert_eq!(meet_label("Configurações de áudio"), None);
        assert_eq!(zoom_label("Unmute Audio"), Some(true));
        assert_eq!(zoom_label("Mute Audio"), Some(false));
        assert_eq!(zoom_label("Mute All"), None);
    }
}
