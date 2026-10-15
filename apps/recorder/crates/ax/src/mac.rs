// Reading the tree: plain Core Foundation / AX calls with their status codes, as in the recorder's mute.rs. An app
// that is busy, quitting or redesigned yields a smaller tree, never a crash.
use crate::{AxNode, Limits, Rect, Scope};
use std::ffi::{c_char, c_void};

type Ref = *const c_void;
const UTF8: u32 = 0x0800_0100;
const AX_VALUE_CGPOINT: u32 = 1;
const AX_VALUE_CGSIZE: u32 = 2;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> Ref;
    fn AXUIElementCopyAttributeValue(element: Ref, attribute: Ref, value: *mut Ref) -> i32;
    fn AXUIElementCopyMultipleAttributeValues(element: Ref, attributes: Ref, options: u32, values: *mut Ref) -> i32;
    fn AXUIElementSetAttributeValue(element: Ref, attribute: Ref, value: Ref) -> i32;
    fn AXUIElementSetMessagingTimeout(element: Ref, seconds: f32) -> i32;
    fn AXValueGetTypeID() -> usize;
    fn AXValueGetType(value: Ref) -> u32;
    fn AXValueGetValue(value: Ref, kind: u32, out: *mut c_void) -> bool;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> Ref;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDictionaryGetValue(dict: Ref, key: Ref) -> Ref;
    fn CFNumberGetValue(number: Ref, kind: isize, out: *mut c_void) -> bool;
    fn CFBooleanGetValue(boolean: Ref) -> bool;
    static kCFBooleanTrue: Ref;
    static kCFTypeArrayCallBacks: c_void;
    fn CFRelease(cf: Ref);
    fn CFRetain(cf: Ref) -> Ref;
    fn CFGetTypeID(cf: Ref) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFArrayCreate(alloc: Ref, values: *const Ref, count: isize, callbacks: *const c_void) -> Ref;
    fn CFArrayGetCount(array: Ref) -> isize;
    fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
    fn CFStringCreateWithBytes(alloc: Ref, bytes: *const u8, len: isize, encoding: u32, external: u8) -> Ref;
    fn CFStringGetLength(s: Ref) -> isize;
    fn CFStringGetMaximumSizeForEncoding(len: isize, encoding: u32) -> isize;
    fn CFStringGetCString(s: Ref, buf: *mut c_char, size: isize, encoding: u32) -> u8;
}

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

    fn str(s: &str) -> Cf {
        Cf(unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, 0) })
    }

    fn attr(&self, name: &str) -> Option<Cf> {
        let name = Cf::str(name);
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
}

fn string(r: Ref) -> Option<String> {
    if r.is_null() || unsafe { CFGetTypeID(r) != CFStringGetTypeID() } {
        return None;
    }
    let size = unsafe { CFStringGetMaximumSizeForEncoding(CFStringGetLength(r), UTF8) } + 1;
    let mut buf = vec![0u8; size.max(1) as usize];
    if unsafe { CFStringGetCString(r, buf.as_mut_ptr() as *mut c_char, size, UTF8) } == 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    buf.truncate(end);
    String::from_utf8(buf).ok().filter(|s| !s.is_empty())
}

/// The items of a CF array, retained (the array may go first).
fn items(r: Ref) -> Vec<Cf> {
    if r.is_null() || unsafe { CFGetTypeID(r) != CFArrayGetTypeID() } {
        return Vec::new();
    }
    (0..unsafe { CFArrayGetCount(r) })
        .filter_map(|i| Cf::owned(unsafe { CFRetain(CFArrayGetValueAtIndex(r, i)) }))
        .collect()
}

fn ax_value<T: Default>(r: Ref, kind: u32) -> Option<T> {
    if r.is_null() || unsafe { CFGetTypeID(r) != AXValueGetTypeID() || AXValueGetType(r) != kind } {
        return None;
    }
    let mut out = T::default();
    unsafe { AXValueGetValue(r, kind, &mut out as *mut T as *mut c_void) }.then_some(out)
}

