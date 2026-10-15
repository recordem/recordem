use anyhow::{Context, anyhow, bail};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::AVFileTypeMPEG4;
use objc2_foundation::{NSError, NSString, NSURL};
use objc2_screen_capture_kit::{
    SCContentFilter, SCRecordingOutput, SCRecordingOutputConfiguration, SCRecordingOutputDelegate, SCShareableContent,
    SCStream, SCStreamConfiguration, SCStreamDelegate, SCWindow,
};
use recordem_ax::Rect;
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// Why a capture ended on its own, if it did (the window closed — Meet's picture-in-picture does when the person
/// returns to the tab —, the permission was revoked…).
type Ended = Arc<Mutex<Option<String>>>;

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "RecordemRecordingDelegate"]
    #[ivars = Ended]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}
    unsafe impl SCRecordingOutputDelegate for Delegate {
        #[unsafe(method(recordingOutput:didFailWithError:))]
        fn recording_failed(&self, _output: &SCRecordingOutput, error: &NSError) {
            *self.ivars().lock().unwrap() = Some(format!("recording failed: {}", error.localizedDescription()));
        }
    }
    unsafe impl SCStreamDelegate for Delegate {
        #[unsafe(method(stream:didStopWithError:))]
        fn stream_stopped(&self, _stream: &SCStream, error: &NSError) {
            *self.ivars().lock().unwrap() =
                Some(format!("capture stopped: {} (code {})", error.localizedDescription(), error.code()));
        }
    }
);

impl Delegate {
    fn new(ended: Ended) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ended);
        unsafe { msg_send![super(this), init] }
    }
}

/// Waits for a completion handler that reports an optional NSError.
fn wait(start: impl FnOnce(&RcBlock<dyn Fn(*mut NSError)>)) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel::<Option<String>>();
    let block = RcBlock::new(move |err: *mut NSError| {
        let msg = unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string());
        let _ = tx.send(msg);
    });
    start(&block);
    match rx.recv_timeout(Duration::from_secs(10)).context("ScreenCaptureKit did not answer")? {
        Some(msg) => Err(anyhow!(msg)),
        None => Ok(()),
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
}

fn shareable_windows() -> anyhow::Result<Vec<Retained<SCWindow>>> {
    // A process without NSApplication (the probe, a helper) has no WindowServer connection until a CoreGraphics
    // call makes one, and ScreenCaptureKit asserts on it.
    unsafe { CGMainDisplayID() };
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
        let out = match unsafe { content.as_ref() } {
            Some(c) => Ok(unsafe { c.windows() }.to_vec()),
            None => Err(unsafe { err.as_ref() }
                .map(|e| e.localizedDescription().to_string())
                .unwrap_or_else(|| "no shareable content (Screen Recording permission?)".into())),
        };
        let _ = tx.send(out);
    });
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    rx.recv_timeout(Duration::from_secs(10)).context("ScreenCaptureKit did not list windows")?.map_err(|e| anyhow!(e))
}

pub struct WindowRecording {
    stream: Retained<SCStream>,
    _output: Retained<SCRecordingOutput>,
    _delegate: Retained<Delegate>,
    ended: Ended,
    /// The captured size, in pixels: what crop_plan needs.
    pub width: u32,
    pub height: u32,
}

/// Starts recording the on-screen window of `pid` closest to `near` (the AX frame of the call window) into `path`.
pub fn record_window(pid: i32, near: Option<Rect>, path: &Path) -> anyhow::Result<WindowRecording> {
    let windows: Vec<_> = shareable_windows()?
        .into_iter()
        .filter(|w| unsafe { w.isOnScreen() && w.owningApplication().is_some_and(|a| a.processID() == pid) })
        .collect();
    let distance = |w: &SCWindow| {
        let f = unsafe { w.frame() };
        match near {
            Some(r) => (f.origin.x - r.x).abs() + (f.origin.y - r.y).abs() + (f.size.width - r.w).abs() + (f.size.height - r.h).abs(),
            None => -(f.size.width * f.size.height),
        }
    };
    let window = windows
        .iter()
        .min_by(|a, b| distance(a).total_cmp(&distance(b)))
        .ok_or_else(|| anyhow!("no on-screen window of process {pid}"))?;
    let frame = unsafe { window.frame() };
    // Retina: twice the points. Good enough to read faces in a tile; the stream scales if the window is smaller.
    let (width, height) = (((frame.size.width * 2.0) as u32) & !1, ((frame.size.height * 2.0) as u32) & !1);
    if width == 0 || height == 0 {
        bail!("the window has no size");
    }
    unsafe {
        let filter = SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), window);
        let config = SCStreamConfiguration::new();
        config.setWidth(width as usize);
        config.setHeight(height as usize);
        config.setShowsCursor(false);
        let ended: Ended = Arc::default();
        let delegate = Delegate::new(ended.clone());
        let stream = SCStream::initWithFilter_configuration_delegate(
            SCStream::alloc(),
            &filter,
            &config,
            Some(ProtocolObject::from_ref(&*delegate)),
        );
        let rec = SCRecordingOutputConfiguration::new();
        rec.setOutputURL(&NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy())));
        rec.setOutputFileType(AVFileTypeMPEG4.context("AVFileTypeMPEG4")?);
        let output =
            SCRecordingOutput::initWithConfiguration_delegate(SCRecordingOutput::alloc(), &rec, ProtocolObject::from_ref(&*delegate));
        stream.addRecordingOutput_error(&output).map_err(|e| anyhow!(e.localizedDescription().to_string()))?;
        wait(|b| stream.startCaptureWithCompletionHandler(Some(b)))?;
        Ok(WindowRecording { stream, _output: output, _delegate: delegate, ended, width, height })
    }
}

impl WindowRecording {
    /// Why the capture ended by itself, if it did; the file is closed then.
    pub fn ended(&self) -> Option<String> {
        self.ended.lock().unwrap().clone()
    }

    /// Stops and finishes the file (nothing to do if the capture already ended).
    pub fn stop(self) -> anyhow::Result<()> {
        if self.ended().is_none() {
            wait(|b| unsafe { self.stream.stopCaptureWithCompletionHandler(Some(b)) })?;
        }
        // The recording output closes the file right after the stream stops.
        std::thread::sleep(Duration::from_millis(500));
        Ok(())
    }
}
