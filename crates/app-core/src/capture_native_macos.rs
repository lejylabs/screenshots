use crate::{
    CaptureCursor, CaptureMonitorBinding, CaptureMonitorIdentity, CaptureOptions,
    CaptureWindowShadow, DisplayInfo,
};
use anyhow::{Context, Result, bail, ensure};
use core_foundation::{
    base::TCFType,
    data::{CFData, CFDataRef},
    string::{CFString, CFStringRef},
    uuid::{CFUUID, CFUUIDCreateString, CFUUIDRef},
};
use image::RgbaImage;
use std::{ffi::c_void, ptr::null};

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Size {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    origin: Point,
    size: Size,
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static CGRectNull: Rect;
    static kCGColorSpaceSRGB: CFStringRef;
    fn CGWindowListCreateImage(rect: Rect, list: u32, window: u32, options: u32) -> *const c_void;
    fn CGImageRelease(image: *const c_void);
    fn CGImageGetWidth(image: *const c_void) -> usize;
    fn CGImageGetHeight(image: *const c_void) -> usize;
    fn CGImageGetBitsPerComponent(image: *const c_void) -> usize;
    fn CGImageGetBitsPerPixel(image: *const c_void) -> usize;
    fn CGImageGetBytesPerRow(image: *const c_void) -> usize;
    fn CGImageGetBitmapInfo(image: *const c_void) -> u32;
    fn CGImageGetAlphaInfo(image: *const c_void) -> u32;
    fn CGImageGetDataProvider(image: *const c_void) -> *const c_void;
    fn CGImageGetColorSpace(image: *const c_void) -> *const c_void;
    fn CGColorSpaceCopyName(space: *const c_void) -> CFStringRef;
    fn CGDataProviderCopyData(provider: *const c_void) -> CFDataRef;
}
#[link(name = "ColorSync", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> CFUUIDRef;
}
struct Image(*const c_void);
impl Drop for Image {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CGImageRelease(self.0);
            }
        }
    }
}
pub(super) fn display_identities(displays: &[DisplayInfo]) -> Result<Vec<CaptureMonitorIdentity>> {
    displays
        .iter()
        .map(|display| {
            let id = display
                .id
                .parse::<u32>()
                .context("Invalid CoreGraphics display ID")?;
            let raw = unsafe { CGDisplayCreateUUIDFromDisplayID(id) };
            ensure!(
                !raw.is_null(),
                "Persistent CoreGraphics display UUID is unavailable"
            );
            let uuid = unsafe { CFUUID::wrap_under_create_rule(raw) };
            let raw = unsafe { CFUUIDCreateString(null(), uuid.as_concrete_TypeRef()) };
            ensure!(
                !raw.is_null(),
                "Cannot represent the CoreGraphics display UUID"
            );
            let name = unsafe { CFString::wrap_under_create_rule(raw) }.to_string();
            ensure!(name.len() == 36, "Invalid CoreGraphics display UUID");
            Ok(CaptureMonitorIdentity {
                display_id: display.id.clone(),
                binding: CaptureMonitorBinding::Physical {
                    key: format!("macos:{}", name.to_lowercase()),
                },
            })
        })
        .collect()
}
pub(super) fn validate_options(options: &CaptureOptions, window: bool) -> Result<()> {
    if window {
        super::validate_native_window_policy(options, false, true, true, true)?;
    }
    ensure!(
        options.cursor != CaptureCursor::Include,
        "Native macOS still cursor composition is not implemented"
    );
    Ok(())
}
pub(super) fn validate_template_session() -> Result<()> {
    Ok(())
}
pub(super) fn apply_cursor(_: &mut RgbaImage, _: i32, _: i32) -> Result<()> {
    bail!("Native macOS still cursor composition is not implemented")
}
pub(super) fn capture_window(window: &xcap::Window, options: &CaptureOptions) -> Result<RgbaImage> {
    let geometry = || -> Result<_> {
        Ok((
            window.x()?,
            window.y()?,
            window.width()?,
            window.height()?,
            window.pid()?,
        ))
    };
    let before = geometry()?;
    let flags = if options.window_shadow == CaptureWindowShadow::Exclude {
        1
    } else {
        0
    };
    let image = Image(unsafe { CGWindowListCreateImage(CGRectNull, 1 << 3, window.id()?, flags) });
    ensure!(
        !image.0.is_null(),
        "Native window capture returned no image; check Screen Recording permission"
    );
    let width = u32::try_from(unsafe { CGImageGetWidth(image.0) })?;
    let height = u32::try_from(unsafe { CGImageGetHeight(image.0) })?;
    crate::validate_image_size(width, height)?;
    ensure!(
        unsafe { CGImageGetBitsPerComponent(image.0) } == 8
            && unsafe { CGImageGetBitsPerPixel(image.0) } == 32,
        "Native window pixels are not supported RGBA8; no HDR conversion is implied"
    );
    let bitmap = unsafe { CGImageGetBitmapInfo(image.0) };
    let space = unsafe { CGImageGetColorSpace(image.0) };
    ensure!(!space.is_null(), "Native window color space is unavailable");
    let name = unsafe { CGColorSpaceCopyName(space) };
    ensure!(
        !name.is_null(),
        "Native window color space has no standard name"
    );
    let name = unsafe { CFString::wrap_under_create_rule(name) };
    let srgb = unsafe { CFString::wrap_under_get_rule(kCGColorSpaceSRGB) };
    ensure!(
        name == srgb,
        "Native window pixels are not marked sRGB; color-managed capture is not implemented"
    );
    ensure!(
        bitmap & (1 << 8) == 0,
        "Floating point native window pixels are unsupported"
    );
    let alpha = unsafe { CGImageGetAlphaInfo(image.0) };
    let order = bitmap & 0x7000;
    ensure!(
        matches!(order, 0x2000 | 0x4000) && matches!(alpha, 1..=6),
        "Unsupported native window component order/alpha encoding"
    );
    let stride = unsafe { CGImageGetBytesPerRow(image.0) };
    ensure!(
        stride >= width as usize * 4
            && stride
                .checked_mul(height as usize)
                .is_some_and(|bytes| bytes <= crate::MAX_CAPTURE_BYTES as usize + 32 * 1024 * 1024),
        "Invalid or oversized native window stride"
    );
    let provider = unsafe { CGImageGetDataProvider(image.0) };
    ensure!(
        !provider.is_null(),
        "Native window data provider is unavailable"
    );
    let data = unsafe { CGDataProviderCopyData(provider) };
    ensure!(!data.is_null(), "Cannot read native window image data");
    let data = unsafe { CFData::wrap_under_create_rule(data) };
    let bytes = data.bytes();
    ensure!(
        stride
            .checked_mul(height as usize)
            .is_some_and(|length| length <= bytes.len())
            && bytes.len() <= crate::MAX_CAPTURE_BYTES as usize + 32 * 1024 * 1024,
        "Invalid native window data extent"
    );
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for row in bytes.chunks(stride).take(height as usize) {
        for pixel in row[..width as usize * 4].chunks_exact(4) {
            let (mut rgb, a) = match (order, alpha) {
                (0x2000, 2 | 4 | 6) => ([pixel[2], pixel[1], pixel[0]], pixel[3]),
                (0x4000, 2 | 4 | 6) => ([pixel[1], pixel[2], pixel[3]], pixel[0]),
                (0x2000, 1 | 3 | 5) => ([pixel[3], pixel[2], pixel[1]], pixel[0]),
                (0x4000, 1 | 3 | 5) => ([pixel[0], pixel[1], pixel[2]], pixel[3]),
                _ => unreachable!(),
            };
            let a = if matches!(alpha, 5 | 6) { 255 } else { a };
            if matches!(alpha, 1 | 2) {
                for channel in &mut rgb {
                    *channel = if a == 0 {
                        0
                    } else {
                        ((u32::from(*channel) * 255 + u32::from(a) / 2) / u32::from(a)).min(255)
                            as u8
                    };
                }
            }
            pixels.extend_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
        }
    }
    ensure!(
        geometry()? == before,
        "Native window moved, resized or changed process during capture"
    );
    let mut result = RgbaImage::from_raw(width, height, pixels)
        .context("Invalid native window pixel backing")?;
    options.flatten_window(&mut result);
    Ok(result)
}
