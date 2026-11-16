//! Public, operation-owned AX control. Never posts global mouse/scroll events.
use crate::{
    MAC_AX_ARRAY_LIMIT, MAC_AX_MESSAGE_MILLIS, MAC_AX_PARENT_LIMIT, MacDisplayProof,
    MacLogicalRect, MacWindowProof, PixelRegion,
};
use anyhow::{Context, Result, ensure};
use core_foundation::{
    array::{CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex, CFArrayRef},
    base::{CFEqual, CFGetTypeID, CFRelease, CFRetain, CFTypeID, CFTypeRef, TCFType},
    boolean::{CFBoolean, CFBooleanGetTypeID},
    dictionary::{CFDictionaryGetTypeID, CFDictionaryGetValue, CFDictionaryRef},
    number::{
        CFNumberGetTypeID, CFNumberGetValue, CFNumberRef, kCFNumberDoubleType, kCFNumberSInt64Type,
    },
    string::{CFString, CFStringGetTypeID, CFStringRef},
    uuid::{CFUUID, CFUUIDCreateString, CFUUIDRef},
};
use std::{ffi::c_void, ptr};
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct Point {
    pub x: f64,
    pub y: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct Size {
    pub width: f64,
    pub height: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct Rect {
    pub origin: Point,
    pub size: Size,
}
impl From<Rect> for MacLogicalRect {
    fn from(rect: Rect) -> Self {
        Self {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.width,
            height: rect.size.height,
        }
    }
}
impl From<MacLogicalRect> for Rect {
    fn from(rect: MacLogicalRect) -> Self {
        Self {
            origin: Point {
                x: rect.x,
                y: rect.y,
            },
            size: Size {
                width: rect.width,
                height: rect.height,
            },
        }
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXUIElementGetTypeID() -> CFTypeID;
    fn AXUIElementCreateSystemWide() -> CFTypeRef;
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        name: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementCopyActionNames(element: CFTypeRef, names: *mut CFArrayRef) -> i32;
    fn AXUIElementCopyElementAtPosition(
        element: CFTypeRef,
        x: f32,
        y: f32,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementGetPid(element: CFTypeRef, pid: *mut i32) -> i32;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> i32;
    fn AXUIElementPerformAction(element: CFTypeRef, action: CFStringRef) -> i32;
    fn AXUIElementIsAttributeSettable(
        element: CFTypeRef,
        name: CFStringRef,
        settable: *mut u8,
    ) -> i32;
    fn AXUIElementSetAttributeValue(element: CFTypeRef, name: CFStringRef, value: CFTypeRef)
    -> i32;
    fn AXValueGetTypeID() -> CFTypeID;
    fn AXValueGetValue(value: CFTypeRef, type_: u32, output: *mut c_void) -> u8;
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGWindowListCopyWindowInfo(options: u32, relative: u32) -> CFArrayRef;
    fn CGRectMakeWithDictionaryRepresentation(dictionary: CFDictionaryRef, rect: *mut Rect) -> u8;
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayBounds(display: u32) -> Rect;
    fn CGDisplayPixelsWide(display: u32) -> usize;
    fn CGDisplayPixelsHigh(display: u32) -> usize;
    fn CGDisplayCopyDisplayMode(display: u32) -> CFTypeRef;
    fn CGDisplayModeGetPixelWidth(mode: CFTypeRef) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: CFTypeRef) -> usize;
    fn CGDisplayRotation(display: u32) -> f64;
}
#[link(name = "ColorSync", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> CFUUIDRef;
}
#[link(name = "proc")]
unsafe extern "C" {
    fn proc_pidinfo(pid: i32, flavor: i32, argument: u64, buffer: *mut c_void, length: i32) -> i32;
}

pub(super) struct Cf(pub CFTypeRef);
impl Cf {
    unsafe fn owned(value: CFTypeRef) -> Result<Self> {
        ensure!(!value.is_null(), "Native macOS value is unavailable");
        Ok(Self(value))
    }
    unsafe fn borrowed(value: CFTypeRef) -> Result<Self> {
        ensure!(!value.is_null(), "Native macOS value is unavailable");
        unsafe { CFRetain(value) };
        Ok(Self(value))
    }
}
impl Drop for Cf {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}
impl Clone for Cf {
    fn clone(&self) -> Self {
        unsafe { CFRetain(self.0) };
        Self(self.0)
    }
}
fn key(name: &str) -> CFString {
    CFString::new(name)
}
fn string(value: CFTypeRef) -> Result<String> {
    ensure!(
        !value.is_null() && unsafe { CFGetTypeID(value) } == unsafe { CFStringGetTypeID() },
        "Native macOS value is not a string"
    );
    let value = unsafe { CFString::wrap_under_get_rule(value.cast()) };
    ensure!(
        value.char_len() <= 128,
        "Native macOS string exceeds its bound"
    );
    Ok(value.to_string())
}
fn number(value: CFTypeRef) -> Result<f64> {
    ensure!(
        !value.is_null() && unsafe { CFGetTypeID(value) } == unsafe { CFNumberGetTypeID() },
        "Native macOS value is not numeric"
    );
    let mut output = 0f64;
    ensure!(
        unsafe {
            CFNumberGetValue(
                value as CFNumberRef,
                kCFNumberDoubleType,
                (&mut output as *mut f64).cast(),
            )
        } && output.is_finite(),
        "Invalid native macOS number"
    );
    Ok(output)
}
fn integer(value: CFTypeRef) -> Result<i64> {
    ensure!(
        !value.is_null() && unsafe { CFGetTypeID(value) } == unsafe { CFNumberGetTypeID() },
        "Native macOS value is not an integer"
    );
    let mut output = 0i64;
    ensure!(
        unsafe {
            CFNumberGetValue(
                value as CFNumberRef,
                kCFNumberSInt64Type,
                (&mut output as *mut i64).cast(),
            )
        },
        "Invalid native macOS integer"
    );
    Ok(output)
}
fn item(dictionary: CFTypeRef, name: &str) -> Result<CFTypeRef> {
    ensure!(
        !dictionary.is_null()
            && unsafe { CFGetTypeID(dictionary) } == unsafe { CFDictionaryGetTypeID() },
        "Native macOS value is not a dictionary"
    );
    let name = key(name);
    let value =
        unsafe { CFDictionaryGetValue(dictionary.cast(), name.as_concrete_TypeRef().cast()) };
    ensure!(!value.is_null(), "Native window metadata is incomplete");
    Ok(value)
}
fn element(value: Cf) -> Result<Cf> {
    ensure!(
        unsafe { CFGetTypeID(value.0) } == unsafe { AXUIElementGetTypeID() },
        "Native AX value is not an element"
    );
    ensure!(
        unsafe { AXUIElementSetMessagingTimeout(value.0, MAC_AX_MESSAGE_MILLIS as f32 / 1000.0) }
            == 0,
        "Cannot bound AX target messaging"
    );
    Ok(value)
}
fn attribute(element: &Cf, name: &str) -> Result<Cf> {
    let name = key(name);
    let mut output = ptr::null();
    ensure!(
        unsafe {
            AXUIElementCopyAttributeValue(element.0, name.as_concrete_TypeRef(), &mut output)
        } == 0,
        "AX attribute is unavailable or timed out"
    );
    unsafe { Cf::owned(output) }
}
fn pid(element: &Cf) -> Result<u32> {
    let mut value = 0i32;
    ensure!(
        unsafe { AXUIElementGetPid(element.0, &mut value) } == 0 && value > 0,
        "AX owner process is unavailable"
    );
    Ok(value as u32)
}
fn ax_bounds(element: &Cf) -> Result<MacLogicalRect> {
    let position = attribute(element, "AXPosition")?;
    let size = attribute(element, "AXSize")?;
    ensure!(
        unsafe { CFGetTypeID(position.0) } == unsafe { AXValueGetTypeID() }
            && unsafe { CFGetTypeID(size.0) } == unsafe { AXValueGetTypeID() },
        "AX geometry has an invalid type"
    );
    let mut p = Point::default();
    let mut s = Size::default();
    ensure!(
        unsafe { AXValueGetValue(position.0, 1, (&mut p as *mut Point).cast()) } != 0
            && unsafe { AXValueGetValue(size.0, 2, (&mut s as *mut Size).cast()) } != 0,
        "AX geometry is unavailable"
    );
    let rect = MacLogicalRect {
        x: p.x,
        y: p.y,
        width: s.width,
        height: s.height,
    };
    rect.validate()?;
    Ok(rect)
}

pub(super) fn main_thread() -> Result<()> {
    ensure!(
        unsafe { libc::pthread_main_np() } != 0,
        "macOS native operation requires its helper main thread"
    );
    Ok(())
}
pub(super) fn screen_permission() -> Result<()> {
    main_thread()?;
    if !unsafe { CGPreflightScreenCaptureAccess() } {
        return Err(crate::CaptureError::MacScreenPermissionDenied.into());
    }
    Ok(())
}
pub(super) fn process_start(pid: u32) -> Result<(u64, u32)> {
    ensure!(
        pid > 0 && pid <= i32::MAX as u32,
        "Invalid native process identity"
    );
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let length = std::mem::size_of_val(&info);
    ensure!(
        unsafe {
            proc_pidinfo(
                pid as i32,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                length as i32,
            )
        } == length as i32,
        "Native process identity changed or disappeared"
    );
    Ok((info.pbi_start_tvsec, u32::try_from(info.pbi_start_tvusec)?))
}
pub(super) fn windows() -> Result<Vec<MacWindowProof>> {
    screen_permission()?;
    let raw = unsafe { CGWindowListCopyWindowInfo(1 | 16, 0) };
    let list = unsafe { Cf::owned(raw.cast()) }?;
    ensure!(
        unsafe { CFGetTypeID(list.0) } == unsafe { CFArrayGetTypeID() },
        "Native window list has an invalid type"
    );
    let count = unsafe { CFArrayGetCount(raw) };
    ensure!(
        (0..=1024).contains(&count),
        "Native window inventory exceeds its bound"
    );
    let mut output = Vec::with_capacity(count as usize);
    for index in 0..count {
        let dictionary = unsafe { CFArrayGetValueAtIndex(raw, index) };
        let id = u32::try_from(integer(item(dictionary, "kCGWindowNumber")?)?)?;
        let pid = u32::try_from(integer(item(dictionary, "kCGWindowOwnerPID")?)?)?;
        let layer = i32::try_from(integer(item(dictionary, "kCGWindowLayer")?)?)?;
        if pid == 0 {
            continue;
        }
        let visible = item(dictionary, "kCGWindowIsOnscreen")?;
        ensure!(
            unsafe { CFGetTypeID(visible) } == unsafe { CFBooleanGetTypeID() },
            "Native window visibility is invalid"
        );
        let mut rect = Rect::default();
        ensure!(
            unsafe {
                CGRectMakeWithDictionaryRepresentation(
                    item(dictionary, "kCGWindowBounds")?.cast(),
                    &mut rect,
                )
            } != 0,
            "Native window bounds are invalid"
        );
        let Ok((seconds, micros)) = process_start(pid) else {
            continue;
        };
        output.push(MacWindowProof {
            id,
            pid,
            process_start_seconds: seconds,
            process_start_micros: micros,
            bounds: rect.into(),
            onscreen: bool::from(unsafe { CFBoolean::wrap_under_get_rule(visible.cast()) }),
            layer,
        });
    }
    Ok(output)
}
pub(super) fn window(id: u32) -> Result<MacWindowProof> {
    windows()?
        .into_iter()
        .find(|window| window.id == id)
        .context("Selected native window is unavailable")
}
pub(super) fn displays() -> Result<Vec<MacDisplayProof>> {
    main_thread()?;
    let mut ids = [0u32; 32];
    let mut expected = 0u32;
    ensure!(
        unsafe { CGGetActiveDisplayList(0, ptr::null_mut(), &mut expected) } == 0
            && expected > 0
            && expected <= ids.len() as u32,
        "Native display inventory is unavailable/oversized"
    );
    let mut count = 0u32;
    ensure!(
        unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) } == 0
            && count == expected,
        "Native display inventory is unavailable/oversized"
    );
    ids[..count as usize]
        .iter()
        .map(|id| {
            let uuid = unsafe { CGDisplayCreateUUIDFromDisplayID(*id) };
            ensure!(!uuid.is_null(), "Native display UUID unavailable");
            let uuid = unsafe { CFUUID::wrap_under_create_rule(uuid) };
            let name = unsafe { CFUUIDCreateString(ptr::null(), uuid.as_concrete_TypeRef()) };
            ensure!(!name.is_null(), "Native display UUID cannot be read");
            let name = unsafe { CFString::wrap_under_create_rule(name) }
                .to_string()
                .to_ascii_lowercase();
            let mode = unsafe { Cf::owned(CGDisplayCopyDisplayMode(*id)) }?;
            let proof = MacDisplayProof {
                id: *id,
                uuid: name,
                logical: unsafe { CGDisplayBounds(*id) }.into(),
                pixel_width: u32::try_from(unsafe { CGDisplayModeGetPixelWidth(mode.0) })?,
                pixel_height: u32::try_from(unsafe { CGDisplayModeGetPixelHeight(mode.0) })?,
                rotation: unsafe { CGDisplayRotation(*id) },
            };
            proof.validate()?;
            Ok(proof)
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Increment {
    Action,
    Value {
        increment: f64,
        minimum: f64,
        maximum: f64,
    },
}
pub(super) struct Target {
    pub proof: MacWindowProof,
    pub display: MacDisplayProof,
    pub scale: f64,
    application: Cf,
    focused: Cf,
    control: Cf,
    control_bounds: MacLogicalRect,
    increment: Increment,
}
impl Target {
    pub fn new(id: u32, region: PixelRegion, origin: u32) -> Result<Self> {
        screen_permission()?;
        if unsafe { AXIsProcessTrusted() } == 0 {
            return Err(crate::CaptureError::MacAccessibilityPermissionDenied.into());
        }
        let proof = window(id)?;
        proof.validate(origin, std::process::id())?;
        let mut matching = displays()?
            .into_iter()
            .filter(|display| display.logical.contains(proof.bounds));
        let display = matching
            .next()
            .context("Scrolling window must fit one native display")?;
        ensure!(
            matching.next().is_none(),
            "Scrolling display binding is ambiguous"
        );
        let scale = display.unrotated_scale()?;
        let (width, height) = proof.bounds.pixel_size(scale)?;
        ensure!(
            u64::from(width) * u64::from(height) * 4 <= 32 * 1024 * 1024,
            "Native scrolling window exceeds its32MiB source limit"
        );
        let application =
            element(unsafe { Cf::owned(AXUIElementCreateApplication(proof.pid as i32)) }?)?;
        let focused = element(attribute(&application, "AXFocusedWindow")?)?;
        let (x, y) = proof.bounds.region_center(scale, region)?;
        ensure!(
            (x - f64::from(x as f32)).abs() <= 1.0 / 1024.0
                && (y - f64::from(y as f32)).abs() <= 1.0 / 1024.0,
            "AX hit-test coordinates lose native precision"
        );
        let mut raw = ptr::null();
        ensure!(
            unsafe {
                AXUIElementCopyElementAtPosition(application.0, x as f32, y as f32, &mut raw)
            } == 0,
            "AX target hit test failed"
        );
        let mut hit = element(unsafe { Cf::owned(raw) }?)?;
        let mut selected = None;
        for _ in 0..MAC_AX_PARENT_LIMIT {
            ensure!(
                pid(&hit)? == proof.pid,
                "AX target belongs to another process"
            );
            if let Ok(bar) = attribute(&hit, "AXVerticalScrollBar").and_then(element) {
                selected = Some(bar);
                break;
            }
            if unsafe { CFEqual(hit.0, focused.0) } != 0 {
                break;
            }
            hit = element(attribute(&hit, "AXParent")?)?;
        }
        let control = selected.context("Selected window exposes no targeted vertical scrollbar")?;
        ensure!(
            pid(&control)? == proof.pid
                && string(attribute(&control, "AXRole")?.0)? == "AXScrollBar"
                && string(attribute(&control, "AXOrientation")?.0)? == "AXVerticalOrientation",
            "AX target is not an owned vertical scrollbar"
        );
        let control_bounds = ax_bounds(&control)?;
        let mut names = ptr::null();
        ensure!(
            unsafe { AXUIElementCopyActionNames(control.0, &mut names) } == 0,
            "AX control actions are unavailable"
        );
        let actions = unsafe { Cf::owned(names.cast()) }?;
        ensure!(
            unsafe { CFGetTypeID(actions.0) } == unsafe { CFArrayGetTypeID() },
            "AX actions have an invalid type"
        );
        let count = unsafe { CFArrayGetCount(names) };
        ensure!(
            count >= 0 && count as usize <= MAC_AX_ARRAY_LIMIT,
            "AX action inventory exceeds its bound"
        );
        let has_increment = (0..count).any(|index| {
            string(unsafe { CFArrayGetValueAtIndex(names, index) })
                .is_ok_and(|name| name == "AXIncrement")
        });
        let increment = if has_increment {
            Increment::Action
        } else {
            let mut settable = 0u8;
            let name = key("AXValue");
            ensure!(
                unsafe {
                    AXUIElementIsAttributeSettable(
                        control.0,
                        name.as_concrete_TypeRef(),
                        &mut settable,
                    )
                } == 0
                    && settable != 0,
                "AX scrollbar has no documented increment/value operation"
            );
            let increment = number(attribute(&control, "AXValueIncrement")?.0)?;
            let minimum = number(attribute(&control, "AXMinValue")?.0)?;
            let maximum = number(attribute(&control, "AXMaxValue")?.0)?;
            ensure!(
                minimum < maximum && increment > 0.0 && increment <= maximum - minimum,
                "Invalid native scrollbar increment/range"
            );
            Increment::Value {
                increment,
                minimum,
                maximum,
            }
        };
        let target = Self {
            proof,
            display,
            scale,
            application,
            focused,
            control,
            control_bounds,
            increment,
        };
        target.check()?;
        Ok(target)
    }
    pub fn check(&self) -> Result<()> {
        let snapshot = windows()?;
        let current = snapshot
            .iter()
            .find(|window| window.id == self.proof.id)
            .copied()
            .context("Scrolling target disappeared")?;
        self.proof.verify_unchanged(current)?;
        let system = element(unsafe { Cf::owned(AXUIElementCreateSystemWide()) }?)?;
        let front = element(attribute(&system, "AXFocusedApplication")?)?;
        ensure!(
            pid(&front)? == self.proof.pid,
            "Scrolling target lost process focus"
        );
        let focused = element(attribute(&self.application, "AXFocusedWindow")?)?;
        ensure!(
            unsafe { CFEqual(focused.0, self.focused.0) } != 0,
            "Scrolling target lost exact window focus"
        );
        let bounds = ax_bounds(&focused)?;
        let matches = snapshot
            .iter()
            .filter(|window| window.pid == self.proof.pid && window.bounds.same_bounds(bounds))
            .count();
        self.proof
            .matches_focused_window(pid(&focused)?, bounds, matches)?;
        ensure!(
            pid(&self.control)? == self.proof.pid
                && self.control_bounds.same_bounds(ax_bounds(&self.control)?),
            "Targeted scrollbar geometry/process changed"
        );
        let mut parent = self.control.clone();
        let mut owned = false;
        for _ in 0..MAC_AX_PARENT_LIMIT {
            if unsafe { CFEqual(parent.0, self.focused.0) } != 0 {
                owned = true;
                break;
            }
            parent = element(attribute(&parent, "AXParent")?)?;
        }
        ensure!(
            owned,
            "Targeted scrollbar no longer belongs to the retained window"
        );
        let displays = displays()?;
        let current = displays
            .iter()
            .find(|display| display.id == self.display.id)
            .context("Scrolling display disappeared")?;
        ensure!(
            current == &self.display,
            "Scrolling display UUID/topology changed"
        );
        Ok(())
    }
    pub fn increment(&self, count: u32, check: &impl Fn() -> Result<()>) -> Result<()> {
        ensure!((1..=10).contains(&count), "Invalid bounded AX scroll count");
        for _ in 0..count {
            check()?;
            self.check()?;
            match self.increment {
                Increment::Action => {
                    let name = key("AXIncrement");
                    ensure!(
                        unsafe {
                            AXUIElementPerformAction(self.control.0, name.as_concrete_TypeRef())
                        } == 0,
                        "Targeted AX scrollbar increment failed"
                    );
                }
                Increment::Value {
                    increment,
                    minimum,
                    maximum,
                } => {
                    let old = number(attribute(&self.control, "AXValue")?.0)?;
                    ensure!(
                        old >= minimum && old <= maximum,
                        "AX scrollbar value escaped its range"
                    );
                    let next = (old + increment).min(maximum);
                    let next = core_foundation::number::CFNumber::from(next);
                    let name = key("AXValue");
                    ensure!(
                        unsafe {
                            AXUIElementSetAttributeValue(
                                self.control.0,
                                name.as_concrete_TypeRef(),
                                next.as_CFTypeRef(),
                            )
                        } == 0,
                        "Targeted AX scrollbar value write failed"
                    );
                }
            }
            check()?;
            self.check()?;
        }
        Ok(())
    }
}
