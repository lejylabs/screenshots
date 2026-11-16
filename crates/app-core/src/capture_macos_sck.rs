//! Public ScreenCaptureKit screenshot API, loaded only for an admitted operation.
//! Dynamic class availability keeps older supported macOS launches intact.
use crate::capture_macos_ax as native;
use crate::{CaptureCursor, CaptureOptions, CaptureWindowShadow, MacLogicalRect, MacWindowProof};
use anyhow::{Context, Result, ensure};
use block2::RcBlock;
use core_foundation::{
    base::{CFRelease, CFRetain},
    runloop::{
        CFRunLoopAddSource, CFRunLoopGetCurrent, CFRunLoopRef, CFRunLoopRemoveSource,
        CFRunLoopRunInMode, CFRunLoopSourceContext, CFRunLoopSourceCreate,
        CFRunLoopSourceInvalidate, CFRunLoopSourceRef, CFRunLoopSourceSignal, CFRunLoopWakeUp,
        kCFRunLoopDefaultMode,
    },
    string::CFStringRef,
};
use objc2::{
    msg_send,
    rc::{Allocated, Retained},
    runtime::{AnyClass, AnyObject, Bool},
    sel,
};
use objc2_core_foundation::{CFString as ObjCfString, CGRect};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSBundle, NSString};
use std::{
    ffi::{CStr, c_void},
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Instant,
};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: CFStringRef;
}
pub(super) struct Image(pub *const c_void);
impl Drop for Image {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}
#[derive(Clone, Copy)]
enum ValueKind {
    Object,
    Image,
}
struct Pending {
    value: AtomicUsize,
    done: AtomicBool,
    failed: AtomicBool,
    active: AtomicBool,
    loop_: usize,
    source: usize,
    kind: ValueKind,
}
extern "C" fn perform(_: *const c_void) {}
impl Pending {
    fn new(kind: ValueKind) -> Result<Arc<Self>> {
        let loop_ = unsafe { CFRunLoopGetCurrent() };
        ensure!(!loop_.is_null(), "Native macOS run loop is unavailable");
        let mut context = CFRunLoopSourceContext {
            version: 0,
            info: ptr::null_mut(),
            retain: None,
            release: None,
            copyDescription: None,
            equal: None,
            hash: None,
            schedule: None,
            cancel: None,
            perform,
        };
        let source = unsafe { CFRunLoopSourceCreate(ptr::null(), 0, &mut context) };
        ensure!(
            !source.is_null(),
            "Cannot create finite native completion source"
        );
        unsafe {
            CFRetain(loop_.cast());
            CFRunLoopAddSource(loop_, source, kCFRunLoopDefaultMode);
        }
        Ok(Arc::new(Self {
            value: AtomicUsize::new(0),
            done: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            active: AtomicBool::new(true),
            loop_: loop_ as usize,
            source: source as usize,
            kind,
        }))
    }
    fn publish_object(&self, value: *mut AnyObject, error: *mut AnyObject) {
        if !self.active.load(Ordering::Acquire) {
            return;
        }
        let value = if error.is_null() {
            unsafe { Retained::retain(value) }
                .map(Retained::into_raw)
                .map(|value| value as usize)
        } else {
            None
        };
        self.publish(value);
    }
    fn publish_image(&self, value: *mut CGImage, error: *mut AnyObject) {
        if !self.active.load(Ordering::Acquire) {
            return;
        }
        let value = if error.is_null() && !value.is_null() {
            unsafe { CFRetain(value.cast()) };
            Some(value as usize)
        } else {
            None
        };
        self.publish(value);
    }
    fn publish(&self, value: Option<usize>) {
        if let Some(value) = value {
            if self
                .value
                .compare_exchange(0, value, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                self.release(value);
                self.failed.store(true, Ordering::Release);
            }
        } else {
            self.failed.store(true, Ordering::Release);
        }
        self.done.store(true, Ordering::Release);
        unsafe {
            CFRunLoopSourceSignal(self.source as CFRunLoopSourceRef);
            CFRunLoopWakeUp(self.loop_ as CFRunLoopRef);
        }
    }
    fn release(&self, value: usize) {
        if value == 0 {
            return;
        }
        match self.kind {
            ValueKind::Image => unsafe { CFRelease((value as *const c_void).cast()) },
            ValueKind::Object => unsafe {
                drop(Retained::<AnyObject>::from_raw(value as *mut AnyObject))
            },
        }
    }
    fn wait(&self, until: Instant) -> Result<usize> {
        while !self.done.load(Ordering::Acquire) {
            let remaining = until.saturating_duration_since(Instant::now());
            ensure!(
                !remaining.is_zero(),
                "Native macOS capture deadline exceeded"
            );
            unsafe {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, remaining.as_secs_f64(), 1);
            }
        }
        ensure!(
            !self.failed.load(Ordering::Acquire),
            "Native macOS capture returned no usable source/image"
        );
        let value = self.value.swap(0, Ordering::AcqRel);
        ensure!(value != 0, "Native macOS completion is empty");
        Ok(value)
    }
    fn retire(&self) {
        self.active.store(false, Ordering::Release);
        unsafe {
            CFRunLoopRemoveSource(
                self.loop_ as CFRunLoopRef,
                self.source as CFRunLoopSourceRef,
                kCFRunLoopDefaultMode,
            );
            CFRunLoopSourceInvalidate(self.source as CFRunLoopSourceRef);
        }
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.release(self.value.swap(0, Ordering::AcqRel));
        unsafe {
            CFRelease((self.source as CFRunLoopSourceRef).cast());
            CFRelease((self.loop_ as CFRunLoopRef).cast());
        }
    }
}
struct Completion(Arc<Pending>);
impl Completion {
    fn new(kind: ValueKind) -> Result<Self> {
        Ok(Self(Pending::new(kind)?))
    }
}
impl Drop for Completion {
    fn drop(&mut self) {
        self.0.retire();
    }
}

