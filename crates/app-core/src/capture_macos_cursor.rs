//! Public legacy system cursor, only for macOS versions without native screenshot embedding.
use crate::{
    MacLogicalRect,
    capture_macos_ax::{self, Point},
    capture_options::CaptureCursorImage,
};
use anyhow::{Context, Result, ensure};
use core_foundation::base::CFRelease;
use objc2_app_kit::NSCursor;
use std::{ffi::c_void, ptr};
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGCursorIsVisible() -> bool;
    fn CGEventCreate(source: *const c_void) -> *const c_void;
    fn CGEventGetLocation(event: *const c_void) -> Point;
}
struct Event(*const c_void);
impl Drop for Event {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}
pub(super) fn compose(
    image: &mut image::RgbaImage,
    origin: MacLogicalRect,
    scale: f64,
) -> Result<()> {
    capture_macos_ax::main_thread()?;
    origin.validate()?;
    ensure!(
        scale.is_finite() && scale > 0.0 && scale <= 8.0,
        "Invalid native cursor scale"
    );
    if !unsafe { CGCursorIsVisible() } {
        return Ok(());
    }
    #[allow(deprecated)]
    let cursor = NSCursor::currentSystemCursor().context(
        "Public legacy system cursor is unavailable; native compositor cursor capture is required",
    )?;
    let source = cursor.image();
    let size = source.size();
    let hotspot = cursor.hotSpot();
    let integral = |value: f64| -> Result<u32> {
        let value = value * scale;
        ensure!(
            value.is_finite()
                && value >= 1.0
                && value <= 1024.0
                && (value - value.round()).abs() <= 1.0 / 1024.0,
            "Native cursor image extent is invalid"
        );
        Ok(value.round() as u32)
    };
    let (width, height) = (integral(size.width)?, integral(size.height)?);
    let native =
        unsafe { source.CGImageForProposedRect_context_hints(ptr::null_mut(), None, None) }
            .context("Public cursor image has no native bitmap")?;
    let pixels = unsafe {
        crate::capture_macos_image::cursor_pixels(
            (&*native as *const objc2_core_graphics::CGImage).cast(),
            width,
            height,
        )
    }?;
    let event = Event(unsafe { CGEventCreate(ptr::null()) });
    ensure!(!event.0.is_null(), "Native pointer position is unavailable");
    let position = unsafe { CGEventGetLocation(event.0) };
    let coordinate = |value: f64| -> Result<i32> {
        ensure!(
            value.is_finite() && value >= i32::MIN as f64 && value <= i32::MAX as f64,
            "Native cursor position exceeds its bound"
        );
        Ok(value.round() as i32)
    };
    let hot = |value: f64, extent: u32| -> Result<u32> {
        let value = value * scale;
        ensure!(
            value.is_finite() && value >= 0.0 && value.round() < f64::from(extent),
            "Native cursor hotspot is invalid"
        );
        Ok(value.round() as u32)
    };
    let cursor = CaptureCursorImage {
        pixels,
        screen_x: coordinate((position.x - origin.x) * scale)?,
        screen_y: coordinate((position.y - origin.y) * scale)?,
        hotspot_x: hot(hotspot.x, width)?,
        hotspot_y: hot(hotspot.y, height)?,
    };
    cursor.compose(image, 0, 0)
}
