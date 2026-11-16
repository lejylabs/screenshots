//! Explicit physical-pixel desktop composition; no capture stream or retained cache.

use crate::{CaptureError, CaptureRequest, CapturedImage, DisplayInfo, PixelFormat, XcapBackend};
use image::RgbaImage;
use std::{collections::HashSet, path::PathBuf, sync::Arc};

const MAX_DISPLAYS: usize = 32;

struct Layout {
    left: i64,
    top: i64,
    width: u32,
    height: u32,
}

fn layout(displays: &[DisplayInfo]) -> Result<Layout, CaptureError> {
    if displays.is_empty() || displays.len() > MAX_DISPLAYS {
        return Err(CaptureError::Backend(
            "Select between 1 and 32 displays".into(),
        ));
    }
    let mut ids = HashSet::new();
    let mut source_bytes = 0_u64;
    let mut left = i64::MAX;
    let mut top = i64::MAX;
    let mut right = i64::MIN;
    let mut bottom = i64::MIN;
    for (index, display) in displays.iter().enumerate() {
        crate::validate_image_size(display.width, display.height)?;
        if display.id.is_empty() || display.id.len() > 64 || !ids.insert(&display.id) {
            return Err(CaptureError::Backend(
                "Display identities must be unique".into(),
            ));
        }
        if !display.scale_factor.is_finite()
            || display.scale_factor <= 0.0
            || !matches!(display.rotation, 0.0 | 90.0 | 180.0 | 270.0)
        {
            return Err(CaptureError::StaleRegion);
        }
        source_bytes += u64::from(display.width) * u64::from(display.height) * 4;
        if source_bytes > crate::MAX_CAPTURE_BYTES {
            return Err(CaptureError::MemoryLimit);
        }
        let x = i64::from(display.x);
        let y = i64::from(display.y);
        let end_x = x + i64::from(display.width);
        let end_y = y + i64::from(display.height);
        for other in &displays[..index] {
            let other_x = i64::from(other.x);
            let other_y = i64::from(other.y);
            if x < other_x + i64::from(other.width)
                && end_x > other_x
                && y < other_y + i64::from(other.height)
                && end_y > other_y
            {
                return Err(CaptureError::Backend("Display pixel bounds overlap; mirrored or logical-coordinate layouts cannot be stitched safely".into()));
            }
        }
        left = left.min(x);
        top = top.min(y);
        right = right.max(end_x);
        bottom = bottom.max(end_y);
    }
    let width = u32::try_from(right - left).map_err(|_| CaptureError::MemoryLimit)?;
    let height = u32::try_from(bottom - top).map_err(|_| CaptureError::MemoryLimit)?;
    crate::validate_image_size(width, height)?;
    Ok(Layout {
        left,
        top,
        width,
        height,
    })
}

/// Compose one frozen image per display, preserving alpha and transparent gaps.
/// Coordinates must be physical desktop pixels; logical/mirrored layouts are rejected.
pub fn stitch_desktop(
    displays: &[DisplayInfo],
    frames: &[CapturedImage],
) -> Result<CapturedImage, CaptureError> {
    stitch_desktop_with_cancel(displays, frames, &|| false)
}

pub fn stitch_desktop_with_cancel(
    displays: &[DisplayInfo],
    frames: &[CapturedImage],
    cancelled: &impl Fn() -> bool,
) -> Result<CapturedImage, CaptureError> {
    let cancelled = crate::image_cancel::latch(cancelled);
    let mut work = crate::image_cancel::ImageWork::new(&cancelled)?;
    let layout = layout(displays)?;
    if frames.len() != displays.len() {
        return Err(CaptureError::Backend(
            "Each display needs exactly one captured frame".into(),
        ));
    }
    let mut ordered = Vec::with_capacity(displays.len());
    for display in displays {
        let matches = frames
            .iter()
            .filter(|frame| frame.display_id == display.id)
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(CaptureError::StaleRegion);
        }
        let frame = matches[0];
        if frame.window_id.is_some()
            || (frame.width, frame.height) != (display.width, display.height)
            || frame.pixels.dimensions() != (display.width, display.height)
            || frame.pixels.as_raw().len() as u64
                != u64::from(display.width) * u64::from(display.height) * 4
            || frame.source_format != PixelFormat::Rgba8UnormSrgb
        {
            return Err(CaptureError::StaleRegion);
        }
        ordered.push(frame);
    }
    let mut pixels = RgbaImage::new(layout.width, layout.height);
    for (display, frame) in displays.iter().zip(ordered) {
        let x = (i64::from(display.x) - layout.left) as u32;
        let y = (i64::from(display.y) - layout.top) as u32;
        let stride = display.width as usize * 4;
        for row in 0..display.height as usize {
            let target = ((y as usize + row) * layout.width as usize + x as usize) * 4;
            let source = row * stride;
            work.copy(
                &mut pixels.as_mut()[target..target + stride],
                &frame.pixels.as_raw()[source..source + stride],
            )?;
        }
    }
    work.check()?;
    Ok(CapturedImage {
        display_id: String::new(),
        window_id: None,
        width: layout.width,
        height: layout.height,
        png_path: PathBuf::new(),
        pixels: Arc::new(pixels),
        elapsed_ms: frames
            .iter()
            .fold(0_u128, |sum, frame| sum.saturating_add(frame.elapsed_ms)),
        source_format: PixelFormat::Rgba8UnormSrgb,
        display_geometry: None,
    })
}

