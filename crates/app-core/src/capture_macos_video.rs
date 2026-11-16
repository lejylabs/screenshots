//! Camera-safe fixed-CGDisplayID AVFoundation producer for an owned native helper.
use crate::{
    MacFrameClock, MacFrameLease, MacFrameSlot, MacMediaTimestamp, MacVideoRequest, MacVideoSummary,
};
use crate::{
    capture_macos_ax as native,
    capture_macos_loop::{EventLoop, Topology, Wake},
};
use anyhow::{Context, Result, ensure};
use block2::RcBlock;
use core_foundation::{
    base::{CFGetTypeID, CFTypeRef, TCFType},
    string::{CFString, CFStringGetTypeID, CFStringRef},
};
use dispatch2::{DispatchQueue, DispatchQueueAttr};
use objc2::{
    AllocAnyThread, DefinedClass, define_class, msg_send, rc::Retained, runtime::ProtocolObject,
};
use objc2_av_foundation::{
    AVCaptureConnection, AVCaptureDevice, AVCaptureOutput, AVCaptureScreenInput, AVCaptureSession,
    AVCaptureSessionDidStopRunningNotification, AVCaptureSessionRuntimeErrorNotification,
    AVCaptureSessionWasInterruptedNotification, AVCaptureVideoDataOutput,
    AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaTypeAudio,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetDataSize,
    CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferIsPlanar, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress, kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{
    NSDictionary, NSNotification, NSNotificationCenter, NSNumber, NSObject, NSObjectProtocol,
    NSString,
};
use std::{
    ffi::c_void,
    fmt,
    ptr::NonNull,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering},
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant},
};

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVImageBufferGetColorSpace(buffer: *const c_void) -> *const c_void;
    fn CVBufferGetAttachment(buffer: *const c_void, key: CFStringRef, mode: *mut u32) -> CFTypeRef;
}
const FORMAT: u8 = 1;
const BACKPRESSURE: u8 = 2;
const DROPPED: u8 = 3;
const TIMESTAMP: u8 = 4;
const WRITE: u8 = 5;
const TOPOLOGY: u8 = 6;
const INTERRUPTED: u8 = 7;
const SESSION_ERROR: u8 = 8;
const SESSION_STOPPED: u8 = 9;
struct Frame {
    bytes: Vec<u8>,
    lease: MacFrameLease,
}
struct State {
    request: MacVideoRequest,
    slot: MacFrameSlot,
    clock: Mutex<MacFrameClock>,
    failure: AtomicU8,
    started: AtomicBool,
    captured: AtomicBool,
    finished: AtomicBool,
    accepting: AtomicBool,
    delivered: AtomicU32,
    stopping: AtomicBool,
    wake: Arc<Wake>,
}
impl State {
    fn fail(&self, kind: u8) {
        let _ = self
            .failure
            .compare_exchange(0, kind, Ordering::AcqRel, Ordering::Acquire);
        self.accepting.store(false, Ordering::Release);
        self.slot.fail();
        self.wake.notify();
    }
    fn check(&self) -> Result<()> {
        ensure!(
            self.failure.load(Ordering::Acquire) == 0,
            "Native screen source failed (format, clock, interruption, session error, dropped frame, backpressure, topology or encoder)"
        );
        Topology::check()
    }
}
struct SessionNotifications {
    center: Retained<NSNotificationCenter>,
    tokens: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
    active: Arc<Mutex<bool>>,
}
impl SessionNotifications {
    fn new(
        center: Retained<NSNotificationCenter>,
        session: &AVCaptureSession,
        state: Arc<State>,
    ) -> Self {
        let active = Arc::new(Mutex::new(true));
        let mut tokens = Vec::with_capacity(3);
        for (name, failure) in unsafe {
            [
                (AVCaptureSessionWasInterruptedNotification, INTERRUPTED),
                (AVCaptureSessionRuntimeErrorNotification, SESSION_ERROR),
                (AVCaptureSessionDidStopRunningNotification, SESSION_STOPPED),
            ]
        } {
            let state = state.clone();
            let entered = active.clone();
            let callback = RcBlock::new(move |_: NonNull<NSNotification>| {
                let enabled = entered
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if *enabled
                    && !(failure == SESSION_STOPPED && state.stopping.load(Ordering::Acquire))
                {
                    state.fail(failure);
                }
            });
            // A nil queue delivers synchronously on the posting thread. The short
            // gate drains an entered callback; its Arc remains valid for late calls.
            let token = unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    Some(session),
                    None,
                    &callback,
                )
            };
            tokens.push(token);
        }
        Self {
            center,
            tokens,
            active,
        }
    }
}
impl Drop for SessionNotifications {
    fn drop(&mut self) {
        for token in &self.tokens {
            unsafe {
                self.center.removeObserver(token);
            }
        }
        *self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
    }
}
struct Vars {
    state: Arc<State>,
    sender: SyncSender<Frame>,
}
impl fmt::Debug for Vars {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BoundedMacVideoDelegate")
    }
}
define_class!(
    #[unsafe(super(NSObject))]
    #[name = "ScreenshotsMacVideoDelegate"]
    #[ivars=Vars]
    struct Delegate;
    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for Delegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        unsafe fn frame(
            &self,
            _: &AVCaptureOutput,
            sample: &CMSampleBuffer,
            _: &AVCaptureConnection,
        ) {
            let vars = self.ivars();
            let state = &vars.state;
            if !state.accepting.load(Ordering::Acquire) {
                return;
            }
            if Topology::check().is_err() {
                state.fail(TOPOLOGY);
                return;
            }
            let lease = match state.slot.reserve() {
                Ok(lease) => lease,
                Err(_) => {
                    state.fail(BACKPRESSURE);
                    return;
                }
            };
            let result = (|| -> Result<Vec<u8>> {
                let time = unsafe { sample.presentation_time_stamp() };
                let timestamp = MacMediaTimestamp {
                    value: time.value,
                    timescale: time.timescale,
                    epoch: time.epoch,
                    flags: time.flags.0,
                };
                let mut clock = state
                    .clock
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Native clock state failed"))?;
                if clock.accept(timestamp).is_err() {
                    state.fail(TIMESTAMP);
                    anyhow::bail!("Native timestamp discontinuity");
                }
                drop(clock);
                let buffer = unsafe { sample.image_buffer() }
                    .context("Native sample has no image buffer")?;
                ensure!(
                    !unsafe { CVPixelBufferIsPlanar(&buffer) }
                        && unsafe { CVPixelBufferGetPixelFormatType(&buffer) }
                            == kCVPixelFormatType_32BGRA,
                    "Native video is not packed BGRA8"
                );
                let width = unsafe { CVPixelBufferGetWidth(&buffer) };
                let height = unsafe { CVPixelBufferGetHeight(&buffer) };
                ensure!(
                    (width, height)
                        == (
                            state.request.region.width as usize,
                            state.request.region.height as usize
                        ),
                    "Native video pixel rectangle changed"
                );
                ensure!(
                    unsafe {
                        CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::ReadOnly)
                    } == 0,
                    "Cannot lock native video buffer"
                );
                struct Unlock<'a>(&'a objc2_core_video::CVPixelBuffer);
                impl Drop for Unlock<'_> {
                    fn drop(&mut self) {
                        unsafe {
                            CVPixelBufferUnlockBaseAddress(
                                self.0,
                                CVPixelBufferLockFlags::ReadOnly,
                            );
                        }
                    }
                }
                let _unlock = Unlock(&buffer);
                let base = unsafe { CVPixelBufferGetBaseAddress(&buffer) };
                let length = unsafe { CVPixelBufferGetDataSize(&buffer) };
                let stride = unsafe { CVPixelBufferGetBytesPerRow(&buffer) };
                ensure!(
                    !base.is_null()
                        && length > 0
                        && length <= 16 * 1024 * 1024
                        && stride >= width * 4
                        && stride
                            .checked_mul(height)
                            .is_some_and(|extent| extent <= length),
                    "Invalid native video buffer extent"
                );
                let key = CFString::new("TransferFunction");
                let transfer = unsafe {
                    CVBufferGetAttachment(
                        (&*buffer as *const objc2_core_video::CVPixelBuffer).cast(),
                        key.as_concrete_TypeRef(),
                        std::ptr::null_mut(),
                    )
                };
                let mut requires_profile = false;
                if !transfer.is_null() {
                    ensure!(
                        unsafe { CFGetTypeID(transfer) } == unsafe { CFStringGetTypeID() },
                        "Native transfer metadata is malformed"
                    );
                    let transfer = unsafe { CFString::wrap_under_get_rule(transfer.cast()) };
                    ensure!(
                        transfer.char_len() <= 64,
                        "Native transfer metadata exceeds its bound"
                    );
                    let transfer = transfer.to_string();
                    ensure!(
                        [
                            "sRGB",
                            "ITU_R_709_2",
                            "SMPTE_240M_1995",
                            "UseGamma",
                            "Linear"
                        ]
                        .contains(&transfer.as_str()),
                        "HDR/unknown native transfer function is unsupported"
                    );
                    requires_profile = transfer != "sRGB";
                }
                let bytes = unsafe { std::slice::from_raw_parts(base.cast::<u8>(), length) };
                let space = unsafe {
                    CVImageBufferGetColorSpace(
                        (&*buffer as *const objc2_core_video::CVPixelBuffer).cast(),
                    )
                };
                ensure!(
                    !requires_profile || !space.is_null(),
                    "Native transfer function has no color profile"
                );
                let bytes = unsafe {
                    crate::capture_macos_image::opaque_bgra_srgb(
                        state.request.region.width,
                        state.request.region.height,
                        stride,
                        bytes,
                        space,
                    )
                }?;
                Topology::check()?;
                Ok(bytes)
            })();
            match result {
                Ok(bytes) => {
                    if vars.sender.try_send(Frame { bytes, lease }).is_err() {
                        state.fail(BACKPRESSURE);
                        return;
                    }
                    state.started.store(true, Ordering::Release);
                    if state
                        .clock
                        .lock()
                        .is_ok_and(|clock| clock.expected_reached())
                    {
                        state.accepting.store(false, Ordering::Release);
                        state.captured.store(true, Ordering::Release);
                    }
                    state.wake.notify();
                }
                Err(_) => {
                    drop(lease);
                    state.fail(FORMAT);
                }
            }
        }
        #[unsafe(method(captureOutput:didDropSampleBuffer:fromConnection:))]
        unsafe fn dropped(
            &self,
            _: &AVCaptureOutput,
            sample: &CMSampleBuffer,
            _: &AVCaptureConnection,
        ) {
            let state = &self.ivars().state;
            if state.accepting.load(Ordering::Acquire) {
                state.fail(DROPPED);
                return;
            }
            if state.captured.load(Ordering::Acquire) {
                let pts = unsafe { sample.presentation_time_stamp() };
                let dropped = MacMediaTimestamp {
                    value: pts.value,
                    timescale: pts.timescale,
                    epoch: pts.epoch,
                    flags: pts.flags.0,
                }
                .nanoseconds();
                let last = state
                    .clock
                    .lock()
                    .ok()
                    .and_then(|clock| clock.finish().ok())
                    .map(|summary| summary.last_pts_ns);
                if !matches!((dropped,last),(Ok(dropped),Some(last))if dropped>last) {
                    state.fail(DROPPED);
                }
            }
        }
    }
);
unsafe impl NSObjectProtocol for Delegate {}
impl Delegate {
    fn new(state: Arc<State>, sender: SyncSender<Frame>) -> Retained<Self> {
        let value = Self::alloc().set_ivars(Vars { state, sender });
        unsafe { msg_send![super(value), init] }
    }
}

