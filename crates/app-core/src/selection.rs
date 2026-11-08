use std::time::Instant;

use crate::CapturedImage;
use anyhow::{Result, ensure};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PixelRegion {
    /// Map window-local logical coordinates to the frozen image's physical pixels.
    pub fn from_drag(
        start: [f32; 2],
        end: [f32; 2],
        viewport: [f32; 2],
        image: [u32; 2],
    ) -> Option<Self> {
        if start
            .into_iter()
            .chain(end)
            .chain(viewport)
            .any(|v| !v.is_finite())
            || viewport.iter().any(|v| *v <= 0.0)
            || image.contains(&0)
        {
            return None;
        }
        let mut lo = [0; 2];
        let mut hi = [0; 2];
        for axis in 0..2 {
            let a = start[axis].clamp(0.0, viewport[axis]);
            let b = end[axis].clamp(0.0, viewport[axis]);
            if a == b {
                return None;
            }
            let scale = image[axis] as f32 / viewport[axis];
            lo[axis] = (a.min(b) * scale).floor() as u32;
            hi[axis] = ((a.max(b) * scale).ceil() as u32).min(image[axis]);
        }
        Some(Self {
            x: lo[0],
            y: lo[1],
            width: hi[0] - lo[0],
            height: hi[1] - lo[1],
        })
    }
}

pub fn crop_snapshot(source: &CapturedImage, region: PixelRegion) -> Result<CapturedImage> {
    let started = Instant::now();
    let image = &source.pixels;
    crate::capture::validate_image_size(region.width, region.height)?;
    ensure!(
        region.width > 0
            && region.height > 0
            && region
                .x
                .checked_add(region.width)
                .is_some_and(|x| x <= image.width())
            && region
                .y
                .checked_add(region.height)
                .is_some_and(|y| y <= image.height()),
        "selection is outside the snapshot"
    );
    let path = source
        .png_path
        .with_file_name(format!("selection-{}.png", uuid::Uuid::new_v4()));
    let pixels = std::sync::Arc::new(
        image::imageops::crop_imm(
            image.as_ref(),
            region.x,
            region.y,
            region.width,
            region.height,
        )
        .to_image(),
    );
    Ok(CapturedImage {
        png_path: path,
        pixels,
        width: region.width,
        height: region.height,
        elapsed_ms: started.elapsed().as_millis(),
        ..source.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_handles_dpi_reverse_direction_and_edges() {
        assert_eq!(
            PixelRegion::from_drag([40.2, 30.2], [10.1, 5.1], [100.0, 50.0], [150, 100]),
            Some(PixelRegion {
                x: 15,
                y: 10,
                width: 46,
                height: 51
            })
        );
        assert_eq!(
            PixelRegion::from_drag([-20.0, -10.0], [200.0, 70.0], [100.0, 50.0], [150, 100]),
            Some(PixelRegion {
                x: 0,
                y: 0,
                width: 150,
                height: 100
            })
        );
        assert!(
            PixelRegion::from_drag([5.0, 5.0], [5.0, 8.0], [100.0, 50.0], [150, 100]).is_none()
        );
        assert!(
            PixelRegion::from_drag([f32::NAN, 5.0], [7.0, 8.0], [100.0, 50.0], [150, 100])
                .is_none()
        );
    }

    #[test]
    fn crop_uses_frozen_pixels_and_preserves_source() -> Result<()> {
        let path =
            std::env::temp_dir().join(format!("selection-test-{}.png", uuid::Uuid::new_v4()));
        let image =
            image::RgbaImage::from_fn(4, 3, |x, y| image::Rgba([x as u8, y as u8, 20, 255]));
        image.save(&path)?;
        let source = CapturedImage {
            window_id: None,
            display_id: "test".into(),
            width: 4,
            height: 3,
            png_path: path.clone(),
            pixels: std::sync::Arc::new(image.clone()),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
        };
        let result = crop_snapshot(
            &source,
            PixelRegion {
                x: 1,
                y: 1,
                width: 2,
                height: 2,
            },
        )?;
        assert!(std::sync::Arc::ptr_eq(
            &source.pixels,
            &source.clone().pixels
        ));
        assert!(!result.png_path.exists());
        let cropped = &result.pixels;
        assert_eq!(cropped.dimensions(), (2, 2));
        assert_eq!(cropped.get_pixel(0, 0), image.get_pixel(1, 1));
        assert_eq!(image::open(&path)?.into_rgba8(), image);
        assert!(
            crop_snapshot(
                &source,
                PixelRegion {
                    x: 3,
                    y: 2,
                    width: 2,
                    height: 2
                }
            )
            .is_err()
        );
        std::fs::remove_file(path)?;

        Ok(())
    }
}