/// Capture all physical-pixel displays once, then verify topology before composing.
pub fn capture_stitched_desktop(backend: &XcapBackend) -> Result<CapturedImage, CaptureError> {
    capture_stitched_desktop_with_cancel(backend, &|| Ok(()))
}

pub fn capture_stitched_desktop_with_cancel(
    backend: &XcapBackend,
    check: &crate::NativeCaptureCheck<'_>,
) -> Result<CapturedImage, CaptureError> {
    check()?;
    // XCap's non-Windows coordinates have not been calibrated against returned pixels.
    if !cfg!(target_os = "windows") {
        return Err(CaptureError::Unsupported(
            "stitched desktop capture requires validated Windows physical-pixel display coordinates",
        ));
    }
    backend.capture_physical_desktop_with_cancel(check)
}

pub(crate) fn capture_stitched_desktop_in_worker(
    backend: &XcapBackend,
    check: &crate::NativeCaptureCheck<'_>,
) -> Result<CapturedImage, CaptureError> {
    if !cfg!(target_os = "windows") || crate::native_capture_job::executor().is_some() {
        return Err(CaptureError::Unsupported(
            "Physical desktop assembly requires the native Windows worker",
        ));
    }
    let before = backend.list_displays_with_cancel(check)?;
    layout(&before)?;
    let frames = backend.capture_batch_with_cancel(
        before
            .iter()
            .map(|d| CaptureRequest::Display(d.id.clone()))
            .collect(),
        check,
    )?;
    validate_native_frames(&before, &frames)?;
    let after = backend.list_displays_with_cancel(check)?;
    if before.len() != after.len()
        || before.iter().any(|a| {
            !after.iter().any(|b| {
                a.id == b.id
                    && a.x == b.x
                    && a.y == b.y
                    && a.width == b.width
                    && a.height == b.height
                    && a.scale_factor == b.scale_factor
                    && a.rotation == b.rotation
            })
        })
    {
        return Err(CaptureError::StaleRegion);
    }
    let failure = std::cell::RefCell::new(None);
    let result = stitch_desktop_with_cancel(&before, &frames, &|| {
        if failure.borrow().is_some() {
            return true;
        }
        if let Err(error) = check() {
            *failure.borrow_mut() = Some(error);
            true
        } else {
            false
        }
    });
    if let Some(error) = failure.into_inner() {
        return Err(error);
    }
    let mut output = result?;
    output.display_id = "desktop".into();
    Ok(output)
}

