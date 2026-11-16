//! Bounded native CGImage→straight sRGB RGBA. No file/PNG handoff.
use crate::capture_macos_ax::{Point, Rect, Size};
use crate::{CaptureCursor, CaptureOptions, CaptureWindowShadow, MacWindowProof};
use anyhow::{Context, Result, ensure};
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
    #[cfg(test)]
    static kCGColorSpaceDisplayP3: CFStringRef;
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
    fn CGColorSpaceGetModel(space: *const c_void) -> i32;
    fn CGColorSpaceGetNumberOfComponents(space: *const c_void) -> usize;
    fn CGColorSpaceCopyICCData(space: *const c_void) -> CFDataRef;
    fn CGColorSpaceIsHDR(space: *const c_void) -> bool;
    fn CGDataProviderCopyData(provider: *const c_void) -> CFDataRef;
    fn CGColorSpaceCreateWithName(name: CFStringRef) -> *const c_void;
    #[cfg(test)]
    fn CGColorSpaceCreateDeviceRGB() -> *const c_void;
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
    fn CGDataProviderCreateWithData(
        info: *mut c_void,
        data: *const c_void,
        size: usize,
        release: Option<extern "C" fn(*mut c_void, *const c_void, usize)>,
    ) -> *const c_void;
    fn CGDataProviderRelease(provider: *const c_void);
    fn CGImageCreate(
        width: usize,
        height: usize,
        bpc: usize,
        bpp: usize,
        stride: usize,
        space: *const c_void,
        info: u32,
        provider: *const c_void,
        decode: *const f64,
        interpolate: bool,
        intent: u32,
    ) -> *const c_void;
    static CGRectNull: Rect;
    fn CGWindowListCreateImage(
        rect: Rect,
        options: u32,
        window: u32,
        image_options: u32,
    ) -> *const c_void;
    fn CGDisplayCreateImage(display: u32) -> *const c_void;
}
pub(super) fn display(
    id: u32,
    region: Option<crate::PixelRegion>,
    options: &CaptureOptions,
) -> Result<image::RgbaImage> {
    crate::capture_macos_ax::screen_permission()?;
    let until = Instant::now() + Duration::from_secs(20);
    let proof = crate::capture_macos_ax::displays()?
        .into_iter()
        .find(|display| display.id == id)
        .context("Native display is unavailable")?;
    if let Some(region) = region {
        proof.screen_input_crop(region)?;
    }
    let selected_bytes = region.map_or_else(
        || u64::from(proof.pixel_width) * u64::from(proof.pixel_height) * 4,
        |region| u64::from(region.width) * u64::from(region.height) * 4,
    );
    crate::capture_macos_model::screenshot_buffer_admission(
        proof.pixel_width,
        proof.pixel_height,
        selected_bytes,
    )?;
    let mut legacy = false;
    let mut pixels = if let Some(source) = crate::capture_macos_sck::Display::new(id, until)? {
        ensure!(
            source.proof == proof,
            "Display identity changed before capture"
        );
        let image = source.capture(options, until, selected_bytes)?;
        unsafe { decode(image.0) }?
    } else {
        legacy = true;
        let image = crate::capture_macos_sck::Image(unsafe { CGDisplayCreateImage(id) });
        ensure!(!image.0.is_null(), "Native display image is unavailable");
        unsafe { decode(image.0) }?
    };
    ensure!(
        pixels.dimensions() == (proof.pixel_width, proof.pixel_height),
        "Native display pixel extent changed"
    );
    if legacy && options.cursor == CaptureCursor::Include {
        crate::capture_macos_cursor::compose(&mut pixels, proof.logical, proof.unrotated_scale()?)?;
    }
    let current = crate::capture_macos_ax::displays()?
        .into_iter()
        .find(|display| display.id == id)
        .context("Native display disappeared")?;
    ensure!(
        current == proof,
        "Native display UUID/geometry changed during capture"
    );
    if let Some(region) = region {
        Ok(
            image::imageops::crop_imm(&pixels, region.x, region.y, region.width, region.height)
                .to_image(),
        )
    } else {
        Ok(pixels)
    }
}
pub(super) fn legacy_window(
    proof: MacWindowProof,
    options: &CaptureOptions,
) -> Result<image::RgbaImage> {
    crate::capture_macos_ax::screen_permission()?;
    ensure!(
        options.cursor != CaptureCursor::Include
            || options.window_shadow != CaptureWindowShadow::Include,
        "Legacy shadow extent has no public cursor origin; exclude shadows or use native compositor capture"
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
    let mut pixels = unsafe { decode(image.0) }?;
    if options.cursor == CaptureCursor::Include {
        let mut displays = crate::capture_macos_ax::displays()?
            .into_iter()
            .filter(|display| display.logical.contains(proof.bounds));
        let display = displays
            .next()
            .context("Legacy cursor window must fit a native display")?;
        ensure!(
            displays.next().is_none(),
            "Legacy cursor display binding is ambiguous"
        );
        let scale = display.unrotated_scale()?;
        ensure!(
            proof.bounds.pixel_size(scale)? == pixels.dimensions(),
            "Legacy cursor/window pixel geometry is ambiguous"
        );
        crate::capture_macos_cursor::compose(&mut pixels, proof.bounds, scale)?;
    }
    proof.verify_unchanged(crate::capture_macos_ax::window(proof.id)?)?;
    Ok(pixels)
}
pub(crate) struct PreparedWindow {
    proof: MacWindowProof,
    source: Option<crate::capture_macos_sck::Window>,
    displays: Vec<crate::MacDisplayProof>,
    dimensions: (u32, u32),
    options: CaptureOptions,
    until: Instant,
}
impl PreparedWindow {
    pub(crate) fn new(window: &xcap::Window, options: &CaptureOptions) -> Result<Self> {
        crate::capture_macos_ax::main_thread()?;
        options.validate_for_capture(true)?;
        let proof = crate::capture_macos_ax::window(window.id()?)?;
        ensure!(
            proof.pid == window.pid()?,
            "Native macOS window process changed"
        );
        let until = Instant::now() + Duration::from_secs(20);
        let displays = crate::capture_macos_ax::displays()?;
        let source = crate::capture_macos_sck::Window::new(proof, until)?;
        let dimensions = if let Some(source) = &source {
            (source.width, source.height)
        } else {
            let mut matches = displays
                .iter()
                .filter(|display| display.logical.contains(proof.bounds));
            let display = matches
                .next()
                .context("Legacy native window physical extent requires one containing display")?;
            ensure!(
                matches.next().is_none(),
                "Legacy native window display is ambiguous"
            );
            proof.bounds.pixel_size(display.unrotated_scale()?)?
        };
        let prepared = Self {
            proof,
            source,
            displays,
            dimensions,
            options: *options,
            until,
        };
        prepared.check()?;
        Ok(prepared)
    }
    pub(crate) fn admission_bytes(&self) -> u64 {
        if self.options.window_shadow == CaptureWindowShadow::Include {
            // The public API supplies no pre-capture full-shadow extent.
            crate::MAX_CAPTURE_BYTES
        } else {
            u64::from(self.dimensions.0) * u64::from(self.dimensions.1) * 4
        }
    }
    fn check(&self) -> Result<()> {
        ensure!(
            Instant::now() < self.until,
            "Prepared native window deadline exceeded"
        );
        crate::capture_macos_loop::Topology::check()?;
        self.proof
            .verify_unchanged(crate::capture_macos_ax::window(self.proof.id)?)?;
        ensure!(
            crate::capture_macos_ax::displays()? == self.displays,
            "Prepared native window display topology changed"
        );
        Ok(())
    }
    pub(crate) fn capture(self) -> Result<image::RgbaImage> {
        self.check()?;
        let selected_bytes = u64::from(self.dimensions.0) * u64::from(self.dimensions.1) * 4;
        let mut pixels = if let Some(source) = &self.source {
            let image = source.capture(&self.options, self.until, selected_bytes)?;
            unsafe { decode(image.0) }?
        } else {
            legacy_window(self.proof, &self.options)?
        };
        self.check()?;
        ensure!(
            self.options.window_shadow == CaptureWindowShadow::Include
                || pixels.dimensions() == self.dimensions,
            "Prepared native window physical extent changed"
        );
        self.options.flatten_window(&mut pixels);
        Ok(pixels)
    }
}
struct Space(*const c_void);
impl Drop for Space {
    fn drop(&mut self) {
        unsafe { CGColorSpaceRelease(self.0) }
    }
}
struct BitmapContext(*mut c_void);
impl Drop for BitmapContext {
    fn drop(&mut self) {
        unsafe { CGContextRelease(self.0) }
    }
}
struct Provider(*const c_void);
impl Drop for Provider {
    fn drop(&mut self) {
        unsafe { CGDataProviderRelease(self.0) }
    }
}
extern "C" fn no_release(_: *mut c_void, _: *const c_void, _: usize) {}
pub(super) unsafe fn cursor_pixels(
    image: *const c_void,
    width: u32,
    height: u32,
) -> Result<image::RgbaImage> {
    ensure!(
        !image.is_null() && (1..=1024).contains(&width) && (1..=1024).contains(&height),
        "Native cursor size is unavailable/oversized"
    );
    let source_width = unsafe { CGImageGetWidth(image) };
    let source_height = unsafe { CGImageGetHeight(image) };
    let source_stride = unsafe { CGImageGetBytesPerRow(image) };
    ensure!(
        (1..=1024).contains(&source_width)
            && (1..=1024).contains(&source_height)
            && source_stride
                .checked_mul(source_height)
                .is_some_and(|bytes| bytes <= 4 * 1024 * 1024),
        "Native cursor backing exceeds its budget"
    );
    let space = Space(unsafe { CGColorSpaceCreateWithName(kCGColorSpaceSRGB) });
    ensure!(!space.0.is_null(), "Native cursor color space unavailable");
    let mut pixels = image::RgbaImage::new(width, height);
    let bytes: &mut [u8] = pixels.as_mut();
    let context = BitmapContext(unsafe {
        CGBitmapContextCreate(
            bytes.as_mut_ptr().cast(),
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
        "Cannot allocate bounded native cursor context"
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
    Ok(pixels)
}
/// The source is a locked, opaque BGRA CV buffer; one destination copy performs
/// public native SDR color management before the raw encoder receives it.
pub(super) unsafe fn opaque_bgra_srgb(
    width: u32,
    height: u32,
    stride: usize,
    bytes: &[u8],
    source_space: *const c_void,
    transfer: Option<&str>,
) -> Result<Vec<u8>> {
    crate::validate_image_size(width, height)?;
    let row = width as usize * 4;
    ensure!(
        stride >= row
            && stride
                .checked_mul(height as usize)
                .is_some_and(|extent| extent <= bytes.len())
            && bytes.len() <= 16 * 1024 * 1024,
        "Invalid native video memory extent"
    );
    for source in bytes.chunks(stride).take(height as usize) {
        ensure!(
            source[..row].chunks_exact(4).all(|pixel| pixel[3] == 255),
            "Display video source is not opaque BGRA8"
        );
    }
    ensure!(
        !source_space.is_null(),
        "Native source color space is unavailable"
    );
    ensure!(
        unsafe { CGColorSpaceGetModel(source_space) } == 1
            && unsafe { CGColorSpaceGetNumberOfComponents(source_space) } == 3
            && !unsafe { CGColorSpaceIsHDR(source_space) },
        "Native source color space is not a defined SDR RGB profile"
    );
    let profile = unsafe { CGColorSpaceCopyICCData(source_space) };
    ensure!(
        !profile.is_null(),
        "Native source has no explicit RGB color profile"
    );
    let profile = unsafe { CFData::wrap_under_create_rule(profile) };
    crate::capture_macos_pixels::validate_video_colour(transfer, Some(profile.bytes()))?;
    let provider = Provider(unsafe {
        CGDataProviderCreateWithData(
            ptr::null_mut(),
            bytes.as_ptr().cast(),
            bytes.len(),
            Some(no_release),
        )
    });
    ensure!(!provider.0.is_null(), "Cannot wrap native video bytes");
    let image = crate::capture_macos_sck::Image(unsafe {
        CGImageCreate(
            width as usize,
            height as usize,
            8,
            32,
            stride,
            source_space,
            0x2000 | 2,
            provider.0,
            ptr::null(),
            false,
            0,
        )
    });
    ensure!(
        !image.0.is_null(),
        "Cannot construct native video color image"
    );
    let space = Space(unsafe { CGColorSpaceCreateWithName(kCGColorSpaceSRGB) });
    ensure!(!space.0.is_null(), "Native sRGB space unavailable");
    let mut output = vec![0u8; row * height as usize];
    let context = BitmapContext(unsafe {
        CGBitmapContextCreate(
            output.as_mut_ptr().cast(),
            width as usize,
            height as usize,
            8,
            row,
            space.0,
            0x2000 | 2,
        )
    });
    ensure!(
        !context.0.is_null(),
        "Cannot construct bounded native video color context"
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
            image.0,
        )
    };
    drop(context);
    Ok(output)
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
    let context = BitmapContext(unsafe {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_color_context_preserves_bgra_channels_row_order_and_padding_without_screen_access() {
        let space = Space(unsafe { CGColorSpaceCreateWithName(kCGColorSpaceSRGB) });
        assert!(!space.0.is_null());
        let bytes = [
            13, 17, 23, 255, 31, 37, 41, 255, 199, 199, 199, 199, 47, 53, 59, 255, 61, 67, 71, 255,
            199, 199, 199, 199,
        ];
        let output = unsafe { opaque_bgra_srgb(2, 2, 12, &bytes, space.0, None) }.unwrap();
        assert_eq!(
            output,
            [
                13, 17, 23, 255, 31, 37, 41, 255, 47, 53, 59, 255, 61, 67, 71, 255
            ]
        );
    }
    #[test]
    fn native_video_requires_a_profile_and_converts_tagged_display_p3_without_screen_access() {
        let bytes = [32, 64, 128, 255];
        for transfer in [None, Some("sRGB"), Some("Linear")] {
            assert!(unsafe { opaque_bgra_srgb(1, 1, 4, &bytes, ptr::null(), transfer) }.is_err());
        }
        let device = Space(unsafe { CGColorSpaceCreateDeviceRGB() });
        assert!(!device.0.is_null());
        assert!(unsafe { opaque_bgra_srgb(1, 1, 4, &bytes, device.0, Some("sRGB")) }.is_err());
        let space = Space(unsafe { CGColorSpaceCreateWithName(kCGColorSpaceDisplayP3) });
        assert!(!space.0.is_null());
        let output = unsafe { opaque_bgra_srgb(1, 1, 4, &bytes, space.0, Some("sRGB")) }.unwrap();
        assert_eq!(output.len(), 4);
        assert_eq!(output[3], 255);
        assert!(output[0] < bytes[0] && output[1] < bytes[1] && output[2] > bytes[2]);
        assert!(unsafe { opaque_bgra_srgb(1, 1, 4, &bytes, space.0, Some("PQ")) }.is_err());
    }
}