/// Only call after the desktop controller has authenticated the raw sink and
/// reserved its process group/admission. Native APIs run only in this helper.
pub fn run_macos_video_source_in_worker(
    request: MacVideoRequest,
    expected: crate::MacDisplayProof,
    deadline: Instant,
    sink: impl FnMut(&[u8]) -> Result<()> + Send,
) -> Result<MacVideoSummary> {
    objc2::rc::autoreleasepool(|_| run_source(request, expected, deadline, sink))
}
fn run_source(
    request: MacVideoRequest,
    expected: crate::MacDisplayProof,
    deadline: Instant,
    mut sink: impl FnMut(&[u8]) -> Result<()> + Send,
) -> Result<MacVideoSummary> {
    ensure!(
        preflight_macos_video_source_in_worker(request)? == expected,
        "Display identity changed after recording preflight"
    );
    let event = EventLoop::new()?;
    let _topology = Topology::new(event.wake())?;
    let display = native::displays()?
        .into_iter()
        .find(|display| display.id == request.display)
        .context("Selected display is not active")?;
    let crop = display.screen_input_crop(request.region)?;
    ensure!(
        display == expected,
        "Display UUID/topology changed before recording source start"
    );
    Topology::check()?;
    let state = Arc::new(State {
        request,
        slot: MacFrameSlot::default(),
        clock: Mutex::new(MacFrameClock::new(request)?),
        failure: AtomicU8::new(0),
        started: AtomicBool::new(false),
        captured: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        accepting: AtomicBool::new(true),
        delivered: AtomicU32::new(0),
        stopping: AtomicBool::new(false),
        wake: event.wake(),
    });
    let (sender, receiver) = mpsc::sync_channel::<Frame>(1);
    let session = unsafe { AVCaptureSession::new() };
    let notifications = SessionNotifications::new(
        NSNotificationCenter::defaultCenter(),
        &session,
        state.clone(),
    );
    // This concrete class cannot select a video camera, irrespective of names or device count.
    let input = unsafe {
        AVCaptureScreenInput::initWithDisplayID(AVCaptureScreenInput::alloc(), request.display)
    }
    .context("Selected display cannot be captured")?;
    unsafe {
        input.setCapturesCursor(request.cursor);
        input.setCapturesMouseClicks(false);
        input.setScaleFactor(display.unrotated_scale()?);
        input.setCropRect(CGRect {
            origin: CGPoint {
                x: crop.x,
                y: crop.y,
            },
            size: CGSize {
                width: crop.width,
                height: crop.height,
            },
        });
        input.setMinFrameDuration(CMTime::new(1, request.fps as i32));
    }
    ensure!(
        unsafe { session.canAddInput(&input) },
        "Native display input is unsupported by its session"
    );
    unsafe {
        session.addInput(&input);
    }
    let output = unsafe { AVCaptureVideoDataOutput::new() };
    let key = NSString::from_str(
        unsafe { kCVPixelBufferPixelFormatTypeKey }
            .to_string()
            .as_str(),
    );
    let format = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
    let settings: Retained<NSDictionary<NSString>> =
        NSDictionary::from_slices::<NSString>(&[key.as_ref()], &[format.as_ref()]);
    unsafe {
        output.setVideoSettings(Some(&settings));
        output.setAlwaysDiscardsLateVideoFrames(true);
    }
    ensure!(
        unsafe { session.canAddOutput(&output) },
        "Native video output is unsupported by its session"
    );
    unsafe {
        session.addOutput(&output);
    }
    let delegate = Delegate::new(state.clone(), sender);
    let queue = DispatchQueue::new("screenshots.mac.video.samples", DispatchQueueAttr::SERIAL);
    let protocol =
        ProtocolObject::<dyn AVCaptureVideoDataOutputSampleBufferDelegate>::from_ref(&*delegate);
    unsafe {
        let _: () = msg_send![&output,setSampleBufferDelegate:protocol,queue:&*queue];
    }
    let session_queue =
        DispatchQueue::new("screenshots.mac.video.session", DispatchQueueAttr::SERIAL);
    // Exclusive handoff to the serial session queue; retain on this thread until
    // its synchronous stop/drain barrier completes. No concurrent session access.
    let session_ptr = Retained::as_ptr(&session) as usize;
    let result = std::thread::scope(|scope| -> Result<_> {
        let writing = state.clone();
        let writer = std::thread::Builder::new()
            .name("mac-video-raw".into())
            .stack_size(256 * 1024)
            .spawn_scoped(scope, move || {
                for _ in 0..request.frames().unwrap() {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    let frame = match receiver.recv_timeout(remaining) {
                        Ok(frame) => frame,
                        Err(_) => {
                            writing.fail(WRITE);
                            return;
                        }
                    };
                    if writing.check().is_err() || sink(&frame.bytes).is_err() {
                        writing.fail(WRITE);
                        return;
                    }
                    let Frame { bytes, lease } = frame;
                    drop(bytes);
                    if lease.complete().is_err() {
                        writing.fail(WRITE);
                        return;
                    }
                    writing.delivered.fetch_add(1, Ordering::AcqRel);
                }
                writing.finished.store(true, Ordering::Release);
                writing.wake.notify();
            })?;
        session_queue.exec_async(move || unsafe {
            (&*(session_ptr as *const AVCaptureSession)).startRunning()
        });
        let running = (|| -> Result<()> {
            event.wait(
                deadline.min(Instant::now() + Duration::from_secs(5)),
                || {
                    state.started.load(Ordering::Acquire)
                        || state.failure.load(Ordering::Acquire) != 0
                        || Topology::check().is_err()
                },
            )?;
            state.check()?;
            event.wait(deadline, || {
                state.captured.load(Ordering::Acquire)
                    || state.failure.load(Ordering::Acquire) != 0
                    || Topology::check().is_err()
            })?;
            state.check()
        })();
        state.accepting.store(false, Ordering::Release);
        state.stopping.store(true, Ordering::Release);
        session_queue.exec_sync(move || unsafe {
            (&*(session_ptr as *const AVCaptureSession)).stopRunning()
        });
        let no_delegate: Option<&ProtocolObject<dyn AVCaptureVideoDataOutputSampleBufferDelegate>> =
            None;
        let no_queue: Option<&DispatchQueue> = None;
        unsafe {
            let _: () = msg_send![&output,setSampleBufferDelegate:no_delegate,queue:no_queue];
        }
        queue.barrier_sync(|| {});
        drop(notifications);
        if running.is_err() {
            state.fail(WRITE);
        }
        // Dropping all producer senders releases the writer immediately on failure.
        drop(delegate);
        if running.is_ok() {
            event.wait(deadline, || {
                state.finished.load(Ordering::Acquire) || state.failure.load(Ordering::Acquire) != 0
            })?;
        }
        writer
            .join()
            .map_err(|_| anyhow::anyhow!("Native raw writer failed"))?;
        running?;
        state.check()?;
        state.slot.finish()?;
        ensure!(
            state.delivered.load(Ordering::Acquire) == request.frames()?,
            "Native raw frame delivery ended early"
        );
        let current = native::displays()?
            .into_iter()
            .find(|current| current.id == display.id)
            .context("Recorded display disappeared")?;
        ensure!(current == display, "Recorded display UUID/topology changed");
        let summary = state
            .clock
            .lock()
            .map_err(|_| anyhow::anyhow!("Native clock state failed"))?
            .finish()?;
        Ok(summary)
    });
    result
}
pub fn preflight_macos_video_source_in_worker(
    request: MacVideoRequest,
) -> Result<crate::MacDisplayProof> {
    objc2::rc::autoreleasepool(|_| preflight_source(request))
}
fn preflight_source(request: MacVideoRequest) -> Result<crate::MacDisplayProof> {
    request.validate()?;
    native::screen_permission()?;
    if request.microphone {
        let audio = unsafe { AVMediaTypeAudio }.context("Native audio media type unavailable")?;
        if unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) }.0 != 3 {
            return Err(crate::CaptureError::MacMicrophonePermissionDenied.into());
        }
    }
    let display = native::displays()?
        .into_iter()
        .find(|display| display.id == request.display)
        .context("Selected display is unavailable")?;
    display.unrotated_scale()?;
    display.screen_input_crop(request.region)?;
    Ok(display)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(wake: Arc<Wake>) -> Arc<State> {
        let request = MacVideoRequest {
            display: 7,
            region: crate::PixelRegion {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            seconds: 1,
            fps: 5,
            cursor: false,
            microphone: false,
        };
        Arc::new(State {
            request,
            slot: MacFrameSlot::default(),
            clock: Mutex::new(MacFrameClock::new(request).unwrap()),
            failure: AtomicU8::new(0),
            started: AtomicBool::new(false),
            captured: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            accepting: AtomicBool::new(true),
            delivered: AtomicU32::new(0),
            stopping: AtomicBool::new(false),
            wake,
        })
    }
    #[test]
    fn owned_session_notifications_fail_terminally_and_filter_the_exact_session() -> Result<()> {
        objc2::rc::autoreleasepool(|_| -> Result<()> {
            let events = EventLoop::notification_test_loop()?;
            let center = NSNotificationCenter::new();
            let session = unsafe { AVCaptureSession::new() };
            let other = unsafe { AVCaptureSession::new() };
            for (name, expected) in unsafe {
                [
                    (AVCaptureSessionWasInterruptedNotification, INTERRUPTED),
                    (AVCaptureSessionRuntimeErrorNotification, SESSION_ERROR),
                    (AVCaptureSessionDidStopRunningNotification, SESSION_STOPPED),
                ]
            } {
                let source = state(events.wake());
                let observations =
                    SessionNotifications::new(center.clone(), &session, source.clone());
                unsafe {
                    center.postNotificationName_object(name, Some(&other));
                }
                assert_eq!(source.failure.load(Ordering::Acquire), 0);
                unsafe {
                    center.postNotificationName_object(name, Some(&session));
                }
                assert_eq!(source.failure.load(Ordering::Acquire), expected);
                assert!(!source.accepting.load(Ordering::Acquire));
                assert!(source.slot.reserve().is_err());
                // An immediately ended interruption cannot recover within PTS slack.
                unsafe {
                    center.postNotificationName_object(
                        objc2_av_foundation::AVCaptureSessionInterruptionEndedNotification,
                        Some(&session),
                    );
                }
                assert_eq!(source.failure.load(Ordering::Acquire), expected);
                assert!(source.check().is_err());
                drop(observations);
            }
            Ok(())
        })
    }
    #[test]
    fn owned_notification_removal_and_expected_stop_cannot_mutate_retired_state() -> Result<()> {
        objc2::rc::autoreleasepool(|_| -> Result<()> {
            let events = EventLoop::notification_test_loop()?;
            let center = NSNotificationCenter::new();
            let session = unsafe { AVCaptureSession::new() };
            let source = state(events.wake());
            let observations = SessionNotifications::new(center.clone(), &session, source.clone());
            source.stopping.store(true, Ordering::Release);
            unsafe {
                center.postNotificationName_object(
                    AVCaptureSessionDidStopRunningNotification,
                    Some(&session),
                );
            }
            assert_eq!(source.failure.load(Ordering::Acquire), 0);
            let active = observations.active.clone();
            drop(observations);
            assert!(!*active.lock().unwrap());
            unsafe {
                center.postNotificationName_object(
                    AVCaptureSessionRuntimeErrorNotification,
                    Some(&session),
                );
            }
            assert_eq!(source.failure.load(Ordering::Acquire), 0);
            assert!(source.accepting.load(Ordering::Acquire));
            Ok(())
        })
    }
}