fn validate_native_frames(
    displays: &[DisplayInfo],
    frames: &[CapturedImage],
) -> Result<(), CaptureError> {
    for display in displays {
        let expected = crate::DisplayGeometry {
            x: display.x,
            y: display.y,
            width: display.width,
            height: display.height,
            scale_factor: display.scale_factor,
            rotation: display.rotation,
        };
        if !frames
            .iter()
            .any(|frame| frame.display_id == display.id && frame.display_geometry == Some(expected))
        {
            return Err(CaptureError::StaleRegion);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn display(id: &str, x: i32, y: i32, width: u32, height: u32) -> DisplayInfo {
        DisplayInfo {
            id: id.into(),
            name: id.into(),
            x,
            y,
            width,
            height,
            scale_factor: 1.0,
            rotation: 0.0,
            is_primary: id == "a",
        }
    }
    fn frame(display: &DisplayInfo, rgba: [u8; 4]) -> CapturedImage {
        CapturedImage {
            display_id: display.id.clone(),
            window_id: None,
            width: display.width,
            height: display.height,
            png_path: PathBuf::new(),
            pixels: Arc::new(RgbaImage::from_pixel(
                display.width,
                display.height,
                image::Rgba(rgba),
            )),
            elapsed_ms: 0,
            source_format: PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        }
    }
    #[test]
    fn native_frames_must_retain_matching_verified_geometry() {
        let display = display("a", -2, 0, 2, 2);
        let mut frame = frame(&display, [1, 2, 3, 255]);
        assert!(
            validate_native_frames(std::slice::from_ref(&display), std::slice::from_ref(&frame))
                .is_err()
        );
        frame.display_geometry = Some(crate::DisplayGeometry {
            x: display.x,
            y: display.y,
            width: display.width,
            height: display.height,
            scale_factor: display.scale_factor,
            rotation: display.rotation,
        });
        assert!(
            validate_native_frames(std::slice::from_ref(&display), std::slice::from_ref(&frame))
                .is_ok()
        );
        frame.display_geometry.as_mut().unwrap().x += 1;
        assert!(validate_native_frames(&[display], &[frame]).is_err());
    }
    #[test]
    fn negative_origins_and_gaps_preserve_exact_alpha() {
        let a = display("a", -2, 0, 2, 2);
        let mut b = display("b", 1, -1, 1, 2);
        b.scale_factor = 1.5;
        let frames = [frame(&b, [4, 5, 6, 128]), frame(&a, [1, 2, 3, 255])];
        let result = stitch_desktop(&[a, b], &frames).unwrap();
        assert_eq!(result.pixels.dimensions(), (4, 3));
        assert_eq!(result.pixels.get_pixel(0, 1).0, [1, 2, 3, 255]);
        assert_eq!(result.pixels.get_pixel(3, 0).0, [4, 5, 6, 128]);
        assert_eq!(result.pixels.get_pixel(2, 1).0, [0, 0, 0, 0]);
        assert!(result.png_path.as_os_str().is_empty());
    }
    #[test]
    fn inconsistent_or_excessive_layouts_fail_before_composition() {
        let a = display("a", 0, 0, 2, 2);
        let image = frame(&a, [1, 2, 3, 255]);
        assert!(stitch_desktop(&[], &[]).is_err());
        assert!(stitch_desktop(&[a.clone(), display("b", 1, 1, 2, 2)], &[]).is_err());
        assert!(stitch_desktop(&[a.clone(), a.clone()], &[]).is_err());
        assert!(stitch_desktop(&[a.clone()], &[]).is_err());
        assert!(stitch_desktop(&[a.clone()], &[image.clone(), image.clone()]).is_err());
        let mut mismatch = image;
        mismatch.width = 1;
        assert!(stitch_desktop(&[a.clone()], &[mismatch]).is_err());
        let far = display("b", i32::MAX, 0, 1, 1);
        assert!(layout(&[a.clone(), far]).is_err());
        let many = vec![a; 33];
        assert!(layout(&many).is_err());
    }

    #[test]
    fn short_rows_share_cancellation_work_and_do_not_change_sources() {
        let display = display("a", -1, 0, 1, 40000);
        let frame = frame(&display, [13, 29, 71, 0]);
        let original = frame.pixels.clone();
        let calls = std::cell::Cell::new(0);
        assert!(matches!(
            stitch_desktop_with_cancel(&[display], &[frame], &|| {
                calls.set(calls.get() + 1);
                calls.get() >= 3
            }),
            Err(CaptureError::Canceled)
        ));
        assert_eq!(calls.get(), 3);
        assert!(original.pixels().all(|pixel| pixel.0 == [13, 29, 71, 0]));
    }

    #[test]
    fn isolated_desktop_call_has_one_unambiguous_raw_frame() {
        let call = crate::NativeCaptureCall::Desktop {
            options: Default::default(),
        };
        call.validate().unwrap();
        assert_eq!(call.frame_count(), Some(1));
        assert!(!call.is_enumeration());
        call.validate_frame_metadata(0, "desktop", None, 3, 2, None)
            .unwrap();
        for (index, id, window) in [
            (1, "desktop", None),
            (0, "other", None),
            (0, "desktop", Some("window")),
        ] {
            assert!(
                call.validate_frame_metadata(index, id, window, 3, 2, None)
                    .is_err()
            );
        }
        assert!(
            crate::NativeCaptureCall::Desktop {
                options: crate::CaptureOptions {
                    delay_ms: 1,
                    ..Default::default()
                }
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    #[ignore = "optimized synthetic copy comparison; no hardware acceptance"]
    fn physical_desktop_copy_performance() {
        let a = display("a", -3840, 0, 3840, 2160);
        let mut b = display("b", 0, 0, 3840, 2160);
        b.scale_factor = 1.5;
        b.rotation = 180.0;
        let frames = [frame(&a, [11, 29, 71, 255]), frame(&b, [47, 61, 89, 128])];
        let displays = [a, b];
        let mut before = Vec::new();
        let mut after = Vec::new();
        for sample in 0..105 {
            let old_copy = || {
                let started = std::time::Instant::now();
                let mut oracle = RgbaImage::new(7680, 2160);
                image::imageops::replace(&mut oracle, frames[0].pixels.as_ref(), 0, 0);
                image::imageops::replace(&mut oracle, frames[1].pixels.as_ref(), 3840, 0);
                (started.elapsed().as_micros(), oracle)
            };
            let new_copy = || {
                let started = std::time::Instant::now();
                let output = stitch_desktop(&displays, &frames).unwrap();
                (started.elapsed().as_micros(), output)
            };
            let ((old, oracle), (new, output)) = if sample % 2 == 0 {
                (old_copy(), new_copy())
            } else {
                let new = new_copy();
                (old_copy(), new)
            };
            assert_eq!(output.pixels.as_ref(), &oracle);
            if sample >= 5 {
                before.push(old);
                after.push(new);
            }
        }
        before.sort_unstable();
        after.sort_unstable();
        eprintln!(
            "{{\"scope\":\"two4K synthetic already-oriented source rasters; alternating order; copy+allocation only\",\"samples\":100,\"old_p50_us\":{},\"old_p95_us\":{},\"new_p50_us\":{},\"new_p95_us\":{},\"source_bytes\":{},\"output_bytes\":{}}}",
            before[49],
            before[94],
            after[49],
            after[94],
            2 * 3840 * 2160 * 4,
            7680 * 2160 * 4
        );
    }
}
