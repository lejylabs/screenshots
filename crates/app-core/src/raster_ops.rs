//! Cancellable versions of the existing exact RGBA transform/composite rules.
use crate::{CaptureError, image_cancel::ImageWork};
use image::RgbaImage;

fn admit(source: &RgbaImage) -> Result<(), CaptureError> {
    crate::validate_image_size(source.width(), source.height())?;
    if source.as_raw().len() as u64 != u64::from(source.width()) * u64::from(source.height()) * 4 {
        return Err(CaptureError::Backend(
            "Image backing does not match its dimensions".into(),
        ));
    }
    Ok(())
}

pub fn rotate_rgba_with_cancel(
    mut source: RgbaImage,
    degrees: u16,
    cancelled: impl Fn() -> bool,
) -> Result<RgbaImage, CaptureError> {
    let mut work = ImageWork::new(&cancelled)?;
    admit(&source)?;
    if degrees == 180 {
        let pixels = source.as_mut().as_chunks_mut::<4>().0;
        for index in 0..pixels.len() / 2 {
            work.pixel()?;
            work.pixel()?;
            pixels.swap(index, pixels.len() - 1 - index);
        }
        work.check()?;
        return Ok(source);
    }
    if !matches!(degrees, 90 | 270) {
        return Err(CaptureError::Backend(
            "Rotation must be 90, 180 or 270 degrees".into(),
        ));
    }
    let mut output = RgbaImage::new(source.height(), source.width());
    for (x, y, pixel) in output.enumerate_pixels_mut() {
        work.pixel()?;
        let (sx, sy) = if degrees == 90 {
            (y, source.height() - 1 - x)
        } else {
            (source.width() - 1 - y, x)
        };
        *pixel = *source.get_pixel(sx, sy);
    }
    work.check()?;
    Ok(output)
}

pub fn flip_rgba_with_cancel(
    mut source: RgbaImage,
    horizontal: bool,
    cancelled: impl Fn() -> bool,
) -> Result<RgbaImage, CaptureError> {
    let mut work = ImageWork::new(&cancelled)?;
    admit(&source)?;
    let (width, height) = (source.width() as usize, source.height() as usize);
    if horizontal && width == 1 {
        work.check()?;
        return Ok(source);
    }
    let pixels = source.as_mut().as_chunks_mut::<4>().0;
    if horizontal {
        for row in pixels.chunks_exact_mut(width) {
            for x in 0..width / 2 {
                work.pixel()?;
                work.pixel()?;
                row.swap(x, width - 1 - x);
            }
        }
    } else {
        for y in 0..height / 2 {
            for x in 0..width {
                work.pixel()?;
                work.pixel()?;
                pixels.swap(y * width + x, (height - 1 - y) * width + x);
            }
        }
    }
    work.check()?;
    Ok(source)
}

#[cfg(test)]
#[path = "raster_ops_tests.rs"]
mod tests;

pub fn resize_nearest_rgba_with_cancel(
    source: &RgbaImage,
    width: u32,
    height: u32,
    cancelled: impl Fn() -> bool,
) -> Result<RgbaImage, CaptureError> {
    let mut work = ImageWork::new(&cancelled)?;
    admit(source)?;
    crate::validate_image_size(width, height)?;
    let mut output = RgbaImage::new(width, height);
    for (x, y, pixel) in output.enumerate_pixels_mut() {
        work.pixel()?;
        let sx =
            ((u64::from(x) * 2 + 1) * u64::from(source.width()) / (u64::from(width) * 2)) as u32;
        let sy =
            ((u64::from(y) * 2 + 1) * u64::from(source.height()) / (u64::from(height) * 2)) as u32;
        *pixel = *source.get_pixel(sx, sy);
    }
    work.check()?;
    Ok(output)
}

fn composite(
    output: &mut RgbaImage,
    source: &RgbaImage,
    x: u32,
    y: u32,
    work: &mut ImageWork<'_, impl Fn() -> bool>,
) -> Result<(), CaptureError> {
    let width = source.width().min(output.width().saturating_sub(x));
    let height = source.height().min(output.height().saturating_sub(y));
    work.charge(4)?;
    if width == 0 || height == 0 {
        return Ok(());
    }
    for sy in 0..height {
        for sx in 0..width {
            work.pixel()?;
            let source = source.get_pixel(sx, sy);
            let alpha = u32::from(source[3]);
            if alpha == 0 {
                continue;
            }
            let destination = output.get_pixel_mut(x + sx, y + sy);
            if alpha == 255 {
                *destination = *source;
                continue;
            }
            let old_weight = u32::from(destination[3]) * (255 - alpha);
            let denominator = alpha * 255 + old_weight;
            for channel in 0..3 {
                let numerator = u32::from(source[channel]) * alpha * 255
                    + u32::from(destination[channel]) * old_weight;
                destination[channel] = ((numerator + denominator / 2) / denominator) as u8;
            }
            destination[3] = ((denominator + 127) / 255) as u8;
        }
    }
    Ok(())
}

pub fn composite_rgba_with_cancel(
    output: &mut RgbaImage,
    source: &RgbaImage,
    x: u32,
    y: u32,
    cancelled: impl Fn() -> bool,
) -> Result<(), CaptureError> {
    let mut work = ImageWork::new(&cancelled)?;
    admit(output)?;
    admit(source)?;
    composite(output, source, x, y, &mut work)?;
    work.check()
}

pub fn canvas_rgba_with_cancel(
    source: &RgbaImage,
    padding: u32,
    background: [u8; 4],
    cancelled: impl Fn() -> bool,
) -> Result<RgbaImage, CaptureError> {
    let mut work = ImageWork::new(&cancelled)?;
    admit(source)?;
    let dimensions = source
        .width()
        .checked_add(padding.checked_mul(2).ok_or(CaptureError::MemoryLimit)?)
        .zip(
            source
                .height()
                .checked_add(padding.checked_mul(2).ok_or(CaptureError::MemoryLimit)?),
        )
        .ok_or(CaptureError::MemoryLimit)?;
    crate::validate_image_size(dimensions.0, dimensions.1)?;
    let mut output = RgbaImage::new(dimensions.0, dimensions.1);
    for chunk in output.as_mut().chunks_mut(64 * 1024) {
        work.check()?;
        chunk.as_chunks_mut::<4>().0.fill(background);
    }
    composite(&mut output, source, padding, padding, &mut work)?;
    work.check()?;
    Ok(output)
}
