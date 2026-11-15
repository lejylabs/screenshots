//! Remove one full-width row band or full-height column band from RGBA pixels.

use crate::CaptureError;
use image::RgbaImage;

/// Remove columns when `vertical` is true, otherwise rows, without resampling retained pixels.
pub fn remove_image_strip(
    source: &RgbaImage,
    vertical: bool,
    start: u32,
    length: u32,
) -> Result<RgbaImage, CaptureError> {
    let (width, height) = source.dimensions();
    crate::validate_image_size(width, height)?;
    if source.as_raw().len() as u64 != u64::from(width) * u64::from(height) * 4 {
        return Err(CaptureError::Backend(
            "Strip source backing does not match its dimensions".into(),
        ));
    }
    let axis = if vertical { width } else { height };
    let end = start
        .checked_add(length)
        .filter(|end| length > 0 && start < axis && *end <= axis && length < axis)
        .ok_or_else(|| {
            CaptureError::Backend(
                "Strip must be nonempty, within the image, and leave at least one row or column"
                    .into(),
            )
        })?;
    let output_width = if vertical { width - length } else { width };
    let output_height = if vertical { height } else { height - length };
    crate::validate_image_size(output_width, output_height)?;
    let mut output = RgbaImage::new(output_width, output_height);
    let input = source.as_raw();
    let destination = output.as_mut();
    let source_row = width as usize * 4;
    if vertical {
        let output_row = output_width as usize * 4;
        let left = start as usize * 4;
        let right = end as usize * 4;
        for row in 0..height as usize {
            let input = &input[row * source_row..(row + 1) * source_row];
            let output = &mut destination[row * output_row..(row + 1) * output_row];
            output[..left].copy_from_slice(&input[..left]);
            output[left..].copy_from_slice(&input[right..]);
        }
    } else {
        let top = start as usize * source_row;
        let bottom = end as usize * source_row;
        destination[..top].copy_from_slice(&input[..top]);
        destination[top..].copy_from_slice(&input[bottom..]);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn pixels() -> RgbaImage {
        RgbaImage::from_fn(3, 4, |x, y| {
            Rgba([
                10 + x as u8,
                30 + y as u8,
                (x + y * 3) as u8,
                [0, 128, 255][x as usize],
            ])
        })
    }

    #[test]
    fn column_removal_preserves_exact_retained_pixels_and_source() {
        let source = pixels();
        let original = source.clone();
        for (start, length) in [(0, 1), (1, 1), (2, 1), (0, 2), (1, 2)] {
            let output = remove_image_strip(&source, true, start, length).unwrap();
            assert_eq!(output.dimensions(), (3 - length, 4));
            for y in 0..4 {
                for x in 0..3 - length {
                    let original_x = if x < start { x } else { x + length };
                    assert_eq!(output.get_pixel(x, y), source.get_pixel(original_x, y));
                }
            }
        }
        assert_eq!(source, original);
    }

    #[test]
    fn row_removal_preserves_exact_retained_pixels_and_source() {
        let source = pixels();
        let original = source.clone();
        for (start, length) in [(0, 1), (1, 2), (3, 1), (0, 3), (1, 3)] {
            let output = remove_image_strip(&source, false, start, length).unwrap();
            assert_eq!(output.dimensions(), (3, 4 - length));
            for y in 0..4 - length {
                let original_y = if y < start { y } else { y + length };
                for x in 0..3 {
                    assert_eq!(output.get_pixel(x, y), source.get_pixel(x, original_y));
                }
            }
        }
        assert_eq!(source, original);
    }

    #[test]
    fn invalid_strips_or_backings_fail_before_allocating_an_output() {
        let source = pixels();
        for vertical in [false, true] {
            let axis = if vertical {
                source.width()
            } else {
                source.height()
            };
            for (start, length) in [
                (0, 0),
                (axis, 1),
                (0, axis),
                (1, axis),
                (u32::MAX, 2),
                (2, u32::MAX),
            ] {
                assert!(remove_image_strip(&source, vertical, start, length).is_err());
            }
        }
        assert!(remove_image_strip(&RgbaImage::new(0, 1), true, 0, 1).is_err());
        let extra = RgbaImage::from_raw(1, 2, vec![0; 12]).unwrap();
        assert!(remove_image_strip(&extra, false, 0, 1).is_err());
    }
}