pub fn is_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// The process of a running app, by bundle id (the first one if several run).
pub fn pid_of(bundle_id: &str) -> Option<i32> {
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::NSString;
    let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle_id));
    apps.to_vec().iter().map(|a| a.processIdentifier()).find(|&pid| pid > 0)
}

/// The order AXUIElementCopyMultipleAttributeValues answers in.
const ATTRS: [&str; 12] = [
    "AXRole",
    "AXSubrole",
    "AXRoleDescription",
    "AXTitle",
    "AXDescription",
    "AXValue",
    "AXIdentifier",
    "AXDOMIdentifier",
    "AXDOMClassList",
    "AXPosition",
    "AXSize",
    "AXChildren",
];

struct Walk<'a> {
    names: Cf,
    limits: Limits,
    scope: &'a Scope<'a>,
    visited: usize,
}

impl Walk<'_> {
    fn node(&mut self, el: &Cf, depth: usize, inside: bool) -> AxNode {
        self.visited += 1;
        let mut out: Ref = std::ptr::null();
        if unsafe { AXUIElementCopyMultipleAttributeValues(el.0, self.names.0, 0, &mut out) } != 0 || out.is_null() {
            return AxNode::default();
        }
        let values = Cf(out);
        let at = |i: isize| unsafe { CFArrayGetValueAtIndex(values.0, i) };
        let text = |i: isize| string(at(i));
        let frame = match (ax_value::<[f64; 2]>(at(9), AX_VALUE_CGPOINT), ax_value::<[f64; 2]>(at(10), AX_VALUE_CGSIZE)) {
            (Some([x, y]), Some([w, h])) => Some(Rect { x, y, w, h }),
            _ => None,
        };
        let mut node = AxNode {
            role: text(0),
            subrole: text(1),
            role_description: text(2),
            title: text(3),
            description: text(4),
            value: text(5),
            identifier: text(6),
            dom_id: text(7),
            dom_classes: items(at(8)).iter().filter_map(|c| string(c.0)).collect(),
            frame,
            children: Vec::new(),
            truncated: false,
        };
        // Below the application, each window is read in full or only up to its page, by the scope.
        let inside = if depth == 1 && node.role.as_deref() == Some("AXWindow") { (self.scope.window)(&node) } else { inside };
        let skip = if inside { (self.scope.prune)(&node) } else { (self.scope.prune_elsewhere)(&node) };
        if skip {
            return node;
        }
        let children = items(at(11));
        if depth >= self.limits.max_depth {
            node.truncated = !children.is_empty();
            return node;
        }
        for child in children {
            if self.visited >= self.limits.max_nodes {
                node.truncated = true;
                break;
            }
            node.children.push(self.node(&child, depth + 1, inside));
        }
        node
    }
}

/// The app's tree: the application element, its menu bar and windows below it. Chromium and Electron apps only
/// build their web content's tree once asked to (AXManualAccessibility); asking twice is harmless.
/// The app's windows as the window server lists them, on every Space and minimised ones included — Accessibility
/// only sees the current Space. Titles need the Screen Recording permission (without it they are missing).
fn server_windows(pid: i32) -> Vec<(Option<String>, Option<Rect>, bool)> {
    const ALL_EXCEPT_DESKTOP: u32 = 1 << 4;
    let Some(list) = Cf::owned(unsafe { CGWindowListCopyWindowInfo(ALL_EXCEPT_DESKTOP, 0) }) else { return Vec::new() };
    let get = |d: Ref, k: &str| unsafe { CFDictionaryGetValue(d, Cf::str(k).0) };
    let int = |r: Ref| {
        let mut v: i64 = 0;
        (!r.is_null() && unsafe { CFNumberGetValue(r, 4, &mut v as *mut i64 as *mut c_void) }).then_some(v)
    };
    let mut out = Vec::new();
    for i in 0..unsafe { CFArrayGetCount(list.0) } {
        let d = unsafe { CFArrayGetValueAtIndex(list.0, i) };
        if int(get(d, "kCGWindowOwnerPID")) != Some(pid as i64) || int(get(d, "kCGWindowLayer")) != Some(0) {
            continue;
        }
        let b = get(d, "kCGWindowBounds");
        let num = |k: &str| {
            let r = if b.is_null() { std::ptr::null() } else { get(b, k) };
            let mut v: f64 = 0.0;
            (!r.is_null() && unsafe { CFNumberGetValue(r, 13, &mut v as *mut f64 as *mut c_void) }).then_some(v)
        };
        let frame = match (num("X"), num("Y"), num("Width"), num("Height")) {
            (Some(x), Some(y), Some(w), Some(h)) => Some(Rect { x, y, w, h }),
            _ => None,
        };
        let on = get(d, "kCGWindowIsOnscreen");
        let onscreen = !on.is_null() && unsafe { CFBooleanGetValue(on) };
        out.push((string(get(d, "kCGWindowName")), frame, onscreen));
    }
    out
}