fn class(name: &CStr) -> Result<&'static AnyClass> {
    AnyClass::get(name)
        .context("Required public ScreenCaptureKit API is unavailable on this macOS version")
}
fn load() -> Result<bool> {
    native::main_thread()?;
    let path = NSString::from_str("/System/Library/Frameworks/ScreenCaptureKit.framework");
    let Some(bundle) = NSBundle::bundleWithPath(&path) else {
        return Ok(false);
    };
    if !bundle.isLoaded() {
        unsafe { bundle.loadAndReturnError() }
            .map_err(|_| anyhow::anyhow!("Cannot load the public ScreenCaptureKit framework"))?;
    }
    Ok(AnyClass::get(c"SCScreenshotManager").is_some())
}
pub(super) fn available() -> Result<bool> {
    load()
}
fn shareable(until: Instant) -> Result<Retained<AnyObject>> {
    let pending = Completion::new(ValueKind::Object)?;
    let state = pending.0.clone();
    let block = RcBlock::new(move |content: *mut AnyObject, error: *mut AnyObject| {
        state.publish_object(content, error)
    });
    let proxy = class(c"SCShareableContent")?;
    unsafe {
        let _: () = msg_send![proxy,getShareableContentExcludingDesktopWindows:Bool::YES, onScreenWindowsOnly:Bool::YES, completionHandler:&*block];
    }
    let raw = pending.0.wait(until)?;
    unsafe { Retained::from_raw(raw as *mut AnyObject) }
        .context("Native shareable content was released")
}
pub(super) struct Window {
    filter: Retained<AnyObject>,
    pub proof: MacWindowProof,
    pub scale: f64,
    pub width: u32,
    pub height: u32,
}
pub(super) struct Display {
    filter: Retained<AnyObject>,
    pub proof: crate::MacDisplayProof,
    pub width: u32,
    pub height: u32,
}
impl Display {
    pub fn new(id: u32, until: Instant) -> Result<Option<Self>> {
        native::screen_permission()?;
        if !load()? {
            return Ok(None);
        }
        let proof = native::displays()?
            .into_iter()
            .find(|display| display.id == id)
            .context("Selected native display is absent")?;
        let content = shareable(until)?;
        let displays: Retained<NSArray<AnyObject>> = unsafe { msg_send![&content, displays] };
        ensure!(
            displays.count() <= 32,
            "Native shareable display inventory exceeds its bound"
        );
        let mut selected = None;
        for index in 0..displays.count() {
            let display = displays.objectAtIndex(index);
            let candidate: u32 = unsafe { msg_send![&display, displayID] };
            if candidate == id {
                ensure!(selected.is_none(), "Native display identity is ambiguous");
                selected = Some(display);
            }
        }
        let selected = selected.context("Selected display is not shareable")?;
        let width: usize = unsafe { msg_send![&selected, width] };
        let height: usize = unsafe { msg_send![&selected, height] };
        let width = u32::try_from(width)?;
        let height = u32::try_from(height)?;
        crate::validate_image_size(width, height)?;
        ensure!(
            (width, height) == (proof.pixel_width, proof.pixel_height),
            "Native display pixel geometry changed or needs rotation calibration"
        );
        let empty = NSArray::<AnyObject>::new();
        let allocated: Allocated<AnyObject> =
            unsafe { msg_send![class(c"SCContentFilter")?, alloc] };
        let filter: Retained<AnyObject> =
            unsafe { msg_send![allocated,initWithDisplay:&*selected,excludingWindows:&*empty] };
        Ok(Some(Self {
            filter,
            proof,
            width,
            height,
        }))
    }
    pub fn capture(&self, options: &CaptureOptions, until: Instant) -> Result<Image> {
        native::screen_permission()?;
        self.check()?;
        // A display image includes its real composited window shadows; per-window
        // shadow options do not remove elements from an explicitly selected display.
        let mut capture = *options;
        capture.window_shadow = CaptureWindowShadow::Exclude;
        let image = capture_legacy_sck(&self.filter, self.width, self.height, &capture, until)?;
        self.check()?;
        Ok(image)
    }
    fn check(&self) -> Result<()> {
        let current = native::displays()?
            .into_iter()
            .find(|display| display.id == self.proof.id)
            .context("Selected display disappeared")?;
        ensure!(
            current == self.proof,
            "Native display UUID/geometry changed"
        );
        Ok(())
    }
}
impl Window {
    pub fn new(proof: MacWindowProof, until: Instant) -> Result<Option<Self>> {
        native::screen_permission()?;
        if !load()? {
            return Ok(None);
        }
        let content = shareable(until)?;
        let windows: Retained<NSArray<AnyObject>> = unsafe { msg_send![&content, windows] };
        ensure!(
            windows.count() <= 1024,
            "Native shareable window count exceeds its limit"
        );
        let mut selected = None;
        for index in 0..windows.count() {
            let window = windows.objectAtIndex(index);
            let id: u32 = unsafe { msg_send![&window, windowID] };
            if id != proof.id {
                continue;
            }
            ensure!(selected.is_none(), "Native window identity is ambiguous");
            let app: Option<Retained<AnyObject>> = unsafe { msg_send![&window, owningApplication] };
            let app = app.context("Native window owner is unavailable")?;
            let pid: i32 = unsafe { msg_send![&app, processID] };
            let rect: CGRect = unsafe { msg_send![&window, frame] };
            let bounds = MacLogicalRect {
                x: rect.origin.x,
                y: rect.origin.y,
                width: rect.size.width,
                height: rect.size.height,
            };
            ensure!(
                pid > 0 && pid as u32 == proof.pid && proof.bounds.same_bounds(bounds),
                "Native source window process/geometry changed"
            );
            selected = Some(window);
        }
        let selected = selected.context("Selected window is absent from shareable content")?;
        let allocated: Allocated<AnyObject> =
            unsafe { msg_send![class(c"SCContentFilter")?, alloc] };
        let filter: Retained<AnyObject> =
            unsafe { msg_send![allocated,initWithDesktopIndependentWindow:&*selected] };
        let supports: Bool = unsafe { msg_send![&filter,respondsToSelector:sel!(pointPixelScale)] };
        ensure!(
            supports.as_bool(),
            "Native screenshot pixel-scale API is unavailable"
        );
        let scale: f32 = unsafe { msg_send![&filter, pointPixelScale] };
        let rect: CGRect = unsafe { msg_send![&filter, contentRect] };
        let size = MacLogicalRect {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.width,
            height: rect.size.height,
        };
        let (width, height) = size.pixel_size(f64::from(scale))?;
        Ok(Some(Self {
            filter,
            proof,
            scale: f64::from(scale),
            width,
            height,
        }))
    }
    pub fn capture(&self, options: &CaptureOptions, until: Instant) -> Result<Image> {
        native::screen_permission()?;
        self.proof
            .verify_unchanged(native::window(self.proof.id)?)?;
        ensure!(
            options.window_frame != crate::CaptureWindowFrame::Client,
            "Client-only macOS window bounds are unavailable"
        );
        let image = if AnyClass::get(c"SCScreenshotConfiguration").is_some() {
            capture_modern(&self.filter, options, until)?
        } else {
            capture_legacy_sck(&self.filter, self.width, self.height, options, until)?
        };
        self.proof
            .verify_unchanged(native::window(self.proof.id)?)?;
        Ok(image)
    }
}
fn capture_legacy_sck(
    filter: &AnyObject,
    width: u32,
    height: u32,
    options: &CaptureOptions,
    until: Instant,
) -> Result<Image> {
    // The older fixed-canvas API supplies no public full-shadow extent.
    ensure!(
        options.window_shadow != CaptureWindowShadow::Include,
        "Unclipped native shadow capture requires macOS26's screenshot configuration"
    );
    let config: Retained<AnyObject> = unsafe { msg_send![class(c"SCStreamConfiguration")?, new] };
    let srgb = unsafe { &*(kCGColorSpaceSRGB.cast::<ObjCfString>()) };
    unsafe {
        let _: () = msg_send![&config,setWidth:width as usize];
        let _: () = msg_send![&config,setHeight:height as usize];
        let _: () = msg_send![&config,setPixelFormat:0x42475241u32];
        let _: () = msg_send![&config,setColorSpaceName:srgb];
        let _: () =
            msg_send![&config,setShowsCursor:Bool::new(options.cursor==CaptureCursor::Include)];
        let _: () = msg_send![&config,setShouldBeOpaque:Bool::NO];
        let _: () = msg_send![&config,setIgnoreShadowsSingleWindow:Bool::YES];
        let _: () = msg_send![&config,setQueueDepth:1usize];
    }
    let pending = Completion::new(ValueKind::Image)?;
    let state = pending.0.clone();
    let block = RcBlock::new(move |image: *mut CGImage, error: *mut AnyObject| {
        state.publish_image(image, error)
    });
    unsafe {
        let _: () = msg_send![class(c"SCScreenshotManager")?,captureImageWithFilter:filter, configuration:&*config, completionHandler:&*block];
    }
    Ok(Image(pending.0.wait(until)? as *const c_void))
}
fn capture_modern(filter: &AnyObject, options: &CaptureOptions, until: Instant) -> Result<Image> {
    // macOS26 defaults to captured-content dimensions, including requested native effects.
    let config: Retained<AnyObject> =
        unsafe { msg_send![class(c"SCScreenshotConfiguration")?, new] };
    unsafe {
        let _: () = msg_send![&config,setFileURL:ptr::null::<AnyObject>()];
        let _: () = msg_send![&config,setDynamicRange:0isize];
        let _: () =
            msg_send![&config,setShowsCursor:Bool::new(options.cursor==CaptureCursor::Include)];
        let _: () = msg_send![&config,setIgnoreShadows:Bool::new(options.window_shadow!=CaptureWindowShadow::Include)];
        let _: () = msg_send![&config,setIncludeChildWindows:Bool::NO];
    }
    let pending = Completion::new(ValueKind::Object)?;
    let state = pending.0.clone();
    let block = RcBlock::new(move |output: *mut AnyObject, error: *mut AnyObject| {
        state.publish_object(output, error)
    });
    unsafe {
        let _: () = msg_send![class(c"SCScreenshotManager")?,captureScreenshotWithFilter:filter, configuration:&*config, completionHandler:&*block];
    }
    let raw = pending.0.wait(until)?;
    let output = unsafe { Retained::<AnyObject>::from_raw(raw as *mut AnyObject) }
        .context("Native screenshot output disappeared")?;
    let image: *mut CGImage = unsafe { msg_send![&output, sdrImage] };
    ensure!(!image.is_null(), "Native screenshot supplied no SDR image");
    unsafe { CFRetain(image.cast()) };
    Ok(Image(image.cast()))
}
