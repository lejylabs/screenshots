//! Bounded native CGImage→straight sRGB RGBA. No file/PNG handoff.
use crate::capture_macos_ax::{Point, Rect, Size};
use crate::{CaptureCursor, CaptureOptions, CaptureWindowShadow, MacWindowProof};
use anyhow::{Result, ensure};
use core_foundation::{
    base::TCFType,
    data::{CFData, CFDataRef},
    string::{CFString, CFStringRef},
};
use std::time::{Duration, Instant};
use std::{ffi::c_void, ptr};
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: CFStringRef;
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
    fn CGColorSpaceCreateWithName(name: CFStringRef) -> *const c_void;
    fn CGColorSpaceRelease(space: *const c_void);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits: usize,
        stride: usize,
        space: *const c_void,
        info: u32,
    ) -> *mut c_void;
    fn CGContextDrawImage(context: *mut c_void, rect: Rect, image: *const c_void);
    fn CGContextRelease(context: *mut c_void);
    static CGRectNull: Rect;
    fn CGWindowListCreateImage(
        rect: Rect,
        options: u32,
        window: u32,
        image_options: u32,
    ) -> *const c_void;
}
pub(super) fn legacy_window(
    proof: MacWindowProof,
    options: &CaptureOptions,
) -> Result<image::RgbaImage> {
    crate::capture_macos_ax::screen_permission()?;
    ensure!(
        options.cursor != CaptureCursor::Include,
        "This macOS version needs its native compositor cursor API for inclusion"
    );
    proof.verify_unchanged(crate::capture_macos_ax::window(proof.id)?)?;
    let flags = 8 | if options.window_shadow == CaptureWindowShadow::Include {
        0
    } else {
        1
    };
    let raw = unsafe { CGWindowListCreateImage(CGRectNull, 1 << 3, proof.id, flags) };
    ensure!(
        !raw.is_null(),
        "Native legacy window capture is unavailable; check supported macOS API/Screen Recording permission"
    );
    let image = crate::capture_macos_sck::Image(raw);
    let pixels = unsafe { decode(image.0) }?;
    proof.verify_unchanged(crate::capture_macos_ax::window(proof.id)?)?;
    Ok(pixels)
}
pub(super) fn window(proof: MacWindowProof, options: &CaptureOptions) -> Result<image::RgbaImage> {
    let until = Instant::now() + Duration::from_secs(20);
    let mut pixels = if let Some(source) = crate::capture_macos_sck::Window::new(proof, until)? {
        let image = source.capture(options, until)?;
        unsafe { decode(image.0) }?
    } else {
        legacy_window(proof, options)?
    };
    options.flatten_window(&mut pixels);
    Ok(pixels)
}
struct Space(*const c_void);
impl Drop for Space {
    fn drop(&mut self) {
        unsafe { CGColorSpaceRelease(self.0) }
    }
}
struct Context(*mut c_void);
impl Drop for Context {
    fn drop(&mut self) {
        unsafe { CGContextRelease(self.0) }
    }
}
/// The caller retains the immutable native image for this complete synchronous read.
pub(super) unsafe fn decode(image: *const c_void) -> Result<image::RgbaImage> {
    ensure!(!image.is_null(), "Native image is unavailable");
    let width = u32::try_from(unsafe { CGImageGetWidth(image) })?;
    let height = u32::try_from(unsafe { CGImageGetHeight(image) })?;
    crate::validate_image_size(width, height)?;
    let bitmap = unsafe { CGImageGetBitmapInfo(image) };
    let alpha = unsafe { CGImageGetAlphaInfo(image) };
    ensure!(
        unsafe { CGImageGetBitsPerComponent(image) } == 8
            && unsafe { CGImageGetBitsPerPixel(image) } == 32
            && bitmap & (1 << 8) == 0
            && alpha <= 6,
        "Native screenshot is not supported SDR RGB8; HDR/float conversion is not implied"
    );
    let stride = unsafe { CGImageGetBytesPerRow(image) };
    ensure!(
        stride >= width as usize * 4
            && stride
                .checked_mul(height as usize)
                .is_some_and(|bytes| bytes <= crate::MAX_CAPTURE_BYTES as usize + 32 * 1024 * 1024),
        "Invalid or oversized native image stride"
    );
    let space = unsafe { CGImageGetColorSpace(image) };
    ensure!(!space.is_null(), "Native image has no color space");
    let name = unsafe { CGColorSpaceCopyName(space) };
    let srgb = if name.is_null() {
        false
    } else {
        let name = unsafe { CFString::wrap_under_create_rule(name) };
        let srgb = unsafe { CFString::wrap_under_get_rule(kCGColorSpaceSRGB) };
        name == srgb
    };
    let order = bitmap & 0x7000;
    if srgb && matches!(order, 0x2000 | 0x4000) && matches!(alpha, 1..=6) {
        let provider = unsafe { CGImageGetDataProvider(image) };
        ensure!(!provider.is_null(), "Native image provider is unavailable");
        let data = unsafe { CGDataProviderCopyData(provider) };
        ensure!(!data.is_null(), "Native image bytes are unavailable");
        let data = unsafe { CFData::wrap_under_create_rule(data) };
        return crate::capture_macos_pixels::Layout {
            width,
            height,
            stride,
            order,
            alpha,
        }
        .decode(data.bytes());
    }
    // Let public ColorSync/CoreGraphics convert a tagged SDR P3/device space and
    // documented default component order; never reinterpret its bytes as sRGB.
    let space = Space(unsafe { CGColorSpaceCreateWithName(kCGColorSpaceSRGB) });
    ensure!(!space.0.is_null(), "Cannot create native sRGB color space");
    let mut pixels = image::RgbaImage::new(width, height);
    let target: &mut [u8] = pixels.as_mut();
    let context = Context(unsafe {
        CGBitmapContextCreate(
            target.as_mut_ptr().cast(),
            width as usize,
            height as usize,
            8,
            width as usize * 4,
            space.0,
            0x4000 | 1,
        )
    });
    ensure!(
        !context.0.is_null(),
        "Cannot create bounded native sRGB bitmap context"
    );
    unsafe {
        CGContextDrawImage(
            context.0,
            Rect {
                origin: Point { x: 0.0, y: 0.0 },
                size: Size {
                    width: f64::from(width),
                    height: f64::from(height),
                },
            },
            image,
        )
    };
    drop(context);
    for pixel in target.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = if alpha == 0 {
                0
            } else {
                ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8
            };
        }
    }
    Ok(pixels)
}