pub fn snapshot(pid: i32, limits: Limits, scope: &Scope) -> Option<AxNode> {
    let app = Cf::owned(unsafe { AXUIElementCreateApplication(pid) })?;
    unsafe {
        AXUIElementSetMessagingTimeout(app.0, limits.timeout);
        AXUIElementSetAttributeValue(app.0, Cf::str("AXManualAccessibility").0, kCFBooleanTrue);
    }
    let refs: Vec<Cf> = ATTRS.iter().map(|a| Cf::str(a)).collect();
    let ptrs: Vec<Ref> = refs.iter().map(|c| c.0).collect();
    let names = Cf(unsafe { CFArrayCreate(std::ptr::null(), ptrs.as_ptr(), ptrs.len() as isize, &kCFTypeArrayCallBacks) });
    let mut walk = Walk { names, limits, scope, visited: 0 };
    let mut tree = walk.node(&app, 0, true);
    // Windows Accessibility doesn't show (another Space, minimised): only their title, as `AXWindow` with subrole
    // `RecordemElsewhere` — enough to know a call goes on there.
    let known: Vec<String> = tree.children.iter().filter_map(|w| w.title.clone()).collect();
    for (title, frame, _) in server_windows(pid) {
        let Some(title) = title.filter(|t| !t.is_empty() && !known.contains(t)) else { continue };
        tree.children.push(AxNode {
            role: Some("AXWindow".into()),
            subrole: Some(crate::ELSEWHERE.into()),
            title: Some(title),
            frame,
            ..Default::default()
        });
    }
    Some(tree)
}

/// The app's version (CFBundleShortVersionString): profiles can depend on it (Zoom changes its tree between releases).
pub fn app_version(pid: i32) -> Option<String> {
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::{NSBundle, NSString};
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
    let url = app.bundleURL()?;
    let bundle = NSBundle::bundleWithURL(&url)?;
    let v = bundle.objectForInfoDictionaryKey(&NSString::from_str("CFBundleShortVersionString"))?;
    v.downcast::<NSString>().ok().map(|s| s.to_string())
}

/// The app's windows (title and frame only), to pick the call window without reading every tree.
pub fn windows(pid: i32) -> Vec<AxNode> {
    let Some(app) = Cf::owned(unsafe { AXUIElementCreateApplication(pid) }) else { return Vec::new() };
    unsafe { AXUIElementSetMessagingTimeout(app.0, 0.5) };
    let Some(list) = app.attr("AXWindows") else { return Vec::new() };
    items(list.0)
        .iter()
        .map(|w| AxNode {
            role: Some("AXWindow".into()),
            title: w.attr("AXTitle").and_then(|t| string(t.0)),
            frame: match (
                w.attr("AXPosition").and_then(|p| ax_value::<[f64; 2]>(p.0, AX_VALUE_CGPOINT)),
                w.attr("AXSize").and_then(|s| ax_value::<[f64; 2]>(s.0, AX_VALUE_CGSIZE)),
            ) {
                (Some([x, y]), Some([w, h])) => Some(Rect { x, y, w, h }),
                _ => None,
            },
            ..Default::default()
        })
        .collect()
}
