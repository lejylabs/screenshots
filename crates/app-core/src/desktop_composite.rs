//! Explicit physical-pixel desktop composition; no capture stream or retained cache.

use crate::{
    CaptureBackend, CaptureError, CaptureRequest, CapturedImage, DisplayInfo, PixelFormat,
    XcapBackend,
};
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
        image::imageops::replace(
            &mut pixels,
            frame.pixels.as_ref(),
            i64::from(x),
            i64::from(y),
        );
    }
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
    // XCap's non-Windows coordinates have not been calibrated against returned pixels.
    if !cfg!(target_os = "windows") {
        return Err(CaptureError::Unsupported(
            "stitched desktop capture requires validated Windows physical-pixel display coordinates",
        ));
    }
    let before = backend.list_displays()?;
    if before
        .iter()
        .any(|display| (display.scale_factor - 1.0).abs() > f32::EPSILON)
    {
        return Err(CaptureError::Unsupported(
            "stitched native capture currently requires 100% display scaling; mixed-DPI coordinates need native acceptance",
        ));
    }
    layout(&before)?;
    let frames = backend.capture_batch(
        before
            .iter()
            .map(|d| CaptureRequest::Display(d.id.clone()))
            .collect(),
    )?;
    validate_native_frames(&before, &frames)?;
    let after = backend.list_displays()?;
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
    stitch_desktop(&before, &frames)
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
}
