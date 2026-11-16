//! Bounded Quartz-layout composition into an explicitly derived output grid.
use crate::capture_macos_coordinates::MacDesktopLayout;
use crate::{CaptureError, CapturedImage, PixelFormat};
use image::RgbaImage;
use std::{path::PathBuf, sync::Arc};

/// The caller must hold one native topology witness around admission, all source
/// captures, final identity/geometry validation, and this assembly operation.
pub fn stitch_macos_desktop_with_cancel(
    layout: &MacDesktopLayout,
    frames: &[CapturedImage],
    cancelled: &impl Fn() -> bool,
) -> Result<CapturedImage, CaptureError> {
    let cancelled = crate::image_cancel::latch(cancelled);
    let mut work = crate::image_cancel::ImageWork::new(&cancelled)?;
    let admitted = MacDesktopLayout::new(&layout.displays)
        .map_err(|error| CaptureError::Backend(error.to_string()))?;
    if &admitted != layout || frames.len() != layout.displays.len() {
        return Err(CaptureError::StaleRegion);
    }
    let mut sources = Vec::with_capacity(frames.len());
    for display in &layout.displays {
        let id = display.display_id.to_string();
        let mut matches = frames.iter().filter(|frame| frame.display_id == id);
        let frame = matches.next().ok_or(CaptureError::StaleRegion)?;
        let size = (display.pixel_width, display.pixel_height);
        if matches.next().is_some()
            || frame.window_id.is_some()
            || (frame.width, frame.height) != size
            || frame.pixels.dimensions() != size
            || frame.pixels.as_raw().len() as u64 != u64::from(size.0) * u64::from(size.1) * 4
            || frame.source_format != PixelFormat::Rgba8UnormSrgb
        {
            return Err(CaptureError::StaleRegion);
        }
        sources.push(frame);
    }
    work.check()?;
    let mut output = RgbaImage::new(layout.width, layout.height);
    for ((display, destination), frame) in
        layout.displays.iter().zip(&layout.placements).zip(sources)
    {
        if (destination.width, destination.height) == (display.pixel_width, display.pixel_height) {
            let stride = display.pixel_width as usize * 4;
            for y in 0..destination.height as usize {
                let start = ((destination.y as usize + y) * layout.width as usize
                    + destination.x as usize)
                    * 4;
                work.copy(
                    &mut output.as_mut()[start..start + stride],
                    &frame.pixels.as_raw()[y * stride..(y + 1) * stride],
                )?;
            }
        } else {
            // Same center-aligned nearest-neighbor rule as raster_ops, written
            // directly into the final grid so no resized-display buffer exists.
            for y in 0..destination.height {
                let source_y = ((u64::from(y) * 2 + 1) * u64::from(display.pixel_height)
                    / (u64::from(destination.height) * 2)) as u32;
                for x in 0..destination.width {
                    work.pixel()?;
                    let source_x = ((u64::from(x) * 2 + 1) * u64::from(display.pixel_width)
                        / (u64::from(destination.width) * 2))
                        as u32;
                    output.put_pixel(
                        destination.x + x,
                        destination.y + y,
                        *frame.pixels.get_pixel(source_x, source_y),
                    );
                }
            }
        }
    }
    work.check()?;
    Ok(CapturedImage {
        desktop_layout: None,
        display_id: "desktop".into(),
        window_id: None,
        width: layout.width,
        height: layout.height,
        png_path: PathBuf::new(),
        pixels: Arc::new(output),
        elapsed_ms: frames
            .iter()
            .fold(0_u128, |sum, frame| sum.saturating_add(frame.elapsed_ms)),
        source_format: PixelFormat::Rgba8UnormSrgb,
        display_geometry: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture_macos_coordinates::{MacDisplayCoordinates, tests::display};
    use image::Rgba;

    fn frame(display: &MacDisplayCoordinates) -> CapturedImage {
        CapturedImage {
            desktop_layout: None,
            display_id: display.display_id.to_string(),
            window_id: None,
            width: display.pixel_width,
            height: display.pixel_height,
            png_path: PathBuf::new(),
            pixels: Arc::new(RgbaImage::from_fn(
                display.pixel_width,
                display.pixel_height,
                |x, y| Rgba([x as u8, y as u8, display.display_id as u8, 71]),
            )),
            elapsed_ms: 1,
            source_format: PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        }
    }

    #[test]
    fn exact_native_pixels_and_alpha_survive_and_low_dpi_expansion_is_explicit() {
        let low = display(1, -2.0, 0.0, 2.0, 2.0, 1.0);
        let high = display(2, 0.0, 0.0, 2.0, 2.0, 2.0);
        let layout = MacDesktopLayout::new(&[low, high]).unwrap();
        let frames = [frame(&high), frame(&low)];
        let original = frames[0].pixels.clone();
        let output = stitch_macos_desktop_with_cancel(&layout, &frames, &|| false).unwrap();
        assert_eq!(output.pixels.dimensions(), (8, 4));
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(
                    output.pixels.get_pixel(x, y),
                    frames[1].pixels.get_pixel(x / 2, y / 2)
                );
                assert_eq!(
                    output.pixels.get_pixel(x + 4, y),
                    frames[0].pixels.get_pixel(x, y)
                );
            }
        }
        assert_eq!(&*original, &*frames[0].pixels);
        assert!(output.display_geometry.is_none());
        assert_eq!(output.display_id, "desktop");
    }

    #[test]
    fn derived_grid_keeps_transparent_holes_and_already_oriented_rotation() {
        let low = display(1, 0.0, 0.0, 2.0, 2.0, 1.0);
        let mut rotated = display(2, 3.0, 1.0, 2.0, 3.0, 2.0);
        rotated.rotation = 90.0;
        std::mem::swap(
            &mut rotated.mode_pixel_width,
            &mut rotated.mode_pixel_height,
        );
        let layout = MacDesktopLayout::new(&[low, rotated]).unwrap();
        let frames = [frame(&low), frame(&rotated)];
        let output = stitch_macos_desktop_with_cancel(&layout, &frames, &|| false).unwrap();
        assert_eq!(output.pixels.get_pixel(4, 0).0, [0; 4]);
        for y in 0..6 {
            for x in 0..4 {
                assert_eq!(
                    output.pixels.get_pixel(x + 6, y + 2),
                    frames[1].pixels.get_pixel(x, y)
                );
            }
        }
    }

    #[test]
    fn malformed_frames_and_forged_placements_fail_before_output_allocation() {
        let source = display(1, 0.0, 0.0, 2.0, 2.0, 2.0);
        let layout = MacDesktopLayout::new(&[source]).unwrap();
        assert!(stitch_macos_desktop_with_cancel(&layout, &[], &|| false).is_err());
        let mut bad = frame(&source);
        bad.width = 1;
        assert!(stitch_macos_desktop_with_cancel(&layout, &[bad], &|| false).is_err());
        let mut bad = frame(&source);
        bad.window_id = Some("1".into());
        assert!(stitch_macos_desktop_with_cancel(&layout, &[bad], &|| false).is_err());
        let mut forged = layout;
        forged.placements[0].x = u32::MAX;
        assert!(stitch_macos_desktop_with_cancel(&forged, &[frame(&source)], &|| false).is_err());
    }

    #[test]
    fn cancellation_is_shared_across_short_copy_and_resampling_rows() {
        for scale in [1.0, 2.0] {
            let first = display(1, 0.0, 0.0, 16.0, 1024.0, 1.0);
            let second = display(2, 16.0, 0.0, 16.0, 1024.0, scale);
            let layout = MacDesktopLayout::new(&[first, second]).unwrap();
            let count = std::cell::Cell::new(0);
            let result = stitch_macos_desktop_with_cancel(
                &layout,
                &[frame(&first), frame(&second)],
                &|| {
                    count.set(count.get() + 1);
                    count.get() == 5
                },
            );
            assert!(matches!(result, Err(CaptureError::Canceled)));
            assert_eq!(count.get(), 5);
        }
    }
}
